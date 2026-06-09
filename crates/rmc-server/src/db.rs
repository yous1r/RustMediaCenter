use sqlx::{sqlite::SqlitePoolOptions, SqlitePool, Row};
use rmc_core::models::Movie;

#[derive(Clone)]
pub struct Database {
    pub pool: SqlitePool,
}

impl Database {
    pub async fn new(db_url: &str) -> Result<Self, sqlx::Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(db_url).await?;
        Ok(Self { pool })
    }

    pub async fn init_schema(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS movies (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                year INTEGER NOT NULL,
                file_path TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS movies_fts USING fts5(title, content='movies', content_rowid='id');
            "
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_movie(&self, movie: &Movie) -> Result<(), sqlx::Error> {
        let file_path_str = movie.file_path.to_string_lossy().to_string();
        let result = sqlx::query("INSERT INTO movies (title, year, file_path) VALUES (?, ?, ?)")
            .bind(&movie.title)
            .bind(movie.year)
            .bind(&file_path_str)
            .execute(&self.pool)
            .await?;
        let id = result.last_insert_rowid();
        sqlx::query("INSERT INTO movies_fts (rowid, title) VALUES (?, ?)")
            .bind(id)
            .bind(&movie.title)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_movies(&self) -> Result<Vec<Movie>, sqlx::Error> {
        use sqlx::Row;
        let rows = sqlx::query("SELECT id, title, year, file_path FROM movies").fetch_all(&self.pool).await?;
        let movies = rows.into_iter().map(|r| Movie {
            id: r.get::<i64, _>("id"),
            title: r.get::<String, _>("title"),
            year: Some(r.get::<i64, _>("year") as u16),
            file_path: std::path::PathBuf::from(r.get::<String, _>("file_path")),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            added_at: 0,
            file_size: None,
        }).collect();
        Ok(movies)
    }

    pub async fn search_movies(&self, query: &str) -> Result<Vec<Movie>, sqlx::Error> {
        let q = format!("{}*", query); // SQLite FTS wildcard
        let rows = sqlx::query("SELECT m.id, m.title, m.year, m.file_path FROM movies m JOIN movies_fts f ON m.id = f.rowid WHERE movies_fts MATCH ?")
            .bind(q)
            .fetch_all(&self.pool).await?;
        let movies = rows.into_iter().map(|r| Movie {
            id: r.get::<i64, _>("id"),
            title: r.get::<String, _>("title"),
            year: Some(r.get::<i64, _>("year") as u16),
            file_path: std::path::PathBuf::from(r.get::<String, _>("file_path")),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            added_at: 0,
            file_size: None,
        }).collect();
        Ok(movies)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmc_core::models::Movie;

    #[tokio::test]
    async fn test_db_init_and_insert() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        
        let movie = Movie {
            id: 1,
            title: "Inception".to_string(),
            year: Some(2010),
            file_path: std::path::PathBuf::from("/movies/inception.mp4"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            added_at: 0,
            file_size: None,
        };
        
        db.insert_movie(&movie).await.unwrap();
        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].title, "Inception");
    }

    #[tokio::test]
    async fn test_search_movies() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let results = db.search_movies("Matrix").await.unwrap();
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_sqlx_pool_init() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        assert!(true); // 如果不抛错说明连接池及 schema 成功初始化
    }

    #[tokio::test]
    async fn test_fts5_search() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let m = rmc_core::models::Movie {
            id: 0,
            title: "The Matrix".to_string(),
            year: Some(1999),
            file_path: std::path::PathBuf::from("/m.mkv"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            added_at: 0,
            file_size: None,
        };
        db.insert_movie(&m).await.unwrap();
        
        let res = db.search_movies("Matrix").await.unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].title, "The Matrix");
    }
}
