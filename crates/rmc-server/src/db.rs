use rmc_core::models::Movie;
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};

const DEFAULT_MAX_CONNECTIONS: u32 = 5;

#[derive(Clone)]
pub struct Database {
    pub pool: SqlitePool,
}

fn map_row_to_movie(r: &sqlx::sqlite::SqliteRow) -> Movie {
    Movie {
        id: r.get::<i64, _>("id"),
        title: r.get::<String, _>("title"),
        year: r
            .get::<Option<i32>, _>("year")
            .and_then(|y| y.try_into().ok()),
        file_path: std::path::PathBuf::from(r.get::<String, _>("file_path")),
        poster_url: r.get::<Option<String>, _>("poster_url"),
        overview: r.get::<Option<String>, _>("overview"),
        tmdb_id: r.get::<Option<i64>, _>("tmdb_id"),
        runtime_minutes: r
            .get::<Option<i32>, _>("runtime_minutes")
            .and_then(|mins| mins.try_into().ok()),
        runtime_seconds: r
            .get::<Option<i64>, _>("runtime_seconds")
            .and_then(|secs| secs.try_into().ok()),
        added_at: r.get::<i64, _>("added_at"),
        file_size: r
            .get::<Option<i64>, _>("file_size")
            .and_then(|size| size.try_into().ok()),
    }
}

impl Database {
    pub async fn new(db_url: &str) -> Result<Self, sqlx::Error> {
        use sqlx::sqlite::SqliteConnectOptions;
        use std::str::FromStr;

        let options = SqliteConnectOptions::from_str(db_url)?.create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(DEFAULT_MAX_CONNECTIONS)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    pub async fn init_schema(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS movies (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                year INTEGER,
                file_path TEXT NOT NULL UNIQUE,
                poster_url TEXT,
                overview TEXT,
                tmdb_id INTEGER,
                runtime_minutes INTEGER,
                runtime_seconds INTEGER,
                added_at INTEGER NOT NULL,
                file_size INTEGER
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS movies_fts USING fts5(
                title, overview, content='movies', content_rowid='id'
            );
            -- 建立触发器以实现 movies 表和 FTS 表的同步
            CREATE TRIGGER IF NOT EXISTS movies_ai AFTER INSERT ON movies BEGIN
                INSERT INTO movies_fts(rowid, title, overview) VALUES (new.id, new.title, new.overview);
            END;
            CREATE TRIGGER IF NOT EXISTS movies_ad AFTER DELETE ON movies BEGIN
                INSERT INTO movies_fts(movies_fts, rowid, title, overview) VALUES('delete', old.id, old.title, old.overview);
            END;
            CREATE TRIGGER IF NOT EXISTS movies_au AFTER UPDATE ON movies BEGIN
                INSERT INTO movies_fts(movies_fts, rowid, title, overview) VALUES('delete', old.id, old.title, old.overview);
                INSERT INTO movies_fts(rowid, title, overview) VALUES (new.id, new.title, new.overview);
            END;"
        )
        .execute(&self.pool)
        .await?;

        let columns = sqlx::query("PRAGMA table_info(movies)")
            .fetch_all(&self.pool)
            .await?;
        let has_runtime_seconds = columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "runtime_seconds");
        if !has_runtime_seconds {
            sqlx::query("ALTER TABLE movies ADD COLUMN runtime_seconds INTEGER")
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    pub async fn insert_movie(&self, movie: &Movie) -> Result<(), sqlx::Error> {
        let file_path_str = movie.file_path.to_string_lossy().into_owned();
        sqlx::query(
            "INSERT OR IGNORE INTO movies (title, year, file_path, poster_url, overview, tmdb_id, runtime_minutes, runtime_seconds, added_at, file_size)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&movie.title)
        .bind(movie.year.map(|y| y as i32))
        .bind(&file_path_str)
        .bind(&movie.poster_url)
        .bind(&movie.overview)
        .bind(movie.tmdb_id)
        .bind(movie.runtime_minutes.map(|r| r as i32))
        .bind(movie.runtime_seconds.map(|r| r as i64))
        .bind(movie.added_at)
        .bind(movie.file_size.map(|s| s as i64))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn movie_exists_by_path(&self, path: &str) -> Result<bool, sqlx::Error> {
        let rec = sqlx::query("SELECT 1 FROM movies WHERE file_path = ? LIMIT 1")
            .bind(path)
            .fetch_optional(&self.pool)
            .await?;
        Ok(rec.is_some())
    }

    pub async fn update_movie_title_year_by_path(
        &self,
        path: &str,
        title: &str,
        year: Option<u16>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE movies SET title = ?, year = COALESCE(?, year) WHERE file_path = ?")
            .bind(title)
            .bind(year.map(|value| value as i32))
            .bind(path)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_movie_by_id(&self, id: i64) -> Result<Movie, sqlx::Error> {
        let r = sqlx::query("SELECT id, title, year, file_path, poster_url, overview, tmdb_id, runtime_minutes, runtime_seconds, added_at, file_size FROM movies WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;

        Ok(map_row_to_movie(&r))
    }

    async fn retain_existing_movies(&self, movies: Vec<Movie>) -> Result<Vec<Movie>, sqlx::Error> {
        let mut existing = Vec::with_capacity(movies.len());
        let mut stale_ids = Vec::new();

        for movie in movies {
            if movie.file_path.exists() {
                existing.push(movie);
            } else {
                stale_ids.push(movie.id);
            }
        }

        for id in stale_ids {
            self.delete_movie(id).await?;
        }

        Ok(existing)
    }

    pub async fn get_available_movie_by_id(&self, id: i64) -> Result<Movie, sqlx::Error> {
        let movie = self.get_movie_by_id(id).await?;
        let mut movies = self.retain_existing_movies(vec![movie]).await?;
        movies.pop().ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn update_movie_metadata(
        &self,
        id: i64,
        poster_url: Option<String>,
        overview: Option<String>,
        tmdb_id: Option<i64>,
        runtime_minutes: Option<u16>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE movies SET poster_url = ?, overview = ?, tmdb_id = ?, runtime_minutes = ? WHERE id = ?"
        )
        .bind(poster_url)
        .bind(overview)
        .bind(tmdb_id)
        .bind(runtime_minutes.map(|r| r as i32))
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_movie_runtime(
        &self,
        id: i64,
        runtime_seconds: Option<u32>,
        runtime_minutes: Option<u16>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE movies SET runtime_seconds = ?, runtime_minutes = ? WHERE id = ?")
            .bind(runtime_seconds.map(|value| value as i64))
            .bind(runtime_minutes.map(|value| value as i32))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_movies_without_metadata(&self) -> Result<Vec<Movie>, sqlx::Error> {
        let rows = sqlx::query("SELECT id, title, year, file_path, poster_url, overview, tmdb_id, runtime_minutes, runtime_seconds, added_at, file_size FROM movies WHERE tmdb_id IS NULL")
            .fetch_all(&self.pool)
            .await?;

        let movies = rows.iter().map(map_row_to_movie).collect();
        Ok(movies)
    }

    pub async fn get_movie_count(&self) -> Result<i64, sqlx::Error> {
        let r = sqlx::query("SELECT COUNT(*) as count FROM movies")
            .fetch_one(&self.pool)
            .await?;
        Ok(r.get::<i64, _>("count"))
    }

    pub async fn delete_movie(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM movies WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_movies(&self) -> Result<Vec<Movie>, sqlx::Error> {
        let rows = sqlx::query("SELECT id, title, year, file_path, poster_url, overview, tmdb_id, runtime_minutes, runtime_seconds, added_at, file_size FROM movies")
            .fetch_all(&self.pool)
            .await?;

        let movies = rows.iter().map(map_row_to_movie).collect();
        Ok(movies)
    }

    pub async fn get_available_movies(&self) -> Result<Vec<Movie>, sqlx::Error> {
        let movies = self.get_movies().await?;
        self.retain_existing_movies(movies).await
    }

    pub async fn search_movies(&self, query: &str) -> Result<Vec<Movie>, sqlx::Error> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let wildcard_query = format!("{}*", query);
        let rows = sqlx::query(
            "SELECT m.id, m.title, m.year, m.file_path, m.poster_url, m.overview, m.tmdb_id, m.runtime_minutes, m.runtime_seconds, m.added_at, m.file_size
             FROM movies m JOIN movies_fts f ON m.id = f.rowid
             WHERE movies_fts MATCH ?"
        )
        .bind(wildcard_query)
        .fetch_all(&self.pool)
        .await?;

        let movies = rows.iter().map(map_row_to_movie).collect();
        Ok(movies)
    }

    pub async fn search_available_movies(&self, query: &str) -> Result<Vec<Movie>, sqlx::Error> {
        let movies = self.search_movies(query).await?;
        self.retain_existing_movies(movies).await
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
            runtime_seconds: None,
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
        // 如果不抛错说明连接池及 schema 成功初始化
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
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        };
        db.insert_movie(&m).await.unwrap();

        let res = db.search_movies("Matrix").await.unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].title, "The Matrix");
    }

    #[tokio::test]
    async fn test_database_metadata_operations() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        let path = std::path::PathBuf::from("/media/inception.mp4");
        let movie = rmc_core::models::Movie {
            id: 0,
            title: "Inception".to_string(),
            year: Some(2010),
            file_path: path.clone(),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 1717896000,
            file_size: Some(5000000),
        };

        // 插入电影
        db.insert_movie(&movie).await.unwrap();

        // 测试重复路径检查
        let exists = db
            .movie_exists_by_path("/media/inception.mp4")
            .await
            .unwrap();
        assert!(exists);

        // 获取所有电影
        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), 1);
        let movie_id = movies[0].id;

        // 通过 ID 查询
        let movie_by_id = db.get_movie_by_id(movie_id).await.unwrap();
        assert_eq!(movie_by_id.title, "Inception");

        // 更新元数据
        db.update_movie_metadata(
            movie_id,
            Some("http://image/path.jpg".to_string()),
            Some("Dream".to_string()),
            Some(27205),
            Some(148),
        )
        .await
        .unwrap();

        // 验证元数据已更新
        let updated = db.get_movie_by_id(movie_id).await.unwrap();
        assert_eq!(
            updated.poster_url,
            Some("http://image/path.jpg".to_string())
        );
        assert_eq!(updated.overview, Some("Dream".to_string()));
        assert_eq!(updated.tmdb_id, Some(27205));
        assert_eq!(updated.runtime_minutes, Some(148));

        // 获取未刮削电影列表
        let uncompleted = db.get_movies_without_metadata().await.unwrap();
        assert_eq!(uncompleted.len(), 0); // 刚才已经刮削过了

        // 获取电影总数
        let count = db.get_movie_count().await.unwrap();
        assert_eq!(count, 1);

        // 删除电影
        db.delete_movie(movie_id).await.unwrap();
        let count_after_delete = db.get_movie_count().await.unwrap();
        assert_eq!(count_after_delete, 0);
    }

    #[tokio::test]
    async fn test_update_movie_title_year_by_path() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        let movie = Movie {
            id: 0,
            title: "Dirty Title".to_string(),
            year: None,
            file_path: std::path::PathBuf::from("/media/fate-zero.mkv"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        };

        db.insert_movie(&movie).await.unwrap();
        db.update_movie_title_year_by_path("/media/fate-zero.mkv", "Fate Zero", Some(2011))
            .await
            .unwrap();

        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].title, "Fate Zero");
        assert_eq!(movies[0].year, Some(2011));
    }

    #[tokio::test]
    async fn test_update_movie_title_year_by_path_preserves_existing_year_when_missing() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        let movie = Movie {
            id: 0,
            title: "Dirty Title".to_string(),
            year: Some(2006),
            file_path: std::path::PathBuf::from("/media/fate-stay-night.mkv"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        };

        db.insert_movie(&movie).await.unwrap();
        db.update_movie_title_year_by_path("/media/fate-stay-night.mkv", "Fate Stay Night", None)
            .await
            .unwrap();

        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].title, "Fate Stay Night");
        assert_eq!(movies[0].year, Some(2006));
    }

    #[tokio::test]
    async fn test_sqlite_file_connection() {
        use sqlx::sqlite::SqliteConnectOptions;
        use std::str::FromStr;

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("rmc.db");
        let db_path_str = db_path.to_string_lossy().to_string();

        // 方案 1：直接通过 SqliteConnectOptions 显式配置 create_if_missing(true)
        let options = SqliteConnectOptions::from_str(&format!("sqlite:{}", db_path_str))
            .unwrap()
            .create_if_missing(true);
        let pool_res1 = SqlitePoolOptions::new().connect_with(options).await;

        // 方案 2：先在磁盘上创建出空白文件，再用原连接方式
        let db_path_manual = temp_dir.path().join("rmc_manual.db");
        std::fs::File::create(&db_path_manual).unwrap();
        let pool_res2 = SqlitePoolOptions::new()
            .connect(&format!("sqlite:{}", db_path_manual.to_string_lossy()))
            .await;

        assert!(
            pool_res1.is_ok(),
            "SqliteConnectOptions with create_if_missing(true) failed: {:?}",
            pool_res1.err()
        );
        assert!(
            pool_res2.is_ok(),
            "Manual file creation connect failed: {:?}",
            pool_res2.err()
        );
    }
}
