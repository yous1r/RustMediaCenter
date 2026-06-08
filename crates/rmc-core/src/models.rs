use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Movie {
    pub id: i64,
    pub title: String,
    pub year: Option<u16>,
    pub file_path: String,
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
            file_path: "/path/to/movie.mp4".to_string(),
        };
        let json = serde_json::to_string(&movie).unwrap();
        assert!(json.contains(r#""title":"Test Movie""#));
    }
}
