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

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
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
                poster_url: None,
                overview: None,
                tmdb_id: None,
                runtime_minutes: None,
                added_at: 0,
                file_size: None,
            },
            stream_url: "/stream/1/direct".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("stream_url"));
    }

    #[test]
    fn test_login_response_serialization() {
        let resp = LoginResponse {
            token: "jwt-token".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("jwt-token"));
    }
}
