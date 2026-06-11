use reqwest::Client;
use rmc_core::models::Movie;

pub struct ApiClient {
    base_url: String,
    client: Client,
}

impl ApiClient {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: Client::new(),
        }
    }

    pub async fn fetch_movies(&self) -> Result<Vec<Movie>, String> {
        let url = format!("{}/api/v1/movies", self.base_url);
        let res = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if res.status().is_success() {
            let movies = res.json::<Vec<Movie>>().await.map_err(|e| e.to_string())?;
            Ok(movies)
        } else {
            Err(format!("Server returned status: {}", res.status()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_movies_actual_request() {
        let client = ApiClient::new("http://127.0.0.1:3000".to_string());
        // 如果服务器没开，真实的 HTTP 客户端应该返回确切的连接被拒绝错误，而非单纯的未实现错误
        let res = client.fetch_movies().await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err();
        assert!(
            err_msg.contains("Connection refused")
                || err_msg.contains("connect")
                || err_msg.contains("error sending request")
        );
    }
}
