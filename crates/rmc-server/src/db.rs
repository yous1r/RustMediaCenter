use rmc_core::models::Movie;
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};

const DEFAULT_MAX_CONNECTIONS: u32 = 5;
const MEDIA_SCHEMA_VERSION: i64 = 1;
const DEFAULT_LIBRARY_ID: i64 = 1;

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

fn movie_runtime_ticks(movie: &Movie) -> Option<i64> {
    movie
        .runtime_seconds
        .map(|seconds| seconds as i64 * 10_000_000)
        .or_else(|| {
            movie
                .runtime_minutes
                .map(|minutes| minutes as i64 * 60 * 10_000_000)
        })
}

fn movie_extension(movie: &Movie) -> Option<String> {
    movie
        .file_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn movie_source_type(movie: &Movie) -> &'static str {
    match movie_extension(movie).as_deref() {
        Some("strm") => "strm",
        _ => "local",
    }
}

fn movie_container(movie: &Movie) -> Option<String> {
    movie_extension(movie).filter(|value| value != "strm")
}

fn sort_title(title: &str) -> String {
    title.trim().to_ascii_lowercase()
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
                metadata_search_hint TEXT,
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
        let has_metadata_search_hint = columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "metadata_search_hint");
        if !has_metadata_search_hint {
            sqlx::query("ALTER TABLE movies ADD COLUMN metadata_search_hint TEXT")
                .execute(&self.pool)
                .await?;
        }
        self.init_media_schema().await?;
        self.backfill_media_schema_from_movies().await?;
        Ok(())
    }

    async fn init_media_schema(&self) -> Result<(), sqlx::Error> {
        let statements = [
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at INTEGER NOT NULL
            )",
            "CREATE TABLE IF NOT EXISTS libraries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                collection_type TEXT NOT NULL,
                path TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            "CREATE TABLE IF NOT EXISTS media_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                library_id INTEGER NOT NULL,
                parent_id INTEGER,
                item_type TEXT NOT NULL,
                title TEXT NOT NULL,
                sort_title TEXT NOT NULL,
                original_title TEXT,
                overview TEXT,
                production_year INTEGER,
                index_number INTEGER,
                parent_index_number INTEGER,
                premiere_date TEXT,
                date_created INTEGER NOT NULL,
                date_modified INTEGER,
                runtime_ticks INTEGER,
                provider_tmdb_id INTEGER,
                provider_imdb_id TEXT,
                provider_tvdb_id TEXT,
                is_folder INTEGER NOT NULL DEFAULT 0,
                is_virtual INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY(library_id) REFERENCES libraries(id),
                FOREIGN KEY(parent_id) REFERENCES media_items(id) ON DELETE CASCADE
            )",
            "CREATE TABLE IF NOT EXISTS media_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                library_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                canonical_path TEXT NOT NULL,
                extension TEXT,
                size INTEGER,
                modified_at INTEGER,
                hash TEXT,
                is_available INTEGER NOT NULL DEFAULT 1,
                last_seen_at INTEGER NOT NULL,
                FOREIGN KEY(library_id) REFERENCES libraries(id)
            )",
            "CREATE TABLE IF NOT EXISTS media_sources (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item_id INTEGER NOT NULL,
                file_id INTEGER,
                source_type TEXT NOT NULL,
                protocol TEXT NOT NULL,
                path TEXT NOT NULL,
                container TEXT,
                size INTEGER,
                runtime_ticks INTEGER,
                bitrate INTEGER,
                supports_direct_play INTEGER NOT NULL DEFAULT 1,
                supports_direct_stream INTEGER NOT NULL DEFAULT 1,
                supports_transcoding INTEGER NOT NULL DEFAULT 1,
                FOREIGN KEY(item_id) REFERENCES media_items(id) ON DELETE CASCADE,
                FOREIGN KEY(file_id) REFERENCES media_files(id)
            )",
            "CREATE TABLE IF NOT EXISTS media_streams (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source_id INTEGER NOT NULL,
                stream_type TEXT NOT NULL,
                index_number INTEGER NOT NULL,
                codec TEXT,
                language TEXT,
                title TEXT,
                width INTEGER,
                height INTEGER,
                channels INTEGER,
                bitrate INTEGER,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_forced INTEGER NOT NULL DEFAULT 0,
                is_external INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY(source_id) REFERENCES media_sources(id) ON DELETE CASCADE
            )",
            "CREATE TABLE IF NOT EXISTS images (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item_id INTEGER NOT NULL,
                image_type TEXT NOT NULL,
                url TEXT,
                path TEXT,
                tag TEXT,
                width INTEGER,
                height INTEGER,
                FOREIGN KEY(item_id) REFERENCES media_items(id) ON DELETE CASCADE
            )",
            "CREATE TABLE IF NOT EXISTS user_item_data (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id TEXT NOT NULL,
                item_id INTEGER NOT NULL,
                playback_position_ticks INTEGER NOT NULL DEFAULT 0,
                play_count INTEGER NOT NULL DEFAULT 0,
                is_favorite INTEGER NOT NULL DEFAULT 0,
                played INTEGER NOT NULL DEFAULT 0,
                last_played_at INTEGER,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY(item_id) REFERENCES media_items(id) ON DELETE CASCADE
            )",
            "CREATE TABLE IF NOT EXISTS playback_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id TEXT NOT NULL,
                item_id INTEGER NOT NULL,
                media_source_id INTEGER,
                device_id TEXT,
                client_name TEXT,
                play_method TEXT,
                position_ticks INTEGER NOT NULL DEFAULT 0,
                is_paused INTEGER NOT NULL DEFAULT 0,
                started_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                stopped_at INTEGER,
                FOREIGN KEY(item_id) REFERENCES media_items(id) ON DELETE CASCADE,
                FOREIGN KEY(media_source_id) REFERENCES media_sources(id)
            )",
            "CREATE VIRTUAL TABLE IF NOT EXISTS media_items_fts USING fts5(
                title,
                original_title,
                overview,
                path_hint
            )",
            "CREATE INDEX IF NOT EXISTS idx_media_items_parent_type_sort
                ON media_items(parent_id, item_type, sort_title)",
            "CREATE INDEX IF NOT EXISTS idx_media_items_library_type_created
                ON media_items(library_id, item_type, date_created DESC)",
            "CREATE INDEX IF NOT EXISTS idx_media_items_parent_indexes
                ON media_items(parent_id, parent_index_number, index_number)",
            "CREATE INDEX IF NOT EXISTS idx_media_items_type_sort
                ON media_items(item_type, sort_title)",
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_media_files_path
                ON media_files(canonical_path)",
            "CREATE INDEX IF NOT EXISTS idx_media_files_library_seen
                ON media_files(library_id, last_seen_at)",
            "CREATE INDEX IF NOT EXISTS idx_media_sources_item
                ON media_sources(item_id)",
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_media_sources_item_file
                ON media_sources(item_id, file_id)",
            "CREATE INDEX IF NOT EXISTS idx_media_streams_source_type
                ON media_streams(source_id, stream_type)",
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_images_item_type
                ON images(item_id, image_type)",
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_user_item_data_user_item
                ON user_item_data(user_id, item_id)",
            "CREATE INDEX IF NOT EXISTS idx_user_item_data_user_played
                ON user_item_data(user_id, played, last_played_at DESC)",
        ];

        for statement in statements {
            sqlx::query(statement).execute(&self.pool).await?;
        }

        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at)
             VALUES (?, ?)",
        )
        .bind(MEDIA_SCHEMA_VERSION)
        .bind(now)
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "INSERT OR IGNORE INTO libraries (id, name, collection_type, path, created_at, updated_at)
             VALUES (?, ?, ?, NULL, ?, ?)",
        )
        .bind(DEFAULT_LIBRARY_ID)
        .bind("Default")
        .bind("mixed")
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn backfill_media_schema_from_movies(&self) -> Result<(), sqlx::Error> {
        let movies = self.get_movies().await?;

        for movie in movies {
            self.upsert_media_schema_for_movie(&movie).await?;
        }

        Ok(())
    }

    async fn get_movie_by_path(&self, path: &str) -> Result<Option<Movie>, sqlx::Error> {
        let row = sqlx::query("SELECT id, title, year, file_path, poster_url, overview, tmdb_id, runtime_minutes, runtime_seconds, added_at, file_size FROM movies WHERE file_path = ?")
            .bind(path)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.as_ref().map(map_row_to_movie))
    }

    async fn sync_media_schema_for_movie_id(&self, id: i64) -> Result<(), sqlx::Error> {
        let movie = self.get_movie_by_id(id).await?;
        self.upsert_media_schema_for_movie(&movie).await
    }

    async fn sync_media_schema_for_movie_path(&self, path: &str) -> Result<(), sqlx::Error> {
        if let Some(movie) = self.get_movie_by_path(path).await? {
            self.upsert_media_schema_for_movie(&movie).await?;
        }
        Ok(())
    }

    async fn upsert_media_schema_for_movie(&self, movie: &Movie) -> Result<(), sqlx::Error> {
        let file_path = movie.file_path.to_string_lossy().into_owned();
        let extension = movie_extension(movie);
        let modified_at = std::fs::metadata(&movie.file_path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64);
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO media_files (
                library_id, path, canonical_path, extension, size, modified_at, is_available, last_seen_at
             )
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(canonical_path) DO UPDATE SET
                path = excluded.path,
                extension = excluded.extension,
                size = excluded.size,
                modified_at = excluded.modified_at,
                is_available = excluded.is_available,
                last_seen_at = excluded.last_seen_at",
        )
        .bind(DEFAULT_LIBRARY_ID)
        .bind(&file_path)
        .bind(&file_path)
        .bind(&extension)
        .bind(movie.file_size.map(|size| size as i64))
        .bind(modified_at)
        .bind(if movie.file_path.exists() { 1 } else { 0 })
        .bind(now)
        .execute(&self.pool)
        .await?;

        let file_id = sqlx::query("SELECT id FROM media_files WHERE canonical_path = ?")
            .bind(&file_path)
            .fetch_one(&self.pool)
            .await?
            .get::<i64, _>("id");

        let runtime_ticks = movie_runtime_ticks(movie);
        sqlx::query(
            "INSERT INTO media_items (
                id, library_id, parent_id, item_type, title, sort_title, overview,
                production_year, date_created, runtime_ticks, provider_tmdb_id,
                is_folder, is_virtual
             )
             VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0)
             ON CONFLICT(id) DO UPDATE SET
                library_id = excluded.library_id,
                item_type = excluded.item_type,
                title = excluded.title,
                sort_title = excluded.sort_title,
                overview = excluded.overview,
                production_year = excluded.production_year,
                runtime_ticks = excluded.runtime_ticks,
                provider_tmdb_id = excluded.provider_tmdb_id",
        )
        .bind(movie.id)
        .bind(DEFAULT_LIBRARY_ID)
        .bind("Movie")
        .bind(&movie.title)
        .bind(sort_title(&movie.title))
        .bind(&movie.overview)
        .bind(movie.year.map(|year| year as i32))
        .bind(movie.added_at)
        .bind(runtime_ticks)
        .bind(movie.tmdb_id)
        .execute(&self.pool)
        .await?;

        sqlx::query("DELETE FROM media_items_fts WHERE rowid = ?")
            .bind(movie.id)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO media_items_fts (rowid, title, original_title, overview, path_hint)
             VALUES (?, ?, NULL, ?, ?)",
        )
        .bind(movie.id)
        .bind(&movie.title)
        .bind(&movie.overview)
        .bind(&file_path)
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "INSERT INTO media_sources (
                item_id, file_id, source_type, protocol, path, container, size, runtime_ticks,
                supports_direct_play, supports_direct_stream, supports_transcoding
             )
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, 1, 1)
             ON CONFLICT(item_id, file_id) DO UPDATE SET
                source_type = excluded.source_type,
                protocol = excluded.protocol,
                path = excluded.path,
                container = excluded.container,
                size = excluded.size,
                runtime_ticks = excluded.runtime_ticks,
                supports_direct_play = excluded.supports_direct_play,
                supports_direct_stream = excluded.supports_direct_stream,
                supports_transcoding = excluded.supports_transcoding",
        )
        .bind(movie.id)
        .bind(file_id)
        .bind(movie_source_type(movie))
        .bind("File")
        .bind(&file_path)
        .bind(movie_container(movie))
        .bind(movie.file_size.map(|size| size as i64))
        .bind(runtime_ticks)
        .execute(&self.pool)
        .await?;

        if let Some(poster_url) = &movie.poster_url {
            sqlx::query(
                "INSERT INTO images (item_id, image_type, url, tag)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT(item_id, image_type) DO UPDATE SET
                    url = excluded.url,
                    tag = excluded.tag",
            )
            .bind(movie.id)
            .bind("Primary")
            .bind(poster_url)
            .bind(format!("poster-{}", movie.id))
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("DELETE FROM images WHERE item_id = ? AND image_type = ?")
                .bind(movie.id)
                .bind("Primary")
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
        self.sync_media_schema_for_movie_path(&file_path_str)
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
        self.sync_media_schema_for_movie_path(path).await?;
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
        self.update_movie_metadata_internal(
            id,
            poster_url,
            overview,
            tmdb_id,
            runtime_minutes,
            None,
        )
        .await
    }

    pub async fn update_movie_metadata_with_search_hint(
        &self,
        id: i64,
        poster_url: Option<String>,
        overview: Option<String>,
        tmdb_id: Option<i64>,
        runtime_minutes: Option<u16>,
        metadata_search_hint: &str,
    ) -> Result<(), sqlx::Error> {
        self.update_movie_metadata_internal(
            id,
            poster_url,
            overview,
            tmdb_id,
            runtime_minutes,
            Some(metadata_search_hint),
        )
        .await
    }

    async fn update_movie_metadata_internal(
        &self,
        id: i64,
        poster_url: Option<String>,
        overview: Option<String>,
        tmdb_id: Option<i64>,
        runtime_minutes: Option<u16>,
        metadata_search_hint: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE movies SET poster_url = ?, overview = ?, tmdb_id = ?, runtime_minutes = ?, metadata_search_hint = ? WHERE id = ?"
        )
        .bind(poster_url)
        .bind(overview)
        .bind(tmdb_id)
        .bind(runtime_minutes.map(|r| r as i32))
        .bind(metadata_search_hint)
        .bind(id)
        .execute(&self.pool)
        .await?;
        self.sync_media_schema_for_movie_id(id).await?;
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
        self.sync_media_schema_for_movie_id(id).await?;
        Ok(())
    }

    pub async fn get_movies_without_metadata(&self) -> Result<Vec<Movie>, sqlx::Error> {
        let rows = sqlx::query("SELECT id, title, year, file_path, poster_url, overview, tmdb_id, runtime_minutes, runtime_seconds, added_at, file_size FROM movies WHERE tmdb_id IS NULL")
            .fetch_all(&self.pool)
            .await?;

        let movies = rows.iter().map(map_row_to_movie).collect();
        Ok(movies)
    }

    pub async fn get_movies_requiring_metadata_scrape(&self) -> Result<Vec<Movie>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, title, year, file_path, poster_url, overview, tmdb_id, runtime_minutes, runtime_seconds, added_at, file_size
             FROM movies
             WHERE tmdb_id IS NULL
                OR metadata_search_hint IS NULL
                OR trim(metadata_search_hint) = ''",
        )
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
        let file_path = self
            .get_movie_by_id(id)
            .await
            .ok()
            .map(|movie| movie.file_path.to_string_lossy().into_owned());

        sqlx::query("DELETE FROM movies WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "DELETE FROM media_streams
             WHERE source_id IN (SELECT id FROM media_sources WHERE item_id = ?)",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        sqlx::query("DELETE FROM media_sources WHERE item_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM images WHERE item_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM user_item_data WHERE item_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM playback_sessions WHERE item_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM media_items WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM media_items_fts WHERE rowid = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if let Some(file_path) = file_path {
            sqlx::query(
                "DELETE FROM media_files
                 WHERE canonical_path = ?
                 AND id NOT IN (
                    SELECT file_id FROM media_sources WHERE file_id IS NOT NULL
                 )",
            )
            .bind(file_path)
            .execute(&self.pool)
            .await?;
        }
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
    async fn test_metadata_scrape_queue_includes_existing_metadata_without_search_hint_once() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        db.insert_movie(&Movie {
            id: 1,
            title: "Yu Yu Hakusho".to_string(),
            year: None,
            file_path: "/media/Anime/Yu Yu Hakusho/Yu Yu Hakusho S01E01.mkv".into(),
            poster_url: Some("http://image/live-action.jpg".to_string()),
            overview: Some("Wrong old metadata".to_string()),
            tmdb_id: Some(999),
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 1,
            file_size: None,
        })
        .await
        .unwrap();

        let pending = db.get_movies_requiring_metadata_scrape().await.unwrap();
        assert_eq!(pending.len(), 1);

        db.update_movie_metadata_with_search_hint(
            1,
            Some("http://image/anime.jpg".to_string()),
            Some("Correct anime metadata".to_string()),
            Some(200),
            Some(112),
            "anime",
        )
        .await
        .unwrap();

        let pending = db.get_movies_requiring_metadata_scrape().await.unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn test_media_schema_init_creates_core_tables_and_default_library() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        let tables = sqlx::query(
            "SELECT name FROM sqlite_master
             WHERE type IN ('table', 'virtual table') AND name IN (
                'schema_migrations',
                'libraries',
                'media_items',
                'media_files',
                'media_sources',
                'media_streams',
                'images',
                'user_item_data',
                'playback_sessions',
                'media_items_fts'
             )",
        )
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(tables.len(), 10);

        let library = sqlx::query("SELECT name, collection_type FROM libraries WHERE id = 1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(library.get::<String, _>("name"), "Default");
        assert_eq!(library.get::<String, _>("collection_type"), "mixed");

        let migration = sqlx::query("SELECT version FROM schema_migrations WHERE version = 1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(migration.get::<i64, _>("version"), 1);
    }

    #[tokio::test]
    async fn test_insert_movie_backfills_media_item_file_source_and_image() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        let movie = Movie {
            id: 0,
            title: "Inception".to_string(),
            year: Some(2010),
            file_path: std::path::PathBuf::from("/movies/inception.mp4"),
            poster_url: Some("https://example.com/inception.jpg".to_string()),
            overview: Some("Dreams inside dreams.".to_string()),
            tmdb_id: Some(27205),
            runtime_minutes: Some(148),
            runtime_seconds: Some(8_880),
            added_at: 1_717_896_000,
            file_size: Some(5_000_000),
        };

        db.insert_movie(&movie).await.unwrap();
        let inserted = db.get_movies().await.unwrap().pop().unwrap();

        let item = sqlx::query(
            "SELECT item_type, title, sort_title, production_year, runtime_ticks, provider_tmdb_id
             FROM media_items WHERE id = ?",
        )
        .bind(inserted.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(item.get::<String, _>("item_type"), "Movie");
        assert_eq!(item.get::<String, _>("title"), "Inception");
        assert_eq!(item.get::<String, _>("sort_title"), "inception");
        assert_eq!(item.get::<Option<i32>, _>("production_year"), Some(2010));
        assert_eq!(
            item.get::<Option<i64>, _>("runtime_ticks"),
            Some(88_800_000_000)
        );
        assert_eq!(item.get::<Option<i64>, _>("provider_tmdb_id"), Some(27205));

        let file = sqlx::query(
            "SELECT id, extension, size, is_available FROM media_files WHERE canonical_path = ?",
        )
        .bind("/movies/inception.mp4")
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            file.get::<Option<String>, _>("extension").as_deref(),
            Some("mp4")
        );
        assert_eq!(file.get::<Option<i64>, _>("size"), Some(5_000_000));
        assert_eq!(file.get::<i64, _>("is_available"), 0);

        let source = sqlx::query(
            "SELECT source_type, protocol, container, size, runtime_ticks
             FROM media_sources WHERE item_id = ? AND file_id = ?",
        )
        .bind(inserted.id)
        .bind(file.get::<i64, _>("id"))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(source.get::<String, _>("source_type"), "local");
        assert_eq!(source.get::<String, _>("protocol"), "File");
        assert_eq!(
            source.get::<Option<String>, _>("container").as_deref(),
            Some("mp4")
        );
        assert_eq!(source.get::<Option<i64>, _>("size"), Some(5_000_000));
        assert_eq!(
            source.get::<Option<i64>, _>("runtime_ticks"),
            Some(88_800_000_000)
        );

        let image = sqlx::query("SELECT image_type, url, tag FROM images WHERE item_id = ?")
            .bind(inserted.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(image.get::<String, _>("image_type"), "Primary");
        assert_eq!(
            image.get::<Option<String>, _>("url").as_deref(),
            Some("https://example.com/inception.jpg")
        );
        assert_eq!(
            image.get::<Option<String>, _>("tag").as_deref(),
            Some(format!("poster-{}", inserted.id).as_str())
        );

        let fts = sqlx::query("SELECT rowid FROM media_items_fts WHERE media_items_fts MATCH ?")
            .bind("Inception")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(fts.get::<i64, _>("rowid"), inserted.id);
    }

    #[tokio::test]
    async fn test_media_schema_backfill_is_idempotent_and_updates_metadata() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        db.insert_movie(&Movie {
            id: 0,
            title: "Dirty Title".to_string(),
            year: None,
            file_path: std::path::PathBuf::from("/media/fate-zero.strm"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        })
        .await
        .unwrap();

        let movie = db.get_movies().await.unwrap().pop().unwrap();
        db.update_movie_metadata(
            movie.id,
            Some("https://example.com/fate.jpg".to_string()),
            Some("A battle for the grail.".to_string()),
            Some(45845),
            Some(24),
        )
        .await
        .unwrap();
        db.backfill_media_schema_from_movies().await.unwrap();

        let item_count = sqlx::query("SELECT COUNT(*) AS count FROM media_items")
            .fetch_one(&db.pool)
            .await
            .unwrap()
            .get::<i64, _>("count");
        let source_count = sqlx::query("SELECT COUNT(*) AS count FROM media_sources")
            .fetch_one(&db.pool)
            .await
            .unwrap()
            .get::<i64, _>("count");
        assert_eq!(item_count, 1);
        assert_eq!(source_count, 1);

        let item = sqlx::query(
            "SELECT overview, provider_tmdb_id, runtime_ticks FROM media_items WHERE id = ?",
        )
        .bind(movie.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            item.get::<Option<String>, _>("overview").as_deref(),
            Some("A battle for the grail.")
        );
        assert_eq!(item.get::<Option<i64>, _>("provider_tmdb_id"), Some(45845));
        assert_eq!(
            item.get::<Option<i64>, _>("runtime_ticks"),
            Some(14_400_000_000)
        );

        let source = sqlx::query("SELECT source_type, container FROM media_sources")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(source.get::<String, _>("source_type"), "strm");
        assert_eq!(source.get::<Option<String>, _>("container"), None);
    }

    #[tokio::test]
    async fn test_delete_movie_removes_backfilled_media_item() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        db.insert_movie(&Movie {
            id: 0,
            title: "Temporary".to_string(),
            year: None,
            file_path: std::path::PathBuf::from("/media/temp.mkv"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        })
        .await
        .unwrap();

        let movie_id = db.get_movies().await.unwrap()[0].id;
        db.delete_movie(movie_id).await.unwrap();

        let item_count = sqlx::query("SELECT COUNT(*) AS count FROM media_items WHERE id = ?")
            .bind(movie_id)
            .fetch_one(&db.pool)
            .await
            .unwrap()
            .get::<i64, _>("count");
        let source_count =
            sqlx::query("SELECT COUNT(*) AS count FROM media_sources WHERE item_id = ?")
                .bind(movie_id)
                .fetch_one(&db.pool)
                .await
                .unwrap()
                .get::<i64, _>("count");
        let file_count =
            sqlx::query("SELECT COUNT(*) AS count FROM media_files WHERE canonical_path = ?")
                .bind("/media/temp.mkv")
                .fetch_one(&db.pool)
                .await
                .unwrap()
                .get::<i64, _>("count");
        assert_eq!(item_count, 0);
        assert_eq!(source_count, 0);
        assert_eq!(file_count, 0);
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
