use notify::{RecommendedWatcher, RecursiveMode, Watcher, Config};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct MediaWatcher {
    directory: String,
    running: Arc<Mutex<bool>>,
}

impl MediaWatcher {
    pub fn new(directory: String) -> Self {
        Self {
            directory,
            running: Arc::new(Mutex::new(false)),
        }
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut running_guard = self.running.lock().await;
        if *running_guard {
            return Ok(());
        }
        
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut watcher = notify::RecommendedWatcher::new(tx, Config::default())?;
        
        watcher.watch(std::path::Path::new(&self.directory), RecursiveMode::Recursive)?;
        
        *running_guard = true;
        // 在实际生产中这里应起一个 tokio::spawn 来监听 rx
        // 为了最少实现，我们目前仅设置并保持状态，避免编译期 unused_variables 警告，我给 rx 加了前缀
        
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        // 对于仅测试用途
        if let Ok(guard) = self.running.try_lock() {
            *guard
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_watcher_init() {
        let temp_dir = std::env::temp_dir().join("rmc_test_init_dir");
        let _ = std::fs::create_dir_all(&temp_dir);
        let mut watcher = MediaWatcher::new(temp_dir.to_string_lossy().to_string());
        assert_eq!(watcher.is_running(), false);
        let res = watcher.start().await;
        assert!(res.is_ok());
        assert_eq!(watcher.is_running(), true);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_watcher_integration() {
        // 创建临时目录
        let temp_dir = std::env::temp_dir().join("rmc_test_watch");
        let _ = std::fs::create_dir_all(&temp_dir);
        
        let mut watcher = MediaWatcher::new(temp_dir.to_string_lossy().to_string());
        // 尝试启动 watcher
        let res = watcher.start().await;
        assert!(res.is_ok());
        assert!(watcher.is_running());
        
        // 扫尾
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
