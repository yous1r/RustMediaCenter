pub struct TmdbScraper {
    api_key: String,
}

impl TmdbScraper {
    pub fn new(key: &str) -> Self {
        Self { api_key: key.to_string() }
    }
    
    pub async fn fetch_movie_meta(&self, _title: &str) -> Option<String> {
        // Mock return
        Some("Mock Metadata".to_string())
    }
}
