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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LibraryItemKind {
    Movie,
    Series,
    Season,
    Episode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryItem {
    pub id: String,
    pub kind: LibraryItemKind,
    pub parent_id: Option<String>,
    pub title: String,
    pub year: Option<u16>,
    pub poster_url: Option<String>,
    pub overview: Option<String>,
    pub child_count: usize,
    pub play_id: Option<i64>,
    pub season_number: Option<u16>,
    pub episode_number: Option<u16>,
    pub runtime_minutes: Option<u16>,
    pub runtime_seconds: Option<u32>,
    pub added_at: i64,
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryItemsResponse {
    pub items: Vec<LibraryItem>,
    pub total_record_count: usize,
    pub start_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarySummary {
    pub id: String,
    pub name: String,
    pub count: usize,
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
                runtime_seconds: None,
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

    #[test]
    fn test_library_item_serialization() {
        let item = LibraryItem {
            id: "series:tiny-world".to_string(),
            kind: LibraryItemKind::Series,
            parent_id: None,
            title: "Tiny World".to_string(),
            year: None,
            poster_url: Some("https://example.com/poster.jpg".to_string()),
            overview: Some("Nature documentary".to_string()),
            child_count: 2,
            play_id: None,
            season_number: None,
            episode_number: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("series"));
        assert!(json.contains("Tiny World"));
    }
}
