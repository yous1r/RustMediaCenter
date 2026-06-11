use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};

use anyhow::Context;
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::db::Database;
use rmc_core::models::Movie;

const DEFAULT_FFPROBE_TIMEOUT_SECS: u64 = 15;

static PROBE_LOCKS: OnceLock<Mutex<HashMap<i64, Arc<Mutex<()>>>>> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

fn ffprobe_cmd() -> String {
    std::env::var("RMC_FFPROBE_CMD").unwrap_or_else(|_| "ffprobe".to_string())
}

fn ffprobe_timeout_secs() -> u64 {
    std::env::var("RMC_FFPROBE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_FFPROBE_TIMEOUT_SECS)
}

async fn probe_lock(movie_id: i64) -> Arc<Mutex<()>> {
    let map = PROBE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().await;
    guard
        .entry(movie_id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn seconds_to_minutes(runtime_seconds: u32) -> Option<u16> {
    let minutes = runtime_seconds.div_ceil(60);
    minutes.try_into().ok()
}

async fn resolve_probe_input(file_path: &Path) -> Result<(String, Option<String>), anyhow::Error> {
    if file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .eq_ignore_ascii_case("strm")
    {
        let content = tokio::fs::read_to_string(file_path)
            .await
            .context("Failed to read strm file for ffprobe")?;
        let source = crate::strm::StrmParser::parse(&content)
            .context("Invalid strm file content for ffprobe")?;
        let headers = source.ffmpeg_header_value();
        Ok((source.url, headers))
    } else {
        Ok((file_path.to_string_lossy().into_owned(), None))
    }
}

async fn probe_duration_seconds(input: &str, headers: Option<&str>) -> Result<u32, anyhow::Error> {
    let mut command = Command::new(ffprobe_cmd());
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("json");

    if let Some(headers) = headers.filter(|value| !value.trim().is_empty()) {
        command.arg("-headers").arg(headers);
    }

    command.arg(input);

    let timeout = std::time::Duration::from_secs(ffprobe_timeout_secs());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .context("ffprobe timed out")?
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffprobe failed: {}", stderr.trim());
    }

    let parsed: FfprobeOutput =
        serde_json::from_slice(&output.stdout).context("Failed to parse ffprobe json")?;
    let seconds = parsed
        .format
        .and_then(|format| format.duration)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.round() as u32)
        .filter(|value| *value > 0)
        .context("ffprobe did not return a valid duration")?;

    Ok(seconds)
}

pub async fn ensure_movie_runtime_seconds(
    db: &Database,
    movie_id: i64,
) -> Result<Movie, anyhow::Error> {
    let lock = probe_lock(movie_id).await;
    let _guard = lock.lock().await;

    let movie = db
        .get_available_movie_by_id(movie_id)
        .await
        .context("Failed to load movie before runtime probe")?;
    if movie.runtime_seconds.is_some() {
        return Ok(movie);
    }

    let (input, headers) = resolve_probe_input(&movie.file_path).await?;
    let runtime_seconds = probe_duration_seconds(&input, headers.as_deref()).await?;

    db.update_movie_runtime(
        movie_id,
        Some(runtime_seconds),
        seconds_to_minutes(runtime_seconds),
    )
    .await
    .context("Failed to persist probed runtime")?;

    db.get_available_movie_by_id(movie_id)
        .await
        .context("Failed to reload movie after runtime probe")
}

pub async fn load_movie_with_runtime(db: &Database, movie_id: i64) -> Result<Movie, anyhow::Error> {
    let movie = db
        .get_available_movie_by_id(movie_id)
        .await
        .context("Failed to load movie before optional runtime probe")?;
    if movie.runtime_seconds.is_some() {
        return Ok(movie);
    }

    match ensure_movie_runtime_seconds(db, movie_id).await {
        Ok(movie) => Ok(movie),
        Err(err) => {
            tracing::warn!(movie_id, error = %err, "Failed to probe runtime seconds, falling back to stored metadata");
            Ok(movie)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex as StdMutex;

    static ENV_MUTEX: StdMutex<()> = StdMutex::new(());

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        entries: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn set(entries: &[(&'static str, String)]) -> Self {
            let lock = ENV_MUTEX.lock().unwrap();
            let mut previous = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                previous.push((*key, std::env::var(key).ok()));
                unsafe {
                    std::env::set_var(key, value);
                }
            }
            Self {
                _lock: lock,
                entries: previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, previous) in &self.entries {
                match previous {
                    Some(value) => unsafe {
                        std::env::set_var(key, value);
                    },
                    None => unsafe {
                        std::env::remove_var(key);
                    },
                }
            }
        }
    }

    fn write_fake_ffprobe(temp_dir: &tempfile::TempDir) -> std::path::PathBuf {
        let script_path = temp_dir.path().join("fake_ffprobe.sh");
        let script = r#"#!/bin/sh
if [ -n "$RMC_FFPROBE_LOG" ]; then
  printf '%s\n' "$@" >> "$RMC_FFPROBE_LOG"
fi
printf '{"format":{"duration":"187.4"}}\n'
"#;
        std::fs::write(&script_path, script).unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
        script_path
    }

    #[tokio::test]
    async fn test_ensure_movie_runtime_seconds_persists_duration() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ffprobe_path = write_fake_ffprobe(&temp_dir);
        let movie_path = temp_dir.path().join("movie.mp4");
        std::fs::write(&movie_path, b"movie").unwrap();

        let _guard = EnvGuard::set(&[(
            "RMC_FFPROBE_CMD",
            ffprobe_path.to_string_lossy().into_owned(),
        )]);

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Movie".to_string(),
            year: Some(2024),
            file_path: movie_path,
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        })
        .await
        .unwrap();

        let movie = ensure_movie_runtime_seconds(&db, 1).await.unwrap();
        assert_eq!(movie.runtime_seconds, Some(187));
        assert_eq!(movie.runtime_minutes, Some(4));
    }

    #[tokio::test]
    async fn test_ensure_movie_runtime_seconds_passes_strm_headers_to_ffprobe() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ffprobe_path = write_fake_ffprobe(&temp_dir);
        let log_path = temp_dir.path().join("ffprobe.log");
        let strm_path = temp_dir.path().join("movie.strm");
        std::fs::write(
            &strm_path,
            "https://example.com/movie.mkv\nUser-Agent: VidHub\nReferer: https://example.com\n",
        )
        .unwrap();

        let _guard = EnvGuard::set(&[
            (
                "RMC_FFPROBE_CMD",
                ffprobe_path.to_string_lossy().into_owned(),
            ),
            ("RMC_FFPROBE_LOG", log_path.to_string_lossy().into_owned()),
        ]);

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Movie".to_string(),
            year: Some(2024),
            file_path: strm_path,
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        })
        .await
        .unwrap();

        let movie = ensure_movie_runtime_seconds(&db, 1).await.unwrap();
        let log = std::fs::read_to_string(log_path).unwrap();

        assert_eq!(movie.runtime_seconds, Some(187));
        assert!(log.contains("-headers"));
        assert!(log.contains("User-Agent: VidHub"));
        assert!(log.contains("Referer: https://example.com"));
        assert!(log.contains("https://example.com/movie.mkv"));
    }
}
