use notify::{RecommendedWatcher, RecursiveMode, Watcher, Config};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct MediaWatcher {
    directory: String,
    running: AtomicBool,
    watcher: Option<RecommendedWatcher>,
}

impl MediaWatcher {
    pub fn new(directory: String) -> Self {
        Self {
            directory,
            running: AtomicBool::new(false),
            watcher: None,
        }
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
        
        watcher.watch(std::path::Path::new(&self.directory), RecursiveMode::Recursive)?;
        
        self.watcher = Some(watcher);
        self.running.store(true, Ordering::SeqCst);
        
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

pub fn start_watcher() {
    // Stub function for file event monitoring
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_watcher_init() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut watcher = MediaWatcher::new(temp_dir.path().to_string_lossy().to_string());
        assert_eq!(watcher.is_running(), false);
        let res = watcher.start().await;
        assert!(res.is_ok());
        assert_eq!(watcher.is_running(), true);
    }

    #[tokio::test]
    async fn test_watcher_integration() {
        // 创建临时目录
        let temp_dir = tempfile::tempdir().unwrap();
        
        let mut watcher = MediaWatcher::new(temp_dir.path().to_string_lossy().to_string());
        // 尝试启动 watcher
        let res = watcher.start().await;
        assert!(res.is_ok());
        assert!(watcher.is_running());
    }
}

