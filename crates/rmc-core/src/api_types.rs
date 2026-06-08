use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct MovieResponse {
    pub movie: crate::models::Movie,
    pub stream_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_movie_response_serialization() {
        let resp = MovieResponse {
            movie: crate::models::Movie {
                id: 1,
                title: "Test".to_string(),
                year: Some(2024),
                file_path: std::path::PathBuf::from("/test.mp4"),
            },
            stream_url: "/stream/1/direct".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("stream_url"));
    }
}
