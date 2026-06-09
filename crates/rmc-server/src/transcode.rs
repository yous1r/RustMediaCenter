use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::process::{Child, Command};

const HEARTBEAT_TIMEOUT_SECS: u64 = 40;
const CLEANUP_INTERVAL_SECS: u64 = 10;

pub enum SessionState {
    Spawning,
    Running(Child),
}

pub struct ActiveSession {
    pub state: SessionState,
    pub last_heartbeat: std::time::Instant,
    pub output_dir: String,
}

#[derive(Clone)]
pub struct TranscodeManager {
    sessions: Arc<Mutex<HashMap<i64, ActiveSession>>>,
    base_temp_dir: String,
}

impl TranscodeManager {
    pub fn new(base_temp_dir: String) -> Self {
        let manager = Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            base_temp_dir,
        };
        
        // 启动后台清理定时器，使用弱引用以在主体销毁后能优雅退出，防范协程/内存泄露
        let sessions_weak = Arc::downgrade(&manager.sessions);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(CLEANUP_INTERVAL_SECS)).await;
                
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
                        if now.saturating_duration_since(session.last_heartbeat).as_secs() > HEARTBEAT_TIMEOUT_SECS {
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
                            let renamed = std::fs::rename(&session.output_dir, &cleanup_dir).is_ok();
                            let actual_cleanup_dir = if renamed { cleanup_dir } else { session.output_dir.clone() };
                            expired_sessions.push((id, session, actual_cleanup_dir));
                        }
                    }
                } // 锁在此处已释放
                
                for (id, session, cleanup_dir) in expired_sessions {
                    if let SessionState::Running(mut child) = session.state {
                        let _ = child.kill().await;
                    }
                    let _ = tokio::fs::remove_dir_all(&cleanup_dir).await;
                    tracing::info!("Killed expired transcode session for movie {}", id);
                }
            }
        });
        
        manager
    }

    pub fn get_m3u8_path(&self, movie_id: i64) -> String {
        format!("{}/{}/master.m3u8", self.base_temp_dir, movie_id)
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
                    SessionState::Running(child) => {
                        match child.try_wait() {
                            Ok(None) => return Some("Running".to_string()),
                            _ => should_cleanup = true,
                        }
                    }
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
        } // 锁在此处已释放
        
        if let Some(session) = session_to_cleanup {
            if let SessionState::Running(mut child) = session.state {
                let _ = child.kill().await;
            }
            let _ = tokio::fs::remove_dir_all(&cleanup_dir).await;
        }
        None
    }

    pub async fn start_transcode_session(&self, movie_id: i64, input_path: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // 校验输入路径存在性，排除了 HTTP/HTTPS 等形式的网络流 (strm 重定向后的内容)
        if !std::path::Path::new(input_path).exists() && !input_path.starts_with("http") {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Input path not found: {}", input_path),
            )));
        }

        // 占位逻辑：快速加锁检查，如果不存在则以 Spawning 状态插入占位，然后释放锁
        {
            let mut guard = self.sessions.lock().await;
            if guard.contains_key(&movie_id) {
                return Ok(());
            }
            let output_dir = format!("{}/{}", self.base_temp_dir, movie_id);
            guard.insert(movie_id, ActiveSession {
                state: SessionState::Spawning,
                last_heartbeat: std::time::Instant::now(),
                output_dir,
            });
        }

        let output_dir = format!("{}/{}", self.base_temp_dir, movie_id);
        tokio::fs::create_dir_all(&output_dir).await?;
        let m3u8_path = format!("{}/master.m3u8", output_dir);
        let ffmpeg_cmd = std::env::var("RMC_FFMPEG_CMD").unwrap_or_else(|_| "ffmpeg".to_string());

        // 默认采用 VA-API 方案在 Linux 上硬解转码
        let mut child = Command::new(&ffmpeg_cmd)
            .args(&[
                "-y",
                "-hwaccel", "vaapi",
                "-hwaccel_device", "/dev/dri/renderD128",
                "-hwaccel_output_format", "vaapi",
                "-i", input_path,
                "-c:v", "h264_vaapi",
                "-b:v", "3M",
                "-maxrate", "4M",
                "-bufsize", "6M",
                "-c:a", "aac",
                "-b:a", "128k",
                "-f", "hls",
                "-hls_time", "6",
                "-hls_list_size", "0",
                "-hls_segment_filename", &format!("{}/seq-%d.ts", output_dir),
                &m3u8_path
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        let use_fallback = match &mut child {
            Err(_) => true,
            Ok(c) => {
                let mut fallback = false;
                // Poll for up to 1.5 seconds to see if the process exits early or succeeds in creating the file
                for _ in 0..15 {
                    match c.try_wait() {
                        Ok(Some(status)) => {
                            if !status.success() {
                                fallback = true;
                            }
                            break;
                        }
                        Ok(None) => {
                            if std::path::Path::new(&m3u8_path).exists() {
                                break;
                            }
                        }
                        Err(_) => {
                            fallback = true;
                            break;
                        }
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
                fallback
            }
        };

        let final_child = if use_fallback {
            tracing::warn!("VA-API transcode failed to start or exited immediately. Falling back to CPU (libx264) software transcoding.");
            Command::new(&ffmpeg_cmd)
                .args(&[
                    "-y",
                    "-i", input_path,
                    "-c:v", "libx264",
                    "-preset", "veryfast",
                    "-b:v", "2M",
                    "-c:a", "aac",
                    "-b:a", "128k",
                    "-f", "hls",
                    "-hls_time", "6",
                    "-hls_list_size", "0",
                    "-hls_segment_filename", &format!("{}/seq-%d.ts", output_dir),
                    &m3u8_path
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
        } else {
            child
        };

        match final_child {
            Ok(c) => {
                // 成功启动，更新占位状态为 Running(child)
                let mut guard = self.sessions.lock().await;
                if let Some(session) = guard.get_mut(&movie_id) {
                    session.state = SessionState::Running(c);
                    session.last_heartbeat = std::time::Instant::now();
                } else {
                    // 若中途被清理，则在此处进行二次防御清理
                    let mut child_to_kill = c;
                    let _ = child_to_kill.kill().await;
                    let millis = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let cleanup_dir = format!("{}-cleanup-{}", output_dir, millis);
                    if std::fs::rename(&output_dir, &cleanup_dir).is_ok() {
                        let _ = tokio::fs::remove_dir_all(&cleanup_dir).await;
                    } else {
                        let _ = tokio::fs::remove_dir_all(&output_dir).await;
                    }
                }
                Ok(())
            }
            Err(e) => {
                // 彻底失败，移除占位，锁内重命名原子隔离后，锁外删除临时输出目录
                let cleanup_dir = {
                    let mut guard = self.sessions.lock().await;
                    guard.remove(&movie_id);
                    let millis = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let cleanup_dir = format!("{}-cleanup-{}", output_dir, millis);
                    if std::fs::rename(&output_dir, &cleanup_dir).is_ok() {
                        cleanup_dir
                    } else {
                        output_dir
                    }
                };
                let _ = tokio::fs::remove_dir_all(&cleanup_dir).await;
                Err(e.into())
            }
        }
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

    #[tokio::test]
    async fn test_transcode_session_lifecycle() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir_fixture = tempfile::tempdir().unwrap();
        let transcode_dir = temp_dir_fixture.path();

        // 创建一个 mock 脚本，忽略所有参数并在生成 m3u8 后无限挂起，模拟正在转码的 ffmpeg 进程
        let mock_script = transcode_dir.join("mock_ffmpeg.sh");
        std::fs::write(&mock_script, "#!/bin/sh\nfor arg; do true; done\ntouch \"$arg\"\nsleep 100\n").unwrap();
        let mut perms = std::fs::metadata(&mock_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_script, perms).unwrap();

        std::env::set_var("RMC_FFMPEG_CMD", mock_script.to_string_lossy().to_string());
        
        let manager = TranscodeManager::new(transcode_dir.to_string_lossy().to_string());
        
        // 创建一个不存在的输入，但由于是 http 格式可以跳过存在性校验以进入 mock 转码
        let res = manager.start_transcode_session(999, "http://invalid/path.mp4").await;
        res.expect("Failed to start transcode session");
        
        // 验证任务记录存在
        let status = manager.get_session_status(999).await;
        assert!(status.is_some());
        
        // 强行关闭任务
        manager.stop_transcode_session(999).await;
        let status2 = manager.get_session_status(999).await;
        assert!(status2.is_none());
        
        std::env::remove_var("RMC_FFMPEG_CMD");
    }
}
