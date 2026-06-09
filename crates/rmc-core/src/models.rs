use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Movie {
    pub id: i64,
    pub title: String,
    pub year: Option<u16>,
    pub file_path: std::path::PathBuf,
    pub poster_url: Option<String>,
    pub overview: Option<String>,
    pub tmdb_id: Option<i64>,
    pub runtime_minutes: Option<u16>,
    pub added_at: i64,
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TVShow {
    pub id: i64,
    pub title: String,
    pub season_count: Option<u16>,
    pub file_path: std::path::PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_movie_serialization() {
        let movie = Movie {
            id: 1,
            title: "Test Movie".to_string(),
            year: Some(2024),
            file_path: std::path::PathBuf::from("/path/to/movie.mp4"),
            poster_url: Some("https://example.com/poster.jpg".to_string()),
            overview: Some("This is a test movie overview".to_string()),
            tmdb_id: Some(12345),
            runtime_minutes: Some(120),
            added_at: 1718021000,
            file_size: Some(1024000),
        };
        let json = serde_json::to_string(&movie).unwrap();
        assert!(json.contains(r#""title":"Test Movie""#));
        assert!(json.contains(r#""poster_url":"https://example.com/poster.jpg""#));
        assert!(json.contains(r#""overview":"This is a test movie overview""#));
        assert!(json.contains(r#""tmdb_id":12345"#));
        assert!(json.contains(r#""runtime_minutes":120"#));
        assert!(json.contains(r#""added_at":1718021000"#));
        assert!(json.contains(r#""file_size":1024000"#));
        
        let deserialized: Movie = serde_json::from_str(&json).unwrap();
        assert_eq!(movie, deserialized);
    }

    #[test]
    fn test_user_model() {
        let user = User { id: 1, username: "admin".to_string() };
        assert_eq!(user.username, "admin");
    }

    #[test]
    fn test_tvshow_serialization() {
        let show = super::TVShow {
            id: 1,
            title: "Test Show".to_string(),
            season_count: Some(3),
            file_path: std::path::PathBuf::from("/path/to/show"),
        };
        let json = serde_json::to_string(&show).unwrap();
        assert!(json.contains("Test Show"));
    }
}
