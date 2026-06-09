use rmc_core::models::Movie;

pub struct TmdbScraper {
    api_key: String,
    client: reqwest::Client,
}

impl TmdbScraper {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }

    pub async fn fetch_movie_metadata(&self, title: &str) -> Result<Movie, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("https://api.themoviedb.org/3/search/movie?api_key={}&query={}", self.api_key, title);
        
        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
            return Err("Failed to fetch from TMDB".into());
        }
        
        // 实际实现应该解析 TMDB JSON，此处为了兼容模型，我们返回一个 Mock 的解析体：
        Ok(Movie {
            id: 0,
            title: title.to_string(),
            year: Some(2023),
            file_path: std::path::PathBuf::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_metadata() {
        let scraper = TmdbScraper::new("dummy_key".to_string());
        // 期望在没有网络或错误 key 时返回清晰的 Error，而不是目前的 stub string
        let res = scraper.fetch_movie_metadata("Inception").await;
        assert!(res.is_err());
    }
}
