#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrmSource {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

pub const DEFAULT_REMOTE_USER_AGENT: &str = "curl/8.0.1";

impl StrmSource {
    pub fn ffmpeg_header_value(&self) -> Option<String> {
        if self.headers.is_empty() {
            return None;
        }

        Some(
            self.headers
                .iter()
                .map(|(name, value)| format!("{}: {}\r\n", name, value))
                .collect(),
        )
    }

    pub async fn resolve_playable_url(&self) -> Result<String, anyhow::Error> {
        self.resolve_playable_url_with_user_agent(None).await
    }

    pub async fn resolve_playable_url_with_user_agent(
        &self,
        user_agent: Option<&str>,
    ) -> Result<String, anyhow::Error> {
        const MAX_REDIRECTS: usize = 5;

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        let mut current_url = self.url.clone();
        let has_user_agent = self
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("user-agent"));

        for _ in 0..MAX_REDIRECTS {
            let mut request = client.get(&current_url);
            for (name, value) in &self.headers {
                if name.eq_ignore_ascii_case("range") {
                    continue;
                }
                if let (Ok(header_name), Ok(header_value)) = (
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                    reqwest::header::HeaderValue::from_str(value),
                ) {
                    request = request.header(header_name, header_value);
                }
            }
            if !has_user_agent {
                if let Some(user_agent) = user_agent.filter(|value| !value.trim().is_empty()) {
                    request = request.header(reqwest::header::USER_AGENT, user_agent);
                }
            }

            let response = request.send().await?;
            if !response.status().is_redirection() {
                return Ok(current_url);
            }

            let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
            else {
                return Ok(current_url);
            };
            current_url = response.url().join(location)?.to_string();
        }

        Ok(current_url)
    }
}

pub struct StrmParser;

impl StrmParser {
    pub fn parse(content: &str) -> Option<StrmSource> {
        let mut lines = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty());
        let url = lines.find(|line| line.starts_with("http"))?.to_string();
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                let name = name.trim();
                let value = value.trim();

                if name.is_empty() || value.is_empty() {
                    return None;
                }

                Some((name.to_string(), value.to_string()))
            })
            .collect();

        Some(StrmSource { url, headers })
    }
}

#[cfg(test)]

mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_redirect_server() -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = Arc::clone(&requests);

        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = vec![0; 4096];
                let Ok(read_len) = socket.read(&mut buffer).await else {
                    return;
                };
                let request = String::from_utf8_lossy(&buffer[..read_len]).to_string();
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_string();
                requests_clone.lock().unwrap().push(request);

                let response = if path.starts_with("/play/") {
                    "HTTP/1.1 302 Found\r\nlocation: /cdn/movie.mkv\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
                } else {
                    "HTTP/1.1 200 OK\r\ncontent-type: video/x-matroska\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
                };
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });

        (format!("http://{}", addr), requests)
    }

    #[test]
    fn test_parse_strm_file() {
        let source = StrmParser::parse("http://example.com/movie.mp4\n");
        assert_eq!(
            source,
            Some(StrmSource {
                url: "http://example.com/movie.mp4".to_string(),
                headers: vec![],
            })
        );
    }

    #[test]
    fn test_parse_invalid_strm() {
        let source = StrmParser::parse("not a url");
        assert_eq!(source, None);
    }

    #[test]
    fn test_parse_strm_headers_for_ffmpeg() {
        let source = StrmParser::parse(
            "https://example.com/movie.mkv\nUser-Agent: VidHub\nReferer: https://example.com\n",
        )
        .unwrap();

        assert_eq!(source.url, "https://example.com/movie.mkv");
        assert_eq!(
            source.headers,
            vec![
                ("User-Agent".to_string(), "VidHub".to_string()),
                ("Referer".to_string(), "https://example.com".to_string()),
            ]
        );
        assert_eq!(
            source.ffmpeg_header_value().as_deref(),
            Some("User-Agent: VidHub\r\nReferer: https://example.com\r\n")
        );
    }

    #[tokio::test]
    async fn test_resolve_playable_url_follows_redirect_without_range_header() {
        let (base_url, requests) = spawn_redirect_server().await;
        let source = StrmParser::parse(&format!("{}/play/movie.mkv\n", base_url)).unwrap();

        let resolved = source.resolve_playable_url().await.unwrap();

        assert_eq!(resolved, format!("{}/cdn/movie.mkv", base_url));
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("GET /play/movie.mkv"));
        assert!(!requests[0].to_ascii_lowercase().contains("range:"));
    }
}
