use rmc_core::models::Movie;
use serde::Deserialize;

pub struct TmdbScraper {
    api_key: String,
    client: reqwest::Client,
}

#[derive(Deserialize, Debug)]
struct TmdbSearchResponse {
    results: Vec<TmdbMovie>,
}

#[derive(Deserialize, Debug)]
struct TmdbMovie {
    id: i64,
    title: String,
    poster_path: Option<String>,
    overview: Option<String>,
    release_date: Option<String>,
}

impl TmdbScraper {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }

    pub async fn fetch_movie_metadata(&self, title: &str) -> Result<Movie, Box<dyn std::error::Error + Send + Sync>> {
        let encoded_title = urlencoding::encode(title);
        let url = format!(
            "https://api.themoviedb.org/3/search/movie?api_key={}&query={}&language=zh-CN",
            self.api_key, encoded_title
        );
        
        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await.unwrap_or_default();
            return Err(format!("TMDB API error: status code {}, response: {}", status, err_body).into());
        }
        
        let search_response: TmdbSearchResponse = response.json().await?;
        if search_response.results.is_empty() {
            return Err("No matching movie found on TMDB".into());
        }

        let best_match = &search_response.results[0];
        
        let poster_url = best_match.poster_path.as_ref().map(|path| {
            format!("https://image.tmdb.org/t/p/w500{}", path)
        });

        let year = best_match.release_date.as_ref().and_then(|date| {
            if date.len() >= 4 {
                date[0..4].parse::<u16>().ok()
            } else {
                None
            }
        });

        Ok(Movie {
            id: 0,
            title: best_match.title.clone(),
            year,
            file_path: std::path::PathBuf::new(),
            poster_url,
            overview: best_match.overview.clone(),
            tmdb_id: Some(best_match.id),
            runtime_minutes: None,
            added_at: 0,
            file_size: None,
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

    #[tokio::test]
    async fn test_fetch_metadata_live_mock() {
        let scraper = TmdbScraper::new("invalid_api_key_test_123".to_string());
        let res = scraper.fetch_movie_metadata("Inception").await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("401") || err_msg.contains("Unauthorized") || err_msg.contains("API key") || err_msg.contains("status code"),
            "Error message was not clear: {}",
            err_msg
        );
    }
}
