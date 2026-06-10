use rmc_core::models::Movie;
use serde::Deserialize;

const TMDB_API_URL: &str = "https://api.themoviedb.org/3/search/movie";
const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w500";

pub struct TmdbScraper {
    api_key: String,
    client: reqwest::Client,
    api_base: Option<String>,
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
    pub fn new(api_key: String, proxy_url: Option<String>, api_base: Option<String>) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10));

        let proxy_url = proxy_url.filter(|s| !s.trim().is_empty());
        let api_base = api_base.filter(|s| !s.trim().is_empty());

        if let Some(ref proxy) = proxy_url {
            if let Ok(reqwest_proxy) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(reqwest_proxy);
            }
        }

        let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
        Self { api_key, client, api_base }
    }

    pub async fn fetch_movie_metadata(&self, title: &str) -> Result<Movie, Box<dyn std::error::Error + Send + Sync>> {
        let url = if let Some(ref base) = self.api_base {
            let base_trimmed = base.trim_end_matches('/');
            format!("{}/3/search/movie", base_trimmed)
        } else {
            TMDB_API_URL.to_string()
        };

        let response = self.client.get(&url)
            .query(&[
                ("api_key", self.api_key.as_str()),
                ("query", title),
                ("language", "zh-CN"),
            ])
            .send()
            .await
            .map_err(|e| format!("TMDB request failed for {}: {}", url, e))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await.unwrap_or_default();
            return Err(format!("TMDB API error: status code {}, response: {}", status, err_body).into());
        }
        
        let text = response.text().await?;
        let search_response: TmdbSearchResponse = serde_json::from_str(&text)
            .map_err(|e| format!("Failed to parse TMDB JSON response: {}, response body: {}", e, text))?;
            
        if search_response.results.is_empty() {
            return Err("No matching movie found on TMDB".into());
        }

        let best_match = &search_response.results[0];
        
        let poster_url = best_match.poster_path.as_ref().map(|path| {
            format!("{}{}", TMDB_IMAGE_BASE, path)
        });

        let year = best_match.release_date.as_ref().and_then(|date| {
            date.split('-').next().and_then(|y| y.parse::<u16>().ok())
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
        let scraper = TmdbScraper::new("dummy_key".to_string(), None, None);
        // 期望在没有网络或错误 key 时返回清晰的 Error，而不是目前的 stub string
        let res = scraper.fetch_movie_metadata("Inception").await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_fetch_metadata_live_mock() {
        let scraper = TmdbScraper::new("invalid_api_key_test_123".to_string(), None, None);
        let res = scraper.fetch_movie_metadata("Inception").await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("401")
                || err_msg.contains("Unauthorized")
                || err_msg.contains("API key")
                || err_msg.contains("status code")
                || err_msg.contains("TMDB request failed"),
            "Error message was not clear: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_fetch_metadata_custom_base() {
        let scraper = TmdbScraper::new(
            "dummy_key".to_string(),
            None,
            Some("https://invalid.domain.tmdb-proxy.com".to_string()),
        );
        let res = scraper.fetch_movie_metadata("Inception").await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("invalid.domain.tmdb-proxy.com"),
            "Error message did not contain custom base URL: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_fetch_metadata_empty_base() {
        let scraper = TmdbScraper::new(
            "invalid_api_key_test_123".to_string(),
            None,
            Some("   ".to_string()),
        );
        let res = scraper.fetch_movie_metadata("Inception").await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("401")
                || err_msg.contains("Unauthorized")
                || err_msg.contains("API key")
                || err_msg.contains("status code")
                || err_msg.contains("TMDB request failed"),
            "Error message was not clear: {}",
            err_msg
        );
    }
}
