use rmc_core::models::Movie;
use rusqlite::{params, Connection, Result};
use std::path::PathBuf;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        Ok(Self {
            conn: Connection::open(path)?,
        })
    }

    pub fn new_in_memory() -> Result<Self> {
        Ok(Self {
            conn: Connection::open_in_memory()?,
        })
    }

    pub fn init_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS movies (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                year INTEGER,
                file_path TEXT NOT NULL
            )",
            [],
        )?;
        Ok(())
    }

    pub fn insert_movie(&self, movie: &Movie) -> Result<()> {
        self.conn.execute(
            "INSERT INTO movies (id, title, year, file_path) VALUES (?1, ?2, ?3, ?4)",
            params![
                movie.id,
                movie.title,
                movie.year,
                movie.file_path.to_string_lossy().to_string()
            ],
        )?;
        Ok(())
    }

    pub fn get_all_movies(&self) -> Result<Vec<Movie>> {
        let mut stmt = self.conn.prepare("SELECT id, title, year, file_path FROM movies")?;
        let iter = stmt.query_map([], |row| {
            let file_path_str: String = row.get(3)?;
            Ok(Movie {
                id: row.get(0)?,
                title: row.get(1)?,
                year: row.get(2)?,
                file_path: PathBuf::from(file_path_str),
            })
        })?;

        let mut movies = Vec::new();
        for m in iter {
            movies.push(m?);
        }
        Ok(movies)
    }

    pub fn search_movies(&self, _query: &str) -> Vec<rmc_core::models::Movie> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmc_core::models::Movie;

    #[test]
    fn test_db_init_and_insert() {
        let db = Database::new_in_memory().unwrap();
        db.init_schema().unwrap();
        
        let movie = Movie {
            id: 1,
            title: "Inception".to_string(),
            year: Some(2010),
            file_path: std::path::PathBuf::from("/movies/inception.mp4"),
        };
        
        db.insert_movie(&movie).unwrap();
        let movies = db.get_all_movies().unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].title, "Inception");
    }

    #[test]
    fn test_search_movies() {
        let db = Database::new_in_memory().unwrap();
        db.init_schema().unwrap();
        let results = db.search_movies("Matrix");
        assert_eq!(results.len(), 0);
    }
}
