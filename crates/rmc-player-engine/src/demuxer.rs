pub struct Demuxer {
    file_path: String,
    format: String,
}

impl Demuxer {
    pub fn new(path: &str) -> Self {
        Self {
            file_path: path.to_string(),
            format: "unknown".to_string(),
        }
    }

    pub fn format(&self) -> &str {
        &self.format
    }
}

impl Default for Demuxer {
    fn default() -> Self {
        Self::new("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_demuxer_init() {
        let demuxer = Demuxer::new("test.mkv");
        assert_eq!(demuxer.format(), "unknown");
    }
}
