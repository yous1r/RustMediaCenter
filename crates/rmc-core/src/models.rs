use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Movie {
    pub id: i64,
    pub title: String,
    pub year: Option<u16>,
    pub file_path: std::path::PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
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
        };
        let json = serde_json::to_string(&movie).unwrap();
        assert!(json.contains(r#""title":"Test Movie""#));
        
        let deserialized: Movie = serde_json::from_str(&json).unwrap();
        assert_eq!(movie, deserialized);
    }

    #[test]
    fn test_user_model() {
        let user = User { id: 1, username: "admin".to_string() };
        assert_eq!(user.username, "admin");
    }
}
