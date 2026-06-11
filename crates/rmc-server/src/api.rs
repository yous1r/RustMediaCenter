use anyhow::Context;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::media_tree::MediaTree;
use crate::transcode::{TranscodeManager, TranscodeQuality};
use rmc_core::api_types::{
    LibraryItem, LibraryItemsResponse, LibrarySummary, LoginRequest, LoginResponse,
};
use rmc_core::models::{Movie, User};
use std::sync::OnceLock;
use tokio_util::io::ReaderStream;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

pub type AppState = Database;

const BASE_TRANSCODE_DIR: &str = "/tmp/rmc-transcode";
const M3U8_WAIT_RETRIES: usize = 300;
const M3U8_WAIT_DELAY_MS: u64 = 100;

fn get_config_path() -> String {
    std::env::var("RMC_CONFIG").unwrap_or_else(|_| "config.toml".to_string())
}

static TRANSCODE_MANAGER: OnceLock<TranscodeManager> = OnceLock::new();

pub fn get_transcode_manager() -> &'static TranscodeManager {
    TRANSCODE_MANAGER.get_or_init(|| TranscodeManager::new(BASE_TRANSCODE_DIR.to_string()))
}

async fn wait_for_playlist_with_retry(
    transcode: &TranscodeManager,
    movie_id: i64,
    m3u8_path: &std::path::Path,
    retries: usize,
    delay_ms: u64,
) -> Result<(), crate::error::AppError> {
    let log_path = transcode.get_log_path(movie_id);
    let mut last_status = "Spawning".to_string();

    for _ in 0..retries {
        if m3u8_path.exists() {
            return Ok(());
        }

        match transcode.get_session_status(movie_id).await {
            Some(status) => last_status = status,
            None => {
                if m3u8_path.exists() {
                    return Ok(());
                }

                return Err(crate::error::AppError::Internal(anyhow::anyhow!(
                    "Transcode session ended before master.m3u8 was generated. See {}",
                    log_path
                )));
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
    }

    if m3u8_path.exists() {
        return Ok(());
    }

    Err(crate::error::AppError::Internal(anyhow::anyhow!(
        "Timeout waiting for master.m3u8 after {} ms (session_status={}). See {}",
        retries as u64 * delay_ms,
        last_status,
        log_path
    )))
}

async fn wait_for_playlist(
    transcode: &TranscodeManager,
    movie_id: i64,
    m3u8_path: &std::path::Path,
) -> Result<(), crate::error::AppError> {
    wait_for_playlist_with_retry(
        transcode,
        movie_id,
        m3u8_path,
        M3U8_WAIT_RETRIES,
        M3U8_WAIT_DELAY_MS,
    )
    .await
}

async fn resolve_movie_input(
    file_path: &std::path::Path,
) -> Result<(String, Option<String>), crate::error::AppError> {
    if file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .eq_ignore_ascii_case("strm")
    {
        let content = tokio::fs::read_to_string(file_path)
            .await
            .context("Failed to read strm file")?;
        let source =
            crate::strm::StrmParser::parse(&content).context("Invalid strm file content")?;
        let headers = source.ffmpeg_header_value();
        Ok((source.url, headers))
    } else {
        Ok((file_path.to_string_lossy().into_owned(), None))
    }
}

fn require_auth(headers: &HeaderMap) -> Result<crate::auth::Claims, crate::error::AppError> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            crate::error::AppError::Unauthorized("Missing Authorization header".to_string())
        })?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::AppError::Unauthorized("Invalid bearer token".to_string()))?;

    crate::auth::verify_jwt(token)
        .map_err(|_| crate::error::AppError::Unauthorized("Invalid or expired token".to_string()))
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PublicServerConfig {
    pub port: u16,
    pub media_dirs: Vec<String>,
    pub db_path: String,
    pub tmdb_api_key_present: bool,
    pub tmdb_proxy_url: Option<String>,
    pub tmdb_api_base: Option<String>,
}

impl From<&crate::config::ServerConfig> for PublicServerConfig {
    fn from(value: &crate::config::ServerConfig) -> Self {
        Self {
            port: value.port,
            media_dirs: value.media_dirs.clone(),
            db_path: value.db_path.clone(),
            tmdb_api_key_present: value
                .tmdb_api_key
                .as_ref()
                .map(|key| !key.trim().is_empty())
                .unwrap_or(false),
            tmdb_proxy_url: value.tmdb_proxy_url.clone(),
            tmdb_api_base: value.tmdb_api_base.clone(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UpdateServerConfigPayload {
    pub port: u16,
    pub media_dirs: Vec<String>,
    pub db_path: String,
    pub tmdb_api_key: Option<String>,
    pub replace_tmdb_api_key: bool,
    pub tmdb_proxy_url: Option<String>,
    pub tmdb_api_base: Option<String>,
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub fn app_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/health", get(|| async { "OK" }))
        .merge(crate::emby::router())
        .nest("/emby", crate::emby::router())
        .nest("/jellyfin", crate::emby::router())
        .route("/api/v1/movies", get(list_movies))
        .route("/users", get(get_users))
        .route("/api/v1/movies/:id/direct", get(direct_play))
        .route("/api/v1/playback/progress", post(report_progress))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/libraries", get(get_libraries))
        .route("/api/v1/library/items", get(get_library_items))
        .route("/api/v1/library/items/:id", get(get_library_item_by_id))
        .route("/api/v1/movies/:id", get(get_movie_by_id))
        .route("/api/v1/playback/start", post(playback_start))
        .route("/api/v1/movies/:id/hls/master.m3u8", get(hls_playlist))
        .route("/api/v1/movies/:id/hls/*segment", get(hls_segment))
        .route("/api/v1/movies/:id/stream.mp4", get(stream_transcode))
        .route("/api/v1/config", get(get_config).post(update_config))
        .route("/api/v1/scan", post(trigger_scan))
        .nest_service("/", ServeDir::new("web-client"))
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
}

pub async fn login(
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, crate::error::AppError> {
    if !crate::auth::validate_admin_login(&payload.username, &payload.password) {
        return Err(crate::error::AppError::Unauthorized(
            "Invalid credentials".to_string(),
        ));
    }

    let token = crate::auth::create_jwt(&payload.username).context("Failed to create jwt")?;

    Ok(Json(LoginResponse { token }))
}

async fn load_media_tree(db: &Database) -> Result<MediaTree, crate::error::AppError> {
    let movies = db
        .get_available_movies()
        .await
        .context("Failed to load movies for library tree")?;
    Ok(MediaTree::from_movies(movies))
}

pub async fn get_libraries(
    State(db): State<AppState>,
) -> Result<Json<Vec<LibrarySummary>>, crate::error::AppError> {
    let tree = load_media_tree(&db).await?;
    Ok(Json(tree.summaries()))
}

#[derive(Debug, Deserialize)]
pub struct LibraryItemsQuery {
    pub parent_id: Option<String>,
    pub search_term: Option<String>,
    pub start_index: Option<usize>,
    pub limit: Option<usize>,
}

pub async fn get_library_items(
    State(db): State<AppState>,
    Query(query): Query<LibraryItemsQuery>,
) -> Result<Json<LibraryItemsResponse>, crate::error::AppError> {
    let tree = load_media_tree(&db).await?;
    let response = tree.paged_items(
        query.parent_id.as_deref(),
        query.search_term.as_deref(),
        query.start_index.unwrap_or(0),
        query.limit,
    );
    Ok(Json(response))
}

pub async fn get_library_item_by_id(
    State(db): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<LibraryItem>, crate::error::AppError> {
    let tree = load_media_tree(&db).await?;
    let item = tree.item(&id).ok_or_else(|| {
        crate::error::AppError::NotFound(format!("Library item {} not found", id))
    })?;
    Ok(Json(item))
}

pub async fn get_movie_by_id(
    State(db): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Movie>, crate::error::AppError> {
    let movie = crate::media_probe::load_movie_with_runtime(&db, id)
        .await
        .map_err(|e| {
            if let Some(sqlx::Error::RowNotFound) = e.downcast_ref::<sqlx::Error>() {
                crate::error::AppError::NotFound(format!("Movie id {} not found", id))
            } else {
                crate::error::AppError::Internal(e)
            }
        })?;
    Ok(Json(movie))
}

pub async fn playback_start() -> &'static str {
    "Playback Started"
}

#[derive(Deserialize)]
pub struct MovieQuery {
    pub q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StreamTranscodeQuery {
    pub start_time: Option<f64>,
    pub quality: Option<String>,
}

async fn list_movies(
    State(db): State<AppState>,
    Query(query): Query<MovieQuery>,
) -> Result<Json<Vec<Movie>>, crate::error::AppError> {
    let movies = if let Some(q) = query.q {
        db.search_available_movies(&q)
            .await
            .context("Failed to search movies")?
    } else {
        db.get_available_movies()
            .await
            .context("Failed to get movies")?
    };

    Ok(Json(movies))
}

pub async fn direct_play(
    State(db): State<AppState>,
    Path(id): Path<i64>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<impl axum::response::IntoResponse, crate::error::AppError> {
    let movie = db
        .get_available_movie_by_id(id)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                crate::error::AppError::NotFound(format!("Movie id {} not found", id))
            }
            _ => crate::error::AppError::Internal(e.into()),
        })?;

    let file_path = movie.file_path;
    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");

    if extension.eq_ignore_ascii_case("strm") {
        let content = tokio::fs::read_to_string(&file_path)
            .await
            .context("Failed to read strm file")?;
        if let Some(source) = crate::strm::StrmParser::parse(&content) {
            use axum::http::{header, StatusCode};
            use axum::response::{IntoResponse, Response};
            let response = Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, source.url)
                .body(axum::body::Body::empty())
                .context("Failed to build response")?;
            Ok(response.into_response())
        } else {
            Err(crate::error::AppError::Internal(anyhow::anyhow!(
                "Invalid strm file content"
            )))
        }
    } else {
        use axum::response::IntoResponse;
        use tower::ServiceExt;
        match tower_http::services::ServeFile::new(file_path)
            .oneshot(req)
            .await
        {
            Ok(res) => Ok(res.into_response()),
            Err(_) => Err(crate::error::AppError::Internal(anyhow::anyhow!(
                "ServeFile failed"
            ))),
        }
    }
}

pub async fn hls_playlist(
    State(db): State<AppState>,
    Path(id): Path<i64>,
) -> Result<impl axum::response::IntoResponse, crate::error::AppError> {
    let movie = db
        .get_available_movie_by_id(id)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                crate::error::AppError::NotFound(format!("Movie id {} not found", id))
            }
            _ => crate::error::AppError::Internal(e.into()),
        })?;

    let (input_path, input_headers) = resolve_movie_input(&movie.file_path).await?;

    let transcode = get_transcode_manager();
    transcode
        .start_transcode_session_with_headers(id, &input_path, input_headers.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!(e))
        .context("Failed to start transcode")?;

    let m3u8_path = transcode.get_m3u8_path(id);
    let m3u8_path_buf = std::path::PathBuf::from(&m3u8_path);

    wait_for_playlist(transcode, id, &m3u8_path_buf).await?;

    transcode.touch_session(id).await;

    let content = tokio::fs::read_to_string(&m3u8_path)
        .await
        .context("Failed to read master.m3u8")?;

    let response = axum::response::Response::builder()
        .header("Content-Type", "application/vnd.apple.mpegurl")
        .body(axum::body::Body::from(content))
        .context("Failed to build response")?;
    Ok(response)
}

pub async fn stream_transcode(
    State(db): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<StreamTranscodeQuery>,
) -> Result<impl axum::response::IntoResponse, crate::error::AppError> {
    let movie = db
        .get_available_movie_by_id(id)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                crate::error::AppError::NotFound(format!("Movie id {} not found", id))
            }
            _ => crate::error::AppError::Internal(e.into()),
        })?;

    let (input_path, input_headers) = resolve_movie_input(&movie.file_path).await?;
    let start_time_secs = query
        .start_time
        .filter(|value| value.is_finite() && *value >= 0.0);
    let quality = TranscodeQuality::from_label(query.quality.as_deref());
    let transcode = get_transcode_manager();
    let session = transcode
        .start_stream_transcode(
            id,
            &input_path,
            input_headers.as_deref(),
            start_time_secs,
            quality,
        )
        .await
        .map_err(|e| anyhow::anyhow!(e))
        .context("Failed to start streaming transcode")?;

    let crate::transcode::StreamTranscodeSession {
        mut child,
        stdout,
        cleanup_dir,
    } = session;

    tokio::spawn(async move {
        let wait_result = child.wait().await;
        if let Err(err) = tokio::fs::remove_dir_all(&cleanup_dir).await {
            tracing::warn!(movie_id = id, error = %err, "Failed to clean up streaming transcode directory");
        }
        match wait_result {
            Ok(status) => {
                tracing::info!(movie_id = id, status = ?status, "Streaming transcode session ended");
            }
            Err(err) => {
                tracing::warn!(movie_id = id, error = %err, "Failed waiting for streaming transcode session");
            }
        }
    });

    let stream = ReaderStream::new(stdout);
    let response = Response::builder()
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .context("Failed to build streaming transcode response")?;
    Ok(response)
}

pub async fn hls_segment(
    Path((id, segment)): Path<(i64, String)>,
) -> Result<impl axum::response::IntoResponse, crate::error::AppError> {
    let transcode = get_transcode_manager();
    transcode.touch_session(id).await;

    let segment_clean = segment.trim_start_matches('/');
    if segment_clean.contains("..") || segment_clean.contains('/') || segment_clean.contains('\\') {
        return Err(crate::error::AppError::NotFound(
            "Invalid segment path".to_string(),
        ));
    }

    let file_path = std::path::PathBuf::from(BASE_TRANSCODE_DIR)
        .join(id.to_string())
        .join(segment_clean);

    if !file_path.exists() {
        return Err(crate::error::AppError::NotFound(format!(
            "Segment {} not found",
            segment_clean
        )));
    }

    let content = tokio::fs::read(file_path)
        .await
        .context("Failed to read segment")?;

    let response = axum::response::Response::builder()
        .header("Content-Type", "video/MP2T")
        .body(axum::body::Body::from(content))
        .context("Failed to build response")?;
    Ok(response)
}

pub async fn get_config(
    headers: HeaderMap,
) -> Result<Json<PublicServerConfig>, crate::error::AppError> {
    let _claims = require_auth(&headers)?;
    let config = crate::config::ServerConfig::load_from(&get_config_path())
        .context("Failed to load config")?;
    Ok(Json(PublicServerConfig::from(&config)))
}

pub async fn update_config(
    headers: HeaderMap,
    Json(payload): Json<UpdateServerConfigPayload>,
) -> Result<Json<PublicServerConfig>, crate::error::AppError> {
    let _claims = require_auth(&headers)?;

    let mut config = crate::config::ServerConfig::load_from(&get_config_path())
        .context("Failed to load existing config")?;
    config.port = payload.port;
    config.media_dirs = payload.media_dirs;
    config.db_path = payload.db_path;
    config.tmdb_proxy_url = normalize_optional_string(payload.tmdb_proxy_url);
    config.tmdb_api_base = normalize_optional_string(payload.tmdb_api_base);
    if payload.replace_tmdb_api_key {
        config.tmdb_api_key = normalize_optional_string(payload.tmdb_api_key);
    }

    config
        .save_to(&get_config_path())
        .context("Failed to save config")?;
    Ok(Json(PublicServerConfig::from(&config)))
}

pub async fn trigger_scan(
    headers: HeaderMap,
    State(db): State<AppState>,
) -> Result<&'static str, crate::error::AppError> {
    let _claims = require_auth(&headers)?;
    let config = crate::config::ServerConfig::load_from(&get_config_path())
        .context("Failed to load config")?;

    let db_clone = db.clone();
    tokio::spawn(async move {
        tracing::info!("Starting background media scan...");
        let scanner = crate::scanner::MediaScanner::new(db_clone.clone());
        for dir in &config.media_dirs {
            tracing::info!("Scanning directory: {}", dir);
            if let Err(e) = scanner.scan_directory(dir).await {
                tracing::error!("Failed to scan directory {}: {:?}", dir, e);
            }
        }

        if let Some(api_key) = &config.tmdb_api_key {
            if !api_key.trim().is_empty() {
                tracing::info!("Starting background metadata scraping...");
                let scraper = crate::scraper::TmdbScraper::new(
                    api_key.clone(),
                    config.tmdb_proxy_url.clone(),
                    config.tmdb_api_base.clone(),
                );
                match db_clone.get_movies_without_metadata().await {
                    Ok(movies) => {
                        for movie in movies {
                            let (scrape_title, scrape_year) = movie
                                .file_path
                                .file_stem()
                                .and_then(|stem| stem.to_str())
                                .map(crate::scanner::parse_filename)
                                .unwrap_or_else(|| (movie.title.clone(), movie.year));
                            let scrape_year = scrape_year.or(movie.year);

                            tracing::info!(
                                movie_id = movie.id,
                                stored_title = %movie.title,
                                query_title = %scrape_title,
                                query_year = ?scrape_year,
                                "Scraping metadata for movie"
                            );
                            match scraper
                                .fetch_movie_metadata(&scrape_title, scrape_year)
                                .await
                            {
                                Ok(metadata) => {
                                    if let Err(e) = db_clone
                                        .update_movie_metadata(
                                            movie.id,
                                            metadata.poster_url,
                                            metadata.overview,
                                            metadata.tmdb_id,
                                            metadata.runtime_minutes,
                                        )
                                        .await
                                    {
                                        tracing::error!(
                                            "Failed to update database metadata for movie {}: {:?}",
                                            movie.title,
                                            e
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        stored_title = %movie.title,
                                        query_title = %scrape_title,
                                        query_year = ?scrape_year,
                                        "Failed to fetch TMDB metadata: {:?}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to query movies without metadata: {:?}", e);
                    }
                }
            }
        }
        tracing::info!("Background media scan and scraping completed.");
    });

    Ok("Scan Started")
}

pub async fn get_users() -> Json<Vec<User>> {
    Json(vec![User {
        id: 1,
        username: "admin".to_string(),
    }])
}

pub async fn report_progress() -> &'static str {
    "Progress Saved"
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::AUTHORIZATION;
    use axum::{body::Body, http::Request};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        entries: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn set_many(entries: &[(&'static str, &str)]) -> Self {
            let lock = ENV_MUTEX.lock().unwrap();
            let mut previous = Vec::with_capacity(entries.len());
            for (key, val) in entries {
                previous.push((*key, std::env::var(key).ok()));
                std::env::set_var(key, val);
            }
            Self {
                _lock: lock,
                entries: previous,
            }
        }

        fn set_owned(entries: &[(&'static str, String)]) -> Self {
            let lock = ENV_MUTEX.lock().unwrap();
            let mut previous = Vec::with_capacity(entries.len());
            for (key, val) in entries {
                previous.push((*key, std::env::var(key).ok()));
                std::env::set_var(key, val);
            }
            Self {
                _lock: lock,
                entries: previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, previous) in &self.entries {
                if let Some(value) = previous {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    async fn login_and_get_token(app: Router) -> String {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&LoginRequest {
                            username: "admin".to_string(),
                            password: "admin".to_string(),
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let login_response: LoginResponse = serde_json::from_slice(&body).unwrap();
        login_response.token
    }

    async fn spawn_tmdb_query_recorder() -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = Arc::clone(&requests);

        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = vec![0; 4096];
            let Ok(read_len) = socket.read(&mut buffer).await else {
                return;
            };
            let request = String::from_utf8_lossy(&buffer[..read_len]).to_string();
            if let Some(path) = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
            {
                requests_clone.lock().unwrap().push(path.to_string());
            }
            let body = r#"{"results":[]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });

        (format!("http://{}", addr), requests)
    }

    #[tokio::test]
    async fn test_health_check() {
        let db = crate::db::Database::new("sqlite::memory:").await.unwrap();
        let app = app_router(db);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "OK");
    }

    #[tokio::test]
    async fn test_get_movies_api() {
        use crate::db::Database;
        use rmc_core::models::Movie;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let movie_path = temp_dir.path().join("matrix.mp4");
        std::fs::write(&movie_path, b"movie").unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Matrix".to_string(),
            year: Some(1999),
            file_path: movie_path,
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

        let app = app_router(db);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/movies")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("Matrix"));
    }

    #[tokio::test]
    async fn test_direct_play_range_header() {
        use axum::body::Body;
        use axum::http::Request;
        use rmc_core::models::Movie;
        use tower::ServiceExt;

        let db = crate::db::Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        // 使用 tempfile 创建一个实际存在的临时文件，并向其写入测试内容
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("movie.mp4");
        std::fs::write(&file_path, "Hello, world! Range test content.").unwrap();

        db.insert_movie(&Movie {
            id: 1,
            title: "Mock Movie".to_string(),
            year: Some(2020),
            file_path,
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
        let app = super::app_router(db);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/movies/1/direct")
                    .header("Range", "bytes=0-4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // 断言返回状态码为 206 Partial Content，且返回前 5 个字节内容
        assert_eq!(response.status().as_u16(), 206);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Hello");
    }

    #[tokio::test]
    async fn test_playback_progress_api() {
        let app = app_router(crate::db::Database::new("sqlite::memory:").await.unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/playback/progress")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_wait_for_playlist_allows_slow_hls_startup_while_session_runs() {
        let temp_dir_fixture = tempfile::tempdir().unwrap();
        let transcode_dir = temp_dir_fixture.path();
        let mock_script = transcode_dir.join("mock_ffmpeg_delayed_playlist.sh");

        std::fs::write(
            &mock_script,
            "#!/bin/sh\nsleep 1.2\nfor arg in \"$@\"; do out=\"$arg\"; done\ntouch \"$out\"\nsleep 5\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&mock_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_script, perms).unwrap();

        let _guard = EnvGuard::set_many(&[
            ("RMC_FFMPEG_CMD", mock_script.to_string_lossy().as_ref()),
            ("RMC_TRANSCODE_MODE", "software"),
        ]);

        let manager = TranscodeManager::new(transcode_dir.to_string_lossy().to_string());
        manager
            .start_transcode_session(77, "http://invalid/path.mp4")
            .await
            .unwrap();

        let m3u8_path = transcode_dir.join("77").join("master.m3u8");
        wait_for_playlist_with_retry(&manager, 77, &m3u8_path, 20, 100)
            .await
            .expect("playlist should be accepted once it appears while session is still alive");

        manager.stop_transcode_session(77).await;
    }

    #[tokio::test]
    async fn test_auth_login() {
        use crate::db::Database;

        let _guard = EnvGuard::set_many(&[
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "test-secret"),
        ]);

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&LoginRequest {
                            username: "admin".to_string(),
                            password: "admin".to_string(),
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let login_response: LoginResponse = serde_json::from_slice(&body).unwrap();
        assert!(!login_response.token.is_empty());
    }

    #[tokio::test]
    async fn test_protected_config_requires_auth() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 401);
    }

    #[tokio::test]
    async fn test_trigger_scan_requires_auth() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/scan")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 401);
    }

    #[tokio::test]
    async fn test_trigger_scan_scrapes_with_filename_cleaned_title_for_existing_dirty_rows() {
        use crate::config::ServerConfig;
        use crate::db::Database;
        use rmc_core::models::Movie;

        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let media_path = temp_dir
            .path()
            .join("[VCB-Studio] Fate Stay Night 2006 [01][Ma10p_1080p][x265_flac].mkv");
        std::fs::write(&media_path, b"video").unwrap();

        let (tmdb_base, requests) = spawn_tmdb_query_recorder().await;
        ServerConfig {
            port: 8000,
            media_dirs: vec![temp_dir.path().to_string_lossy().into_owned()],
            db_path: "ignored.db".to_string(),
            tmdb_api_key: Some("dummy".to_string()),
            tmdb_proxy_url: None,
            tmdb_api_base: Some(tmdb_base),
        }
        .save_to(&config_path)
        .unwrap();

        let config_path_string = config_path.to_string_lossy().into_owned();
        let _guard = EnvGuard::set_owned(&[
            ("RMC_CONFIG", config_path_string),
            ("RMC_ADMIN_USERNAME", "admin".to_string()),
            ("RMC_ADMIN_PASSWORD", "admin".to_string()),
            ("RMC_JWT_SECRET", "test-secret".to_string()),
        ]);

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "VCB-Studio Fate Stay Night".to_string(),
            year: Some(2006),
            file_path: media_path,
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 1,
            file_size: None,
        })
        .await
        .unwrap();

        let app = super::app_router(db);
        let token = login_and_get_token(app.clone()).await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/scan")
                    .header(AUTHORIZATION, format!("Bearer {}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !requests.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        let request_path = requests.lock().unwrap()[0].clone();
        assert!(
            request_path.contains("query=Fate+Stay+Night")
                || request_path.contains("query=Fate%20Stay%20Night"),
            "unexpected TMDB request path: {}",
            request_path
        );
        assert!(request_path.contains("year=2006"));
        assert!(!request_path.contains("VCB-Studio"));
    }

    #[tokio::test]
    async fn test_get_libraries() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/libraries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_get_library_items_groups_series() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use rmc_core::models::Movie;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let episode_one_path = temp_dir
            .path()
            .join("shows")
            .join("Tiny World")
            .join("Tiny.World.S01E01.mkv");
        let episode_two_path = temp_dir
            .path()
            .join("shows")
            .join("Tiny World")
            .join("Tiny.World.S01E02.mkv");
        std::fs::create_dir_all(episode_one_path.parent().unwrap()).unwrap();
        std::fs::write(&episode_one_path, b"episode-one").unwrap();
        std::fs::write(&episode_two_path, b"episode-two").unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Tiny World".to_string(),
            year: Some(2020),
            file_path: episode_one_path,
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
        db.insert_movie(&Movie {
            id: 2,
            title: "Tiny World".to_string(),
            year: Some(2020),
            file_path: episode_two_path,
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 1,
            file_size: None,
        })
        .await
        .unwrap();

        let app = super::app_router(db);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/library/items")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let items: LibraryItemsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(items.total_record_count, 1);
        assert_eq!(
            items.items[0].kind,
            rmc_core::api_types::LibraryItemKind::Series
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/library/items?parent_id={}",
                        items.items[0].id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_get_library_items_skips_missing_local_files() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use rmc_core::models::Movie;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let existing_path = temp_dir.path().join("existing.mp4");
        std::fs::write(&existing_path, b"video").unwrap();

        db.insert_movie(&Movie {
            id: 1,
            title: "Existing Movie".to_string(),
            year: Some(2024),
            file_path: existing_path,
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: Some(5),
        })
        .await
        .unwrap();

        db.insert_movie(&Movie {
            id: 2,
            title: "Missing Movie".to_string(),
            year: Some(2024),
            file_path: temp_dir.path().join("missing.mp4"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 1,
            file_size: None,
        })
        .await
        .unwrap();

        let app = super::app_router(db.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/library/items")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let items: LibraryItemsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(items.total_record_count, 1);
        assert_eq!(items.items[0].title, "Existing Movie");

        let remaining = db.get_movies().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].title, "Existing Movie");
    }

    #[tokio::test]
    async fn test_stream_transcode_missing_file_returns_not_found() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use rmc_core::models::Movie;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Missing Movie".to_string(),
            year: Some(2024),
            file_path: std::path::PathBuf::from("/tmp/rmc-does-not-exist.mp4"),
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

        let app = super::app_router(db);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/movies/1/stream.mp4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 404);
    }

    #[tokio::test]
    async fn test_get_movie_by_id_backfills_runtime_seconds() {
        use crate::db::Database;
        use rmc_core::models::Movie;

        let temp_dir = tempfile::tempdir().unwrap();
        let ffprobe_path = temp_dir.path().join("fake_ffprobe.sh");
        let movie_path = temp_dir.path().join("movie.mp4");
        std::fs::write(&movie_path, b"movie").unwrap();
        std::fs::write(
            &ffprobe_path,
            "#!/bin/sh\nprintf '{\"format\":{\"duration\":\"187.4\"}}\\n'\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&ffprobe_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&ffprobe_path, perms).unwrap();

        let ffprobe_cmd = Box::leak(ffprobe_path.to_string_lossy().into_owned().into_boxed_str());
        let _guard = EnvGuard::set_many(&[("RMC_FFPROBE_CMD", ffprobe_cmd)]);

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Runtime Movie".to_string(),
            year: Some(2024),
            file_path: movie_path,
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

        let app = super::app_router(db.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/movies/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let movie: Movie = serde_json::from_slice(&body).unwrap();
        assert_eq!(movie.runtime_seconds, Some(187));
        assert_eq!(
            db.get_movie_by_id(1).await.unwrap().runtime_seconds,
            Some(187)
        );
    }

    #[tokio::test]
    async fn test_get_movie_by_id() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use rmc_core::models::Movie;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let movie_path = temp_dir.path().join("mock.mp4");
        std::fs::write(&movie_path, b"movie").unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Mock Movie".to_string(),
            year: Some(2020),
            file_path: movie_path,
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
        let app = super::app_router(db);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/movies/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_playback_start() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/playback/start")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_health_check_tracing() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_cors_headers() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/v1/movies")
                    .header("Origin", "http://localhost:5173")
                    .header("Access-Control-Request-Method", "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert!(response
            .headers()
            .contains_key("access-control-allow-origin"));
    }

    #[tokio::test]
    async fn test_api_search_movies() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let movie_path = temp_dir.path().join("inception.mkv");
        std::fs::write(&movie_path, b"movie").unwrap();
        db.insert_movie(&rmc_core::models::Movie {
            id: 0,
            title: "Inception".to_string(),
            year: Some(2010),
            file_path: movie_path,
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

        let app = super::app_router(db);

        // 测 GET /api/v1/movies?q=Inception
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/movies?q=Inception")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Inception"));
    }

    #[tokio::test]
    async fn test_direct_play_strm_302() {
        use crate::db::Database;
        use axum::body::Body;
        use axum::http::Request;
        use rmc_core::models::Movie;
        use tower::ServiceExt;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        // 创建临时 .strm 文件
        let temp_dir = tempfile::tempdir().unwrap();
        let strm_file_path = temp_dir.path().join("movie.strm");
        std::fs::write(&strm_file_path, "http://example.com/stream.mkv").unwrap();

        db.insert_movie(&Movie {
            id: 1,
            title: "Strm Movie".to_string(),
            year: Some(2024),
            file_path: strm_file_path.clone(),
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

        let app = super::app_router(db);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/movies/1/direct")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status().as_u16(), 302);
        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "http://example.com/stream.mkv");
    }

    #[tokio::test]
    async fn test_config_api_endpoints() -> Result<(), Box<dyn std::error::Error>> {
        use crate::config::ServerConfig;

        // Create a temp file path for configuration isolation
        let temp_dir = tempfile::tempdir()?;
        let config_path = temp_dir.path().join("test_config.toml");

        // Thread-safe isolation for environment variable mutation
        let _guard = EnvGuard::set_many(&[
            (
                "RMC_CONFIG",
                config_path.to_str().ok_or("Invalid path string")?,
            ),
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "test-secret"),
        ]);

        // Ensure default config file is created/loaded
        let mut default_config = ServerConfig::default();
        default_config.tmdb_api_key = Some("existing-secret".to_string());
        default_config.save_to(&config_path)?;

        let db = Database::new("sqlite::memory:").await?;
        db.init_schema().await?;
        let app = super::app_router(db);
        let token = login_and_get_token(app.clone()).await;

        // 1. Send GET request to /api/v1/config
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/config")
                    .header(AUTHORIZATION, format!("Bearer {}", token))
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body_config: PublicServerConfig = serde_json::from_slice(&body)?;

        // Verify the secret is not leaked by the read endpoint
        assert!(body_config.tmdb_api_key_present);
        assert!(body_config.tmdb_proxy_url.is_none());
        assert!(body_config.tmdb_api_base.is_none());
        let body_json: serde_json::Value = serde_json::from_slice(&body)?;
        assert!(body_json.get("tmdb_api_key").is_none());

        // 2. Send POST request to /api/v1/config with updated values
        let updated_config = UpdateServerConfigPayload {
            port: 9000,
            db_path: "rmc_test.db".to_string(),
            media_dirs: vec!["/media1".to_string(), "/media2".to_string()],
            tmdb_api_key: None,
            replace_tmdb_api_key: false,
            tmdb_proxy_url: Some("http://127.0.0.1:7890".to_string()),
            tmdb_api_base: Some("https://api.tmdb.org".to_string()),
        };

        let post_body = serde_json::to_vec(&updated_config)?;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/config")
                    .header(AUTHORIZATION, format!("Bearer {}", token))
                    .header("content-type", "application/json")
                    .body(Body::from(post_body))?,
            )
            .await?;

        assert_eq!(response.status(), 200);

        // 3. Send GET request again to verify updated values
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/config")
                    .header(AUTHORIZATION, format!("Bearer {}", token))
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body_config: PublicServerConfig = serde_json::from_slice(&body)?;

        assert_eq!(body_config.port, 9000);
        assert_eq!(body_config.db_path, "rmc_test.db");
        assert_eq!(
            body_config.tmdb_proxy_url.as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            body_config.tmdb_api_base.as_deref(),
            Some("https://api.tmdb.org")
        );

        // 4. Verify that the file on disk actually contains the updated configuration
        let file_config = ServerConfig::load_from(&config_path)?;
        assert_eq!(file_config.tmdb_api_key.as_deref(), Some("existing-secret"));
        assert_eq!(file_config.port, 9000);
        assert_eq!(file_config.db_path, "rmc_test.db");
        assert_eq!(
            file_config.tmdb_proxy_url.as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            file_config.tmdb_api_base.as_deref(),
            Some("https://api.tmdb.org")
        );

        Ok(())
    }
}
