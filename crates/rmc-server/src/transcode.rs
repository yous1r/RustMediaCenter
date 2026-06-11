use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::Mutex;

const DEFAULT_HEARTBEAT_TIMEOUT_SECS: u64 = 180;
const DEFAULT_CLEANUP_INTERVAL_SECS: u64 = 10;
const STARTUP_PROBE_RETRIES: usize = 20;
const STARTUP_PROBE_DELAY_MS: u64 = 100;
const DEFAULT_HLS_TIME_SECS: u64 = 6;
const DEFAULT_STREAM_FRAGMENT_SECS: u64 = 4;
const DEFAULT_RENDER_DEVICE: &str = "/dev/dri/renderD128";
const DEFAULT_MEDIASRV_LIB_PATH: &str = "/usr/trim/lib/mediasrv";
const DEFAULT_LIBVA_DRIVER_NAME: &str = "iHD";
const FFMPEG_LOG_NAME: &str = "ffmpeg.log";

static STREAM_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

pub enum SessionState {
    Spawning,
    Running(Child),
}

pub struct ActiveSession {
    pub state: SessionState,
    pub last_heartbeat: std::time::Instant,
    pub output_dir: String,
}

pub struct StreamTranscodeSession {
    pub child: Child,
    pub stdout: ChildStdout,
    pub cleanup_dir: PathBuf,
}

#[derive(Clone)]
pub struct TranscodeManager {
    sessions: Arc<Mutex<HashMap<i64, ActiveSession>>>,
    base_temp_dir: String,
    heartbeat_timeout: std::time::Duration,
    cleanup_interval: std::time::Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HardwareAccelMode {
    Auto,
    Qsv,
    Vaapi,
    Software,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TranscodeStrategy {
    Qsv,
    Vaapi,
    Software,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscodeQuality {
    Source,
    P1080,
    P720,
    P480,
    P360,
}

#[derive(Debug)]
struct TranscodeRuntimeConfig {
    ffmpeg_cmd: String,
    mode: HardwareAccelMode,
    render_device: String,
    libva_driver_name: Option<String>,
    libva_drivers_path: Option<String>,
    ld_library_path: Option<String>,
    hls_time_secs: u64,
    stream_fragment_secs: u64,
}

impl TranscodeQuality {
    pub fn from_label(raw: Option<&str>) -> Self {
        match raw.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
            "1080" | "1080p" | "fhd" => Self::P1080,
            "720" | "720p" | "hd" => Self::P720,
            "480" | "480p" | "sd" => Self::P480,
            "360" | "360p" | "low" => Self::P360,
            _ => Self::Source,
        }
    }

    pub fn as_label(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::P1080 => "1080p",
            Self::P720 => "720p",
            Self::P480 => "480p",
            Self::P360 => "360p",
        }
    }

    fn max_height(self) -> Option<u16> {
        match self {
            Self::Source => None,
            Self::P1080 => Some(1080),
            Self::P720 => Some(720),
            Self::P480 => Some(480),
            Self::P360 => Some(360),
        }
    }

    fn video_bitrate(self, strategy: TranscodeStrategy) -> &'static str {
        match self {
            Self::Source if matches!(strategy, TranscodeStrategy::Software) => "2M",
            Self::Source => "3M",
            Self::P1080 => "4M",
            Self::P720 => "2500k",
            Self::P480 => "1200k",
            Self::P360 => "700k",
        }
    }

    fn maxrate(self) -> &'static str {
        match self {
            Self::Source => "4M",
            Self::P1080 => "5M",
            Self::P720 => "3M",
            Self::P480 => "1600k",
            Self::P360 => "900k",
        }
    }

    fn bufsize(self) -> &'static str {
        match self {
            Self::Source => "6M",
            Self::P1080 => "8M",
            Self::P720 => "5M",
            Self::P480 => "2400k",
            Self::P360 => "1400k",
        }
    }

    fn scale_filter(self) -> Option<String> {
        self.max_height()
            .map(|height| format!("scale=-2:min({height}\\,ih)"))
    }

    fn qsv_filter(self) -> String {
        match self.scale_filter() {
            Some(scale) => format!("{scale},format=nv12,hwupload=extra_hw_frames=64"),
            None => "format=nv12,hwupload=extra_hw_frames=64".to_string(),
        }
    }

    fn vaapi_filter(self) -> String {
        match self.scale_filter() {
            Some(scale) => format!("{scale},format=nv12,hwupload"),
            None => "format=nv12,hwupload".to_string(),
        }
    }
}

impl HardwareAccelMode {
    fn from_env() -> Self {
        match std::env::var("RMC_TRANSCODE_MODE") {
            Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "auto" => Self::Auto,
                "qsv" => Self::Qsv,
                "vaapi" => Self::Vaapi,
                "software" | "cpu" => Self::Software,
                other => {
                    tracing::warn!(
                        mode = other,
                        "Unknown RMC_TRANSCODE_MODE value, falling back to auto"
                    );
                    Self::Auto
                }
            },
            Err(_) => Self::Auto,
        }
    }

    fn strategies(self) -> &'static [TranscodeStrategy] {
        match self {
            Self::Auto => &[
                TranscodeStrategy::Qsv,
                TranscodeStrategy::Vaapi,
                TranscodeStrategy::Software,
            ],
            Self::Qsv => &[TranscodeStrategy::Qsv],
            Self::Vaapi => &[TranscodeStrategy::Vaapi],
            Self::Software => &[TranscodeStrategy::Software],
        }
    }
}

impl TranscodeStrategy {
    fn append_start_time_args(args: &mut Vec<String>, start_time_secs: Option<f64>) {
        if let Some(start_time_secs) =
            start_time_secs.filter(|value| value.is_finite() && *value > 0.0)
        {
            args.extend(["-ss".to_string(), format!("{start_time_secs:.3}")]);
        }
    }

    fn append_input_args(args: &mut Vec<String>, input_path: &str, input_headers: Option<&str>) {
        if let Some(headers) = input_headers.filter(|value| !value.trim().is_empty()) {
            args.extend(["-headers".to_string(), headers.to_string()]);
        }

        args.extend(["-i".to_string(), input_path.to_string()]);
    }

    fn label(self) -> &'static str {
        match self {
            Self::Qsv => "qsv",
            Self::Vaapi => "vaapi",
            Self::Software => "software",
        }
    }

    fn requires_render_device(self) -> bool {
        !matches!(self, Self::Software)
    }

    fn build_args(
        self,
        input_path: &str,
        output_dir: &str,
        m3u8_path: &str,
        render_device: &str,
        hls_time_secs: u64,
        input_headers: Option<&str>,
    ) -> Vec<String> {
        let mut args = Vec::new();
        match self {
            Self::Qsv => {
                args.extend([
                    "-y".to_string(),
                    "-init_hw_device".to_string(),
                    format!("qsv=hw:{}", render_device),
                    "-filter_hw_device".to_string(),
                    "hw".to_string(),
                ]);
                Self::append_input_args(&mut args, input_path, input_headers);
                args.extend([
                    "-vf".to_string(),
                    "format=nv12,hwupload=extra_hw_frames=64".to_string(),
                    "-c:v".to_string(),
                    "h264_qsv".to_string(),
                    "-preset".to_string(),
                    "fast".to_string(),
                    "-b:v".to_string(),
                    "3M".to_string(),
                    "-maxrate".to_string(),
                    "4M".to_string(),
                    "-bufsize".to_string(),
                    "6M".to_string(),
                ]);
            }
            Self::Vaapi => {
                args.extend([
                    "-y".to_string(),
                    "-init_hw_device".to_string(),
                    format!("vaapi=va:{}", render_device),
                    "-filter_hw_device".to_string(),
                    "va".to_string(),
                ]);
                Self::append_input_args(&mut args, input_path, input_headers);
                args.extend([
                    "-vf".to_string(),
                    "format=nv12,hwupload".to_string(),
                    "-c:v".to_string(),
                    "h264_vaapi".to_string(),
                    "-b:v".to_string(),
                    "3M".to_string(),
                    "-maxrate".to_string(),
                    "4M".to_string(),
                    "-bufsize".to_string(),
                    "6M".to_string(),
                ]);
            }
            Self::Software => {
                args.push("-y".to_string());
                Self::append_input_args(&mut args, input_path, input_headers);
                args.extend([
                    "-c:v".to_string(),
                    "libx264".to_string(),
                    "-preset".to_string(),
                    "veryfast".to_string(),
                    "-b:v".to_string(),
                    "2M".to_string(),
                ]);
            }
        }

        args.extend([
            "-force_key_frames".to_string(),
            format!("expr:gte(t,n_forced*{})", hls_time_secs),
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            "128k".to_string(),
            "-f".to_string(),
            "hls".to_string(),
            "-hls_time".to_string(),
            hls_time_secs.to_string(),
            "-hls_list_size".to_string(),
            "0".to_string(),
            "-hls_flags".to_string(),
            "independent_segments".to_string(),
            "-hls_segment_filename".to_string(),
            format!("{}/seq-%d.ts", output_dir),
            m3u8_path.to_string(),
        ]);

        args
    }

    fn build_stream_args(
        self,
        input_path: &str,
        render_device: &str,
        fragment_secs: u64,
        input_headers: Option<&str>,
        start_time_secs: Option<f64>,
        quality: TranscodeQuality,
    ) -> Vec<String> {
        let mut args = Vec::new();

        match self {
            Self::Qsv => {
                args.extend([
                    "-y".to_string(),
                    "-init_hw_device".to_string(),
                    format!("qsv=hw:{}", render_device),
                    "-filter_hw_device".to_string(),
                    "hw".to_string(),
                ]);
                Self::append_start_time_args(&mut args, start_time_secs);
                Self::append_input_args(&mut args, input_path, input_headers);
                args.extend([
                    "-vf".to_string(),
                    quality.qsv_filter(),
                    "-c:v".to_string(),
                    "h264_qsv".to_string(),
                    "-preset".to_string(),
                    "fast".to_string(),
                    "-b:v".to_string(),
                    quality.video_bitrate(self).to_string(),
                    "-maxrate".to_string(),
                    quality.maxrate().to_string(),
                    "-bufsize".to_string(),
                    quality.bufsize().to_string(),
                ]);
            }
            Self::Vaapi => {
                args.extend([
                    "-y".to_string(),
                    "-init_hw_device".to_string(),
                    format!("vaapi=va:{}", render_device),
                    "-filter_hw_device".to_string(),
                    "va".to_string(),
                ]);
                Self::append_start_time_args(&mut args, start_time_secs);
                Self::append_input_args(&mut args, input_path, input_headers);
                args.extend([
                    "-vf".to_string(),
                    quality.vaapi_filter(),
                    "-c:v".to_string(),
                    "h264_vaapi".to_string(),
                    "-b:v".to_string(),
                    quality.video_bitrate(self).to_string(),
                    "-maxrate".to_string(),
                    quality.maxrate().to_string(),
                    "-bufsize".to_string(),
                    quality.bufsize().to_string(),
                ]);
            }
            Self::Software => {
                args.push("-y".to_string());
                Self::append_start_time_args(&mut args, start_time_secs);
                Self::append_input_args(&mut args, input_path, input_headers);
                if let Some(scale_filter) = quality.scale_filter() {
                    args.extend(["-vf".to_string(), scale_filter]);
                }
                args.extend([
                    "-c:v".to_string(),
                    "libx264".to_string(),
                    "-preset".to_string(),
                    "veryfast".to_string(),
                    "-b:v".to_string(),
                    quality.video_bitrate(self).to_string(),
                ]);
            }
        }

        args.extend([
            "-force_key_frames".to_string(),
            format!("expr:gte(t,n_forced*{})", fragment_secs),
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            "128k".to_string(),
            "-movflags".to_string(),
            "frag_keyframe+empty_moov+default_base_moof".to_string(),
            "-frag_duration".to_string(),
            (fragment_secs * 1_000_000).to_string(),
            "-f".to_string(),
            "mp4".to_string(),
            "pipe:1".to_string(),
        ]);

        args
    }
}

impl TranscodeRuntimeConfig {
    fn from_env() -> Self {
        let default_lib_path = Path::new(DEFAULT_MEDIASRV_LIB_PATH)
            .exists()
            .then(|| DEFAULT_MEDIASRV_LIB_PATH.to_string());

        let libva_driver_name = std::env::var("RMC_LIBVA_DRIVER_NAME")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                default_lib_path
                    .as_ref()
                    .map(|_| DEFAULT_LIBVA_DRIVER_NAME.to_string())
            });

        let libva_drivers_path = std::env::var("RMC_LIBVA_DRIVERS_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                default_lib_path
                    .as_deref()
                    .map(|value| prepend_env_path(value, "LIBVA_DRIVERS_PATH"))
            });

        let ld_library_path = std::env::var("RMC_LD_LIBRARY_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                default_lib_path
                    .as_deref()
                    .map(|value| prepend_env_path(value, "LD_LIBRARY_PATH"))
            });

        Self {
            ffmpeg_cmd: std::env::var("RMC_FFMPEG_CMD").unwrap_or_else(|_| "ffmpeg".to_string()),
            mode: HardwareAccelMode::from_env(),
            render_device: std::env::var("RMC_DRI_RENDER_DEVICE")
                .unwrap_or_else(|_| DEFAULT_RENDER_DEVICE.to_string()),
            libva_driver_name,
            libva_drivers_path,
            ld_library_path,
            hls_time_secs: read_u64_env("RMC_HLS_TIME_SECS", DEFAULT_HLS_TIME_SECS),
            stream_fragment_secs: read_u64_env(
                "RMC_STREAM_FRAGMENT_SECS",
                DEFAULT_STREAM_FRAGMENT_SECS,
            ),
        }
    }

    fn apply_env(&self, command: &mut Command) {
        if let Some(value) = &self.libva_driver_name {
            command.env("LIBVA_DRIVER_NAME", value);
        }
        if let Some(value) = &self.libva_drivers_path {
            command.env("LIBVA_DRIVERS_PATH", value);
        }
        if let Some(value) = &self.ld_library_path {
            command.env("LD_LIBRARY_PATH", value);
        }
    }
}

fn read_u64_env(env_key: &str, default_value: u64) -> u64 {
    match std::env::var(env_key) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(value) if value > 0 => value,
            Ok(_) | Err(_) => {
                tracing::warn!(
                    env_key,
                    value = raw,
                    default_value,
                    "Invalid numeric env override, using default"
                );
                default_value
            }
        },
        Err(_) => default_value,
    }
}

fn prepend_env_path(prefix: &str, env_key: &str) -> String {
    match std::env::var(env_key) {
        Ok(existing) => {
            let trimmed = existing.trim();
            if trimmed.is_empty() {
                prefix.to_string()
            } else if trimmed.split(':').any(|segment| segment == prefix) {
                trimmed.to_string()
            } else {
                format!("{}:{}", prefix, trimmed)
            }
        }
        Err(_) => prefix.to_string(),
    }
}

fn open_ffmpeg_log(path: &Path) -> Result<std::fs::File, std::io::Error> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn append_log_line(path: &Path, line: impl AsRef<str>) {
    if let Ok(mut file) = open_ffmpeg_log(path) {
        let _ = writeln!(file, "{}", line.as_ref());
    }
}

fn format_status(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "signal".to_string(),
    }
}

fn next_stream_session_suffix() -> u128 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let counter = STREAM_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    (millis << 16) | (counter & 0xffff)
}

async fn probe_startup(child: &mut Child, m3u8_path: &Path) -> Result<(), String> {
    for _ in 0..STARTUP_PROBE_RETRIES {
        match child.try_wait() {
            Ok(Some(status)) => {
                if m3u8_path.exists() {
                    return Ok(());
                }
                return Err(format!(
                    "ffmpeg exited before producing playlist (status={})",
                    format_status(status)
                ));
            }
            Ok(None) => {
                if m3u8_path.exists() {
                    return Ok(());
                }
            }
            Err(err) => {
                return Err(format!("failed to poll ffmpeg process: {}", err));
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(STARTUP_PROBE_DELAY_MS)).await;
    }

    // 某些源在建立滤镜链、探测字幕或写首个切片前不会立刻产出 master.m3u8，
    // 只要进程仍然存活，就视为启动成功，后续由上层 HLS 轮询继续等待清单文件出现。
    Ok(())
}

async fn probe_pipe_startup(child: &mut Child) -> Result<(), String> {
    for _ in 0..STARTUP_PROBE_RETRIES {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!(
                    "ffmpeg exited before producing stream output (status={})",
                    format_status(status)
                ));
            }
            Ok(None) => {
                tokio::time::sleep(tokio::time::Duration::from_millis(STARTUP_PROBE_DELAY_MS))
                    .await;
            }
            Err(err) => {
                return Err(format!("failed to poll ffmpeg process: {}", err));
            }
        }
    }

    Ok(())
}

impl TranscodeManager {
    pub fn new(base_temp_dir: String) -> Self {
        let cleanup_interval_secs = read_u64_env(
            "RMC_TRANSCODE_CLEANUP_INTERVAL_SECS",
            DEFAULT_CLEANUP_INTERVAL_SECS,
        );
        let heartbeat_timeout_secs = read_u64_env(
            "RMC_TRANSCODE_HEARTBEAT_TIMEOUT_SECS",
            DEFAULT_HEARTBEAT_TIMEOUT_SECS,
        )
        .max(cleanup_interval_secs.saturating_mul(2));

        let manager = Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            base_temp_dir,
            heartbeat_timeout: std::time::Duration::from_secs(heartbeat_timeout_secs),
            cleanup_interval: std::time::Duration::from_secs(cleanup_interval_secs),
        };

        tracing::info!(
            heartbeat_timeout_secs,
            cleanup_interval_secs,
            "Configured transcode session cleanup policy"
        );

        // 启动后台清理定时器，使用弱引用以在主体销毁后能优雅退出，防范协程/内存泄露
        let sessions_weak = Arc::downgrade(&manager.sessions);
        let cleanup_interval = manager.cleanup_interval;
        let heartbeat_timeout = manager.heartbeat_timeout;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(cleanup_interval).await;

                let sessions_clone = match sessions_weak.upgrade() {
                    Some(s) => s,
                    None => break,
                };

                let mut expired_sessions = Vec::new();
                {
                    let mut guard = sessions_clone.lock().await;
                    let now = std::time::Instant::now();
                    let mut to_remove = Vec::new();

                    for (id, session) in guard.iter() {
                        if now.saturating_duration_since(session.last_heartbeat) > heartbeat_timeout
                        {
                            to_remove.push(*id);
                        }
                    }

                    for id in to_remove {
                        if let Some(session) = guard.remove(&id) {
                            let millis = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis();
                            let cleanup_dir = format!("{}-cleanup-{}", session.output_dir, millis);
                            let renamed =
                                std::fs::rename(&session.output_dir, &cleanup_dir).is_ok();
                            let actual_cleanup_dir = if renamed {
                                cleanup_dir
                            } else {
                                session.output_dir.clone()
                            };
                            expired_sessions.push((id, session, actual_cleanup_dir));
                        }
                    }
                }

                for (id, session, cleanup_dir) in expired_sessions {
                    if let SessionState::Running(mut child) = session.state {
                        let _ = child.kill().await;
                    }
                    let _ = tokio::fs::remove_dir_all(&cleanup_dir).await;
                    tracing::info!(movie_id = id, "Killed expired transcode session");
                }
            }
        });

        manager
    }

    pub fn get_m3u8_path(&self, movie_id: i64) -> String {
        format!("{}/{}/master.m3u8", self.base_temp_dir, movie_id)
    }

    pub fn get_log_path(&self, movie_id: i64) -> String {
        format!("{}/{}/{}", self.base_temp_dir, movie_id, FFMPEG_LOG_NAME)
    }

    pub async fn touch_session(&self, movie_id: i64) {
        let mut guard = self.sessions.lock().await;
        if let Some(session) = guard.get_mut(&movie_id) {
            session.last_heartbeat = std::time::Instant::now();
        }
    }

    pub async fn get_session_status(&self, movie_id: i64) -> Option<String> {
        let mut session_to_cleanup = None;
        let mut cleanup_dir = String::new();

        {
            let mut guard = self.sessions.lock().await;
            let mut should_cleanup = false;

            if let Some(session) = guard.get_mut(&movie_id) {
                match &mut session.state {
                    SessionState::Spawning => {
                        return Some("Spawning".to_string());
                    }
                    SessionState::Running(child) => match child.try_wait() {
                        Ok(None) => return Some("Running".to_string()),
                        _ => should_cleanup = true,
                    },
                }
            }

            if should_cleanup {
                if let Some(removed) = guard.remove(&movie_id) {
                    let millis = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    cleanup_dir = format!("{}-cleanup-{}", removed.output_dir, millis);
                    let renamed = std::fs::rename(&removed.output_dir, &cleanup_dir).is_ok();
                    if !renamed {
                        cleanup_dir = removed.output_dir.clone();
                    }
                    session_to_cleanup = Some(removed);
                }
            }
        }

        if let Some(session) = session_to_cleanup {
            if let SessionState::Running(mut child) = session.state {
                let _ = child.kill().await;
            }
            let _ = tokio::fs::remove_dir_all(&cleanup_dir).await;
        }
        None
    }

    pub async fn start_transcode_session(
        &self,
        movie_id: i64,
        input_path: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.start_transcode_session_with_headers(movie_id, input_path, None)
            .await
    }

    pub async fn start_transcode_session_with_headers(
        &self,
        movie_id: i64,
        input_path: &str,
        input_headers: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !Path::new(input_path).exists() && !input_path.starts_with("http") {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Input path not found: {}", input_path),
            )));
        }

        {
            let mut guard = self.sessions.lock().await;
            if guard.contains_key(&movie_id) {
                return Ok(());
            }
            let output_dir = format!("{}/{}", self.base_temp_dir, movie_id);
            guard.insert(
                movie_id,
                ActiveSession {
                    state: SessionState::Spawning,
                    last_heartbeat: std::time::Instant::now(),
                    output_dir,
                },
            );
        }

        let output_dir = format!("{}/{}", self.base_temp_dir, movie_id);
        if Path::new(&output_dir).exists() {
            let _ = tokio::fs::remove_dir_all(&output_dir).await;
        }
        tokio::fs::create_dir_all(&output_dir).await?;

        let m3u8_path = format!("{}/master.m3u8", output_dir);
        let m3u8_path_buf = PathBuf::from(&m3u8_path);
        let log_path = PathBuf::from(&output_dir).join(FFMPEG_LOG_NAME);
        let runtime = TranscodeRuntimeConfig::from_env();

        append_log_line(
            &log_path,
            format!(
                "session movie_id={} mode={:?} render_device={} ffmpeg_cmd={}",
                movie_id, runtime.mode, runtime.render_device, runtime.ffmpeg_cmd
            ),
        );
        if let Some(value) = &runtime.libva_driver_name {
            append_log_line(&log_path, format!("LIBVA_DRIVER_NAME={}", value));
        }
        if let Some(value) = &runtime.libva_drivers_path {
            append_log_line(&log_path, format!("LIBVA_DRIVERS_PATH={}", value));
        }
        if let Some(value) = &runtime.ld_library_path {
            append_log_line(&log_path, format!("LD_LIBRARY_PATH={}", value));
        }
        append_log_line(
            &log_path,
            format!("RMC_HLS_TIME_SECS={}", runtime.hls_time_secs),
        );

        let mut final_child = None;
        let mut last_error = String::new();

        for strategy in runtime.mode.strategies() {
            let strategy = *strategy;
            let strategy_name = strategy.label();

            if strategy.requires_render_device() && !Path::new(&runtime.render_device).exists() {
                last_error = format!(
                    "render device {} is not available for {}",
                    runtime.render_device, strategy_name
                );
                append_log_line(
                    &log_path,
                    format!("skip strategy={} reason={}", strategy_name, last_error),
                );
                continue;
            }

            let args = strategy.build_args(
                input_path,
                &output_dir,
                &m3u8_path,
                &runtime.render_device,
                runtime.hls_time_secs,
                input_headers,
            );
            append_log_line(
                &log_path,
                format!(
                    "spawn strategy={} command={} {}",
                    strategy_name,
                    runtime.ffmpeg_cmd,
                    args.join(" ")
                ),
            );

            let mut log_file = open_ffmpeg_log(&log_path)?;
            writeln!(log_file, "--- strategy={} ---", strategy_name)?;
            log_file.flush()?;
            let stdout_file = log_file.try_clone()?;
            let stderr_file = log_file.try_clone()?;

            let mut command = Command::new(&runtime.ffmpeg_cmd);
            runtime.apply_env(&mut command);
            command
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout_file))
                .stderr(Stdio::from(stderr_file));

            match command.spawn() {
                Ok(mut child) => match probe_startup(&mut child, &m3u8_path_buf).await {
                    Ok(()) => {
                        tracing::info!(
                            movie_id,
                            strategy = strategy_name,
                            mode = ?runtime.mode,
                            log = %log_path.display(),
                            "Started ffmpeg transcode session"
                        );
                        final_child = Some(child);
                        break;
                    }
                    Err(err) => {
                        let _ = child.kill().await;
                        last_error = err;
                        append_log_line(
                            &log_path,
                            format!("strategy={} failed: {}", strategy_name, last_error),
                        );
                    }
                },
                Err(err) => {
                    last_error = format!("failed to spawn ffmpeg for {}: {}", strategy_name, err);
                    append_log_line(
                        &log_path,
                        format!("strategy={} spawn_error={}", strategy_name, err),
                    );
                }
            }
        }

        match final_child {
            Some(child) => {
                let mut guard = self.sessions.lock().await;
                if let Some(session) = guard.get_mut(&movie_id) {
                    session.state = SessionState::Running(child);
                    session.last_heartbeat = std::time::Instant::now();
                }
                Ok(())
            }
            None => {
                let mut guard = self.sessions.lock().await;
                guard.remove(&movie_id);
                let error_message = format!(
                    "Failed to start transcoding in {:?} mode. Last error: {}. See {}",
                    runtime.mode,
                    if last_error.is_empty() {
                        "no ffmpeg attempt was started"
                    } else {
                        &last_error
                    },
                    log_path.display()
                );
                Err(std::io::Error::other(error_message).into())
            }
        }
    }

    pub async fn start_stream_transcode(
        &self,
        movie_id: i64,
        input_path: &str,
        input_headers: Option<&str>,
        start_time_secs: Option<f64>,
        quality: TranscodeQuality,
    ) -> Result<StreamTranscodeSession, Box<dyn std::error::Error + Send + Sync>> {
        if !Path::new(input_path).exists() && !input_path.starts_with("http") {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Input path not found: {}", input_path),
            )));
        }

        let output_dir = format!(
            "{}/stream-{}-{}",
            self.base_temp_dir,
            movie_id,
            next_stream_session_suffix()
        );
        tokio::fs::create_dir_all(&output_dir).await?;

        let cleanup_dir = PathBuf::from(&output_dir);
        let log_path = cleanup_dir.join(FFMPEG_LOG_NAME);
        let runtime = TranscodeRuntimeConfig::from_env();

        append_log_line(
            &log_path,
            format!(
                "stream movie_id={} mode={:?} render_device={} ffmpeg_cmd={}",
                movie_id, runtime.mode, runtime.render_device, runtime.ffmpeg_cmd
            ),
        );
        append_log_line(&log_path, format!("quality={}", quality.as_label()));

        let mut last_error = String::new();

        for strategy in runtime.mode.strategies() {
            let strategy = *strategy;
            let strategy_name = strategy.label();

            if strategy.requires_render_device() && !Path::new(&runtime.render_device).exists() {
                last_error = format!(
                    "render device {} is not available for {}",
                    runtime.render_device, strategy_name
                );
                append_log_line(
                    &log_path,
                    format!("skip strategy={} reason={}", strategy_name, last_error),
                );
                continue;
            }

            let args = strategy.build_stream_args(
                input_path,
                &runtime.render_device,
                runtime.stream_fragment_secs,
                input_headers,
                start_time_secs,
                quality,
            );
            append_log_line(
                &log_path,
                format!(
                    "spawn stream strategy={} command={} {}",
                    strategy_name,
                    runtime.ffmpeg_cmd,
                    args.join(" ")
                ),
            );

            let mut log_file = open_ffmpeg_log(&log_path)?;
            writeln!(log_file, "--- stream strategy={} ---", strategy_name)?;
            log_file.flush()?;
            let stderr_file = log_file.try_clone()?;

            let mut command = Command::new(&runtime.ffmpeg_cmd);
            runtime.apply_env(&mut command);
            command
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::from(stderr_file));

            match command.spawn() {
                Ok(mut child) => {
                    let stdout = match child.stdout.take() {
                        Some(stdout) => stdout,
                        None => {
                            let _ = child.kill().await;
                            last_error = "ffmpeg stdout pipe is unavailable".to_string();
                            append_log_line(
                                &log_path,
                                format!("strategy={} failed: {}", strategy_name, last_error),
                            );
                            continue;
                        }
                    };

                    match probe_pipe_startup(&mut child).await {
                        Ok(()) => {
                            tracing::info!(
                                movie_id,
                                strategy = strategy_name,
                                mode = ?runtime.mode,
                                "Started streaming ffmpeg transcode session"
                            );
                            return Ok(StreamTranscodeSession {
                                child,
                                stdout,
                                cleanup_dir,
                            });
                        }
                        Err(err) => {
                            let _ = child.kill().await;
                            last_error = err;
                            append_log_line(
                                &log_path,
                                format!("strategy={} failed: {}", strategy_name, last_error),
                            );
                        }
                    }
                }
                Err(err) => {
                    last_error = format!("failed to spawn ffmpeg for {}: {}", strategy_name, err);
                    append_log_line(
                        &log_path,
                        format!("strategy={} spawn_error={}", strategy_name, err),
                    );
                }
            }
        }

        let _ = tokio::fs::remove_dir_all(&cleanup_dir).await;
        Err(std::io::Error::other(format!(
            "Failed to start streaming transcode in {:?} mode. Last error: {}",
            runtime.mode,
            if last_error.is_empty() {
                "no ffmpeg attempt was started"
            } else {
                &last_error
            }
        ))
        .into())
    }

    pub async fn stop_transcode_session(&self, movie_id: i64) {
        let mut cleanup_dir = String::new();
        let session = {
            let mut guard = self.sessions.lock().await;
            if let Some(removed) = guard.remove(&movie_id) {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                cleanup_dir = format!("{}-cleanup-{}", removed.output_dir, millis);
                let renamed = std::fs::rename(&removed.output_dir, &cleanup_dir).is_ok();
                if !renamed {
                    cleanup_dir = removed.output_dir.clone();
                }
                Some(removed)
            } else {
                None
            }
        };

        if let Some(s) = session {
            if let SessionState::Running(mut child) = s.state {
                let _ = child.kill().await;
            }
            let _ = tokio::fs::remove_dir_all(&cleanup_dir).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::OnceLock;

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static ENV_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    #[tokio::test]
    async fn test_runtime_config_respects_explicit_overrides() {
        let _guard = env_lock().lock().await;

        std::env::set_var("RMC_TRANSCODE_MODE", "vaapi");
        std::env::set_var("RMC_DRI_RENDER_DEVICE", "/dev/dri/custom-render");
        std::env::set_var("RMC_LIBVA_DRIVER_NAME", "custom");
        std::env::set_var("RMC_LIBVA_DRIVERS_PATH", "/custom/libva");
        std::env::set_var("RMC_LD_LIBRARY_PATH", "/custom/ld");
        std::env::set_var("RMC_FFMPEG_CMD", "/custom/ffmpeg");

        let runtime = TranscodeRuntimeConfig::from_env();

        assert_eq!(runtime.mode, HardwareAccelMode::Vaapi);
        assert_eq!(runtime.render_device, "/dev/dri/custom-render");
        assert_eq!(runtime.libva_driver_name.as_deref(), Some("custom"));
        assert_eq!(runtime.libva_drivers_path.as_deref(), Some("/custom/libva"));
        assert_eq!(runtime.ld_library_path.as_deref(), Some("/custom/ld"));
        assert_eq!(runtime.ffmpeg_cmd, "/custom/ffmpeg");

        std::env::remove_var("RMC_TRANSCODE_MODE");
        std::env::remove_var("RMC_DRI_RENDER_DEVICE");
        std::env::remove_var("RMC_LIBVA_DRIVER_NAME");
        std::env::remove_var("RMC_LIBVA_DRIVERS_PATH");
        std::env::remove_var("RMC_LD_LIBRARY_PATH");
        std::env::remove_var("RMC_FFMPEG_CMD");
    }

    #[tokio::test]
    async fn test_transcode_session_lifecycle() {
        let _guard = env_lock().lock().await;

        let temp_dir_fixture = tempfile::tempdir().unwrap();
        let transcode_dir = temp_dir_fixture.path();
        let mock_script = transcode_dir.join("mock_ffmpeg.sh");
        std::fs::write(
            &mock_script,
            "#!/bin/sh\nfor arg in \"$@\"; do out=\"$arg\"; done\ntouch \"$out\"\nsleep 100\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&mock_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_script, perms).unwrap();

        std::env::set_var("RMC_FFMPEG_CMD", mock_script.to_string_lossy().to_string());
        std::env::set_var("RMC_TRANSCODE_MODE", "software");

        let manager = TranscodeManager::new(transcode_dir.to_string_lossy().to_string());
        manager
            .start_transcode_session(999, "http://invalid/path.mp4")
            .await
            .expect("Failed to start transcode session");

        let status = manager.get_session_status(999).await;
        assert!(status.is_some());

        manager.stop_transcode_session(999).await;
        let status2 = manager.get_session_status(999).await;
        assert!(status2.is_none());

        std::env::remove_var("RMC_FFMPEG_CMD");
        std::env::remove_var("RMC_TRANSCODE_MODE");
    }

    #[tokio::test]
    async fn test_auto_mode_falls_back_and_persists_log() {
        let _guard = env_lock().lock().await;

        let temp_dir_fixture = tempfile::tempdir().unwrap();
        let transcode_dir = temp_dir_fixture.path();
        let mock_script = transcode_dir.join("mock_ffmpeg_fallback.sh");
        let fake_render_device = transcode_dir.join("renderD128");
        std::fs::write(&fake_render_device, "").unwrap();

        std::fs::write(
            &mock_script,
            r#"#!/bin/sh
out=""
software="0"
for arg in "$@"; do
  case "$arg" in
    h264_qsv|h264_vaapi)
      echo "simulated hardware init failure for $arg" >&2
      exit 1
      ;;
    libx264)
      software="1"
      ;;
    */master.m3u8)
      out="$arg"
      ;;
  esac
done

if [ "$software" = "1" ]; then
  echo "software fallback engaged" >&2
  touch "$out"
  sleep 100
fi
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&mock_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_script, perms).unwrap();

        std::env::set_var("RMC_FFMPEG_CMD", mock_script.to_string_lossy().to_string());
        std::env::set_var("RMC_TRANSCODE_MODE", "auto");
        std::env::set_var(
            "RMC_DRI_RENDER_DEVICE",
            fake_render_device.to_string_lossy().to_string(),
        );

        let manager = TranscodeManager::new(transcode_dir.to_string_lossy().to_string());
        manager
            .start_transcode_session(123, "http://invalid/path.mp4")
            .await
            .expect("auto fallback should succeed");

        let log_path = transcode_dir.join("123").join(FFMPEG_LOG_NAME);
        let log_content = std::fs::read_to_string(log_path).unwrap();
        assert!(log_content.contains("strategy=qsv"));
        assert!(log_content.contains("strategy=vaapi"));
        assert!(log_content.contains("strategy=software"));
        assert!(log_content.contains("simulated hardware init failure"));
        assert!(log_content.contains("software fallback engaged"));

        manager.stop_transcode_session(123).await;

        std::env::remove_var("RMC_FFMPEG_CMD");
        std::env::remove_var("RMC_TRANSCODE_MODE");
        std::env::remove_var("RMC_DRI_RENDER_DEVICE");
    }

    #[tokio::test]
    async fn test_probe_startup_tolerates_slow_playlist_creation() {
        let _guard = env_lock().lock().await;

        let temp_dir_fixture = tempfile::tempdir().unwrap();
        let transcode_dir = temp_dir_fixture.path();
        let mock_script = transcode_dir.join("mock_ffmpeg_slow_start.sh");

        std::fs::write(
            &mock_script,
            "#!/bin/sh\nsleep 3\nfor arg in \"$@\"; do out=\"$arg\"; done\ntouch \"$out\"\nsleep 100\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&mock_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_script, perms).unwrap();

        std::env::set_var("RMC_FFMPEG_CMD", mock_script.to_string_lossy().to_string());
        std::env::set_var("RMC_TRANSCODE_MODE", "software");

        let manager = TranscodeManager::new(transcode_dir.to_string_lossy().to_string());
        manager
            .start_transcode_session(456, "http://invalid/path.mp4")
            .await
            .expect("slow startup should still be treated as success while ffmpeg keeps running");

        let status = manager.get_session_status(456).await;
        assert_eq!(status.as_deref(), Some("Running"));

        manager.stop_transcode_session(456).await;

        std::env::remove_var("RMC_FFMPEG_CMD");
        std::env::remove_var("RMC_TRANSCODE_MODE");
    }

    #[tokio::test]
    async fn test_stream_transcode_uses_unique_cleanup_dirs_per_request() {
        let _guard = env_lock().lock().await;

        let temp_dir_fixture = tempfile::tempdir().unwrap();
        let transcode_dir = temp_dir_fixture.path();
        let mock_script = transcode_dir.join("mock_ffmpeg_stream_unique.sh");

        std::fs::write(&mock_script, "#!/bin/sh\nsleep 100\n").unwrap();
        let mut perms = std::fs::metadata(&mock_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_script, perms).unwrap();

        std::env::set_var("RMC_FFMPEG_CMD", mock_script.to_string_lossy().to_string());
        std::env::set_var("RMC_TRANSCODE_MODE", "software");

        let manager = TranscodeManager::new(transcode_dir.to_string_lossy().to_string());
        let session_a = manager
            .start_stream_transcode(
                88,
                "http://invalid/path.mp4",
                None,
                None,
                TranscodeQuality::Source,
            )
            .await
            .expect("first stream session should start");
        let session_b = manager
            .start_stream_transcode(
                88,
                "http://invalid/path.mp4",
                None,
                Some(15.0),
                TranscodeQuality::Source,
            )
            .await
            .expect("second stream session should start");

        assert_ne!(session_a.cleanup_dir, session_b.cleanup_dir);

        let mut child_a = session_a.child;
        let mut child_b = session_b.child;
        let cleanup_dir_a = session_a.cleanup_dir;
        let cleanup_dir_b = session_b.cleanup_dir;

        let _ = child_a.kill().await;
        let _ = child_b.kill().await;
        let _ = tokio::fs::remove_dir_all(cleanup_dir_a).await;
        let _ = tokio::fs::remove_dir_all(cleanup_dir_b).await;

        std::env::remove_var("RMC_FFMPEG_CMD");
        std::env::remove_var("RMC_TRANSCODE_MODE");
    }

    #[test]
    fn test_hardware_args_include_hwupload_filters_for_10bit_sources() {
        let qsv_args = TranscodeStrategy::Qsv.build_args(
            "/input.mkv",
            "/tmp/out",
            "/tmp/out/master.m3u8",
            "/dev/dri/renderD128",
            DEFAULT_HLS_TIME_SECS,
            None,
        );
        let vaapi_args = TranscodeStrategy::Vaapi.build_args(
            "/input.mkv",
            "/tmp/out",
            "/tmp/out/master.m3u8",
            "/dev/dri/renderD128",
            DEFAULT_HLS_TIME_SECS,
            None,
        );

        assert!(qsv_args
            .windows(2)
            .any(|w| w == ["-filter_hw_device", "hw"]));
        assert!(qsv_args
            .windows(2)
            .any(|w| w == ["-vf", "format=nv12,hwupload=extra_hw_frames=64"]));
        assert!(!qsv_args.iter().any(|arg| arg == "-hwaccel"));
        assert!(qsv_args.windows(2).any(|w| w == ["-hls_time", "6"]));
        assert!(qsv_args
            .windows(2)
            .any(|w| w == ["-hls_flags", "independent_segments"]));
        assert!(qsv_args
            .windows(2)
            .any(|w| w == ["-force_key_frames", "expr:gte(t,n_forced*6)"]));

        assert!(vaapi_args
            .windows(2)
            .any(|w| w == ["-filter_hw_device", "va"]));
        assert!(vaapi_args
            .windows(2)
            .any(|w| w == ["-vf", "format=nv12,hwupload"]));
        assert!(!vaapi_args.iter().any(|arg| arg == "-hwaccel"));
        assert!(vaapi_args.windows(2).any(|w| w == ["-hls_time", "6"]));
        assert!(vaapi_args
            .windows(2)
            .any(|w| w == ["-hls_flags", "independent_segments"]));
    }

    #[test]
    fn test_stream_args_include_seek_offset_before_input() {
        let args = TranscodeStrategy::Software.build_stream_args(
            "/input.mkv",
            "/dev/dri/renderD128",
            DEFAULT_STREAM_FRAGMENT_SECS,
            None,
            Some(125.5),
            TranscodeQuality::Source,
        );

        let seek_index = args
            .iter()
            .position(|arg| arg == "-ss")
            .expect("missing -ss");
        let input_index = args.iter().position(|arg| arg == "-i").expect("missing -i");

        assert!(seek_index < input_index);
        assert!(args.windows(2).any(|w| w == ["-ss", "125.500"]));
    }

    #[test]
    fn test_stream_args_apply_quality_scale_and_bitrate() {
        let args = TranscodeStrategy::Software.build_stream_args(
            "/input.mkv",
            "/dev/dri/renderD128",
            DEFAULT_STREAM_FRAGMENT_SECS,
            None,
            None,
            TranscodeQuality::P720,
        );

        assert!(args
            .windows(2)
            .any(|w| w == ["-vf", "scale=-2:min(720\\,ih)"]));
        assert!(args.windows(2).any(|w| w == ["-b:v", "2500k"]));
    }

    #[test]
    fn test_stream_args_keep_source_quality_without_scale() {
        let args = TranscodeStrategy::Software.build_stream_args(
            "/input.mkv",
            "/dev/dri/renderD128",
            DEFAULT_STREAM_FRAGMENT_SECS,
            None,
            None,
            TranscodeQuality::Source,
        );

        assert!(!args.iter().any(|arg| arg.starts_with("scale=")));
        assert!(args.windows(2).any(|w| w == ["-b:v", "2M"]));
    }
}
