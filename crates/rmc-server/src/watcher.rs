use notify::{RecommendedWatcher, RecursiveMode, Watcher, Config};
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::Path;

const SCAN_QUEUE_CAPACITY: usize = 100;
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "mpg", "mpeg", "strm"
];

pub struct MediaWatcher {
    watcher: Option<RecommendedWatcher>,
    db: crate::db::Database,
    running: AtomicBool,
}

fn is_video_file(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        for video_ext in VIDEO_EXTENSIONS {
            if ext.eq_ignore_ascii_case(video_ext) {
                return true;
            }
        }
    }
    false
}

impl MediaWatcher {
    pub fn new(db: crate::db::Database) -> Self {
        Self {
            watcher: None,
            db,
            running: AtomicBool::new(false),
        }
    }

    pub fn start(&mut self, dirs: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
        
        for dir in dirs {
            watcher.watch(Path::new(dir), RecursiveMode::Recursive)?;
        }
        
        let handle = tokio::runtime::Handle::current();
        let (scan_tx, mut scan_rx) = tokio::sync::mpsc::channel::<String>(SCAN_QUEUE_CAPACITY);
        
        // Spawn background task to process scan requests sequentially
        let db_clone = self.db.clone();
        handle.spawn(async move {
            tracing::info!("Background scan consumer started");
            while let Some(dir) = scan_rx.recv().await {
                tracing::debug!("Consumer received scan request for directory: {}", dir);
                let scanner = crate::scanner::MediaScanner::new(db_clone.clone());
                match scanner.scan_directory(&dir).await {
                    Ok(count) => tracing::info!("Scan succeeded, added {} movies", count),
                    Err(e) => tracing::error!("Scan failed for directory {}: {:?}", dir, e),
                }
            }
            tracing::info!("Background scan consumer stopped");
        });
        
        tokio::task::spawn_blocking(move || {
            tracing::info!("Event watcher thread started");
            
            while let Ok(res) = rx.recv() {
                match res {
                    Ok(event) => {
                        tracing::debug!("Event received: {:?}", event);
                        use notify::EventKind;
                        match event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) => {
                                for path in event.paths {
                                    if is_video_file(&path) {
                                        if let Some(parent) = path.parent() {
                                            let parent_str = parent.to_string_lossy().to_string();
                                            tracing::debug!("Valid video file modified: {:?}, queueing parent scan: {}", path, parent_str);
                                            if let Err(e) = scan_tx.try_send(parent_str) {
                                                tracing::warn!("Failed to queue directory scan: {:?}", e);
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        tracing::error!("Watcher event error: {:?}", e);
                    }
                }
            }
            tracing::info!("Event watcher thread stopped");
        });

        self.watcher = Some(watcher);
        self.running.store(true, Ordering::SeqCst);
        
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_watcher_init() {
        let db = crate::db::Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let watcher = MediaWatcher::new(db);
        assert_eq!(watcher.is_running(), false);
    }

    #[tokio::test]
    async fn test_watcher_integration() {
        // 创建临时目录
        let temp_dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        
        let mut watcher = MediaWatcher::new(db);
        // 尝试启动 watcher
        let res = watcher.start(&[temp_dir.path().to_string_lossy().to_string()]);
        assert!(res.is_ok());
        assert!(watcher.is_running());
    }

    #[tokio::test]
    async fn test_watcher_event_trigger() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_db_dir = tempfile::tempdir().unwrap();
        let db_path = temp_db_dir.path().join("test_rmc.db");
        std::fs::File::create(&db_path).unwrap();
        let db_url = format!("sqlite:{}", db_path.to_string_lossy());
        let db = crate::db::Database::new(&db_url).await.unwrap();
        db.init_schema().await.unwrap();

        // 启动 watcher
        let dirs = vec![temp_dir.path().to_string_lossy().to_string()];
        let mut watcher = MediaWatcher::new(db.clone());
        let res = watcher.start(&dirs);
        assert!(res.is_ok());

        // 在临时目录中创建一个视频 file
        let file_path = temp_dir.path().join("Test Movie (2025).mp4");
        std::fs::File::create(&file_path).unwrap();

        // 轮询检查数据库中是否已存入该视频文件（最多等 2 秒）
        let mut found = false;
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if db.movie_exists_by_path(&file_path.to_string_lossy()).await.unwrap() {
                found = true;
                break;
            }
        }

        // drop watcher 以便关闭底层监控信道和后台线程，防止测试挂起
        drop(watcher);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert!(found, "The movie file was not auto-detected and inserted into database");
    }
}
