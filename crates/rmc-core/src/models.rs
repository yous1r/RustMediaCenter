use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Movie {
    pub id: i64,
    pub title: String,
    pub year: Option<u16>,
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
        };
        let json = serde_json::to_string(&movie).unwrap();
        assert!(json.contains(r#""title":"Test Movie""#));
        
        let deserialized: Movie = serde_json::from_str(&json).unwrap();
        assert_eq!(movie, deserialized);
    }
}
