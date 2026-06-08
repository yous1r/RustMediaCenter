pub struct MediaWatcher {
    directory: String,
    watching: bool,
}

impl MediaWatcher {
    pub fn new(dir: &str) -> Self {
        Self {
            directory: dir.to_string(),
            watching: false,
        }
    }

    pub fn start(&mut self) {
        self.watching = true;
        // 实际应用中这里将初始化 notify::RecommendedWatcher
    }

    pub fn is_watching(&self) -> bool {
        self.watching
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watcher_init() {
        let mut watcher = MediaWatcher::new("/tmp/mock_media_dir");
        assert_eq!(watcher.is_watching(), false);
        watcher.start();
        assert_eq!(watcher.is_watching(), true);
    }
}
