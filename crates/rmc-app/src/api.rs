use crate::config::ServerConfig;
use rmc_core::api_types::{LibraryItem, LibraryItemKind, LibraryItemsResponse};
use std::collections::VecDeque;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("server returned status {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
}

#[derive(Debug, Clone)]
pub struct ApiClient {
    config: ServerConfig,
    http: reqwest::Client,
}

impl ApiClient {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    pub async fn fetch_library_items(&self) -> Result<LibraryItemsResponse, ApiError> {
        self.get_json("/api/v1/library/items").await
    }

    pub async fn fetch_library_item(&self, id: &str) -> Result<LibraryItem, ApiError> {
        let encoded = urlencoding::encode(id);
        self.get_json(&format!("/api/v1/library/items/{encoded}"))
            .await
    }

    pub async fn fetch_library_children(
        &self,
        parent_id: &str,
    ) -> Result<LibraryItemsResponse, ApiError> {
        let encoded = urlencoding::encode(parent_id);
        self.get_json(&format!("/api/v1/library/items?parent_id={encoded}"))
            .await
    }

    pub async fn fetch_playable_items(&self) -> Result<Vec<LibraryItem>, ApiError> {
        let root = self.fetch_library_items().await?;
        let mut playable = Vec::new();
        let mut queue = VecDeque::from(root.items);

        while let Some(item) = queue.pop_front() {
            if is_playable_library_item(&item) {
                playable.push(item);
                continue;
            }

            if item.child_count > 0 {
                let children = self.fetch_library_children(&item.id).await?;
                queue.extend(children.items);
            }
        }

        Ok(playable)
    }

    async fn get_json<T>(&self, path: &str) -> Result<T, ApiError>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = self
            .http
            .get(format!("{}{}", self.config.base_url(), path))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_else(|_| String::new());
            return Err(ApiError::Status { status, body });
        }

        Ok(response.json::<T>().await?)
    }
}

fn is_playable_library_item(item: &LibraryItem) -> bool {
    item.play_id.is_some() && matches!(item.kind, LibraryItemKind::Movie | LibraryItemKind::Episode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_json_server(status: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = socket.read(&mut buffer).await.unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{}", addr)
    }

    async fn spawn_sequence_server(
        bodies: Vec<&'static str>,
        seen_paths: Arc<Mutex<Vec<String>>>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for body in bodies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 2048];
                let bytes = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..bytes]);
                if let Some(path) = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                {
                    seen_paths.lock().unwrap().push(path.to_string());
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn fetch_library_items_decodes_server_items() {
        let body = r#"{"items":[{"id":"movie:1","kind":"movie","parent_id":null,"title":"Test","year":2024,"poster_url":null,"overview":null,"child_count":0,"play_id":1,"season_number":null,"episode_number":null,"runtime_minutes":90,"runtime_seconds":5400,"added_at":1,"file_size":100}],"total_record_count":1,"start_index":0}"#;
        let base_url = spawn_json_server("200 OK", body).await;
        let client = ApiClient::new(ServerConfig::from_base_url(base_url).unwrap());

        let response = client.fetch_library_items().await.unwrap();

        assert_eq!(response.total_record_count, 1);
        assert_eq!(response.items[0].title, "Test");
        assert_eq!(response.items[0].play_id, Some(1));
    }

    #[tokio::test]
    async fn fetch_library_items_reports_non_success_status() {
        let base_url = spawn_json_server("500 Internal Server Error", r#"{"error":"boom"}"#).await;
        let client = ApiClient::new(ServerConfig::from_base_url(base_url).unwrap());

        let err = client.fetch_library_items().await.unwrap_err();

        assert!(err.to_string().contains("500"));
        assert!(err.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn fetch_playable_items_recurses_through_series_and_seasons() {
        let root = r#"{"items":[{"id":"series:tiny","kind":"series","parent_id":null,"title":"Tiny","year":2024,"poster_url":null,"overview":null,"child_count":1,"play_id":null,"season_number":null,"episode_number":null,"runtime_minutes":null,"runtime_seconds":null,"added_at":1,"file_size":null}],"total_record_count":1,"start_index":0}"#;
        let season = r#"{"items":[{"id":"season:tiny:1","kind":"season","parent_id":"series:tiny","title":"Season 1","year":2024,"poster_url":null,"overview":null,"child_count":1,"play_id":null,"season_number":1,"episode_number":null,"runtime_minutes":null,"runtime_seconds":null,"added_at":1,"file_size":null}],"total_record_count":1,"start_index":0}"#;
        let episode = r#"{"items":[{"id":"episode:77","kind":"episode","parent_id":"season:tiny:1","title":"Episode 1","year":2024,"poster_url":null,"overview":null,"child_count":0,"play_id":77,"season_number":1,"episode_number":1,"runtime_minutes":24,"runtime_seconds":1440,"added_at":1,"file_size":100}],"total_record_count":1,"start_index":0}"#;
        let seen_paths = Arc::new(Mutex::new(Vec::new()));
        let base_url =
            spawn_sequence_server(vec![root, season, episode], Arc::clone(&seen_paths)).await;
        let client = ApiClient::new(ServerConfig::from_base_url(base_url).unwrap());

        let playable = client.fetch_playable_items().await.unwrap();

        assert_eq!(playable.len(), 1);
        assert_eq!(playable[0].title, "Episode 1");
        assert_eq!(playable[0].play_id, Some(77));
        assert_eq!(
            *seen_paths.lock().unwrap(),
            vec![
                "/api/v1/library/items".to_string(),
                "/api/v1/library/items?parent_id=series%3Atiny".to_string(),
                "/api/v1/library/items?parent_id=season%3Atiny%3A1".to_string(),
            ]
        );
    }
}
