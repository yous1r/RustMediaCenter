use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use anyhow::Context;

use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tokio_util::io::ReaderStream;
use crate::db::Database;
use rmc_core::api_types::{LoginRequest, LoginResponse};
use rmc_core::models::{Movie, User};
use std::sync::OnceLock;
use crate::transcode::TranscodeManager;

pub type AppState = Database;

const BASE_TRANSCODE_DIR: &str = "/tmp/rmc-transcode";
const M3U8_WAIT_RETRIES: usize = 300;
const M3U8_WAIT_DELAY_MS: u64 = 100;

fn get_config_path() -> String {
    std::env::var("RMC_CONFIG").unwrap_or_else(|_| "config.toml".to_string())
}

static TRANSCODE_MANAGER: OnceLock<TranscodeManager> = OnceLock::new();

pub fn get_transcode_manager() -> &'static TranscodeManager {
    TRANSCODE_MANAGER.get_or_init(|| {
        TranscodeManager::new(BASE_TRANSCODE_DIR.to_string())
    })
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
    wait_for_playlist_with_retry(transcode, movie_id, m3u8_path, M3U8_WAIT_RETRIES, M3U8_WAIT_DELAY_MS)
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
        let source = crate::strm::StrmParser::parse(&content)
            .context("Invalid strm file content")?;
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
        .ok_or_else(|| crate::error::AppError::Unauthorized("Missing Authorization header".to_string()))?;

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
        return Err(crate::error::AppError::Unauthorized("Invalid credentials".to_string()));
    }

    let token = crate::auth::create_jwt(&payload.username)
        .context("Failed to create jwt")?;

    Ok(Json(LoginResponse { token }))
}

pub async fn get_libraries(
    State(db): State<AppState>,
) -> Result<Json<Vec<serde_json::Value>>, crate::error::AppError> {
    let count = db.get_movie_count().await.context("Failed to get movie count")?;
    Ok(Json(vec![serde_json::json!({
        "id": 1,
        "name": "Movies",
        "count": count
    })]))
}

pub async fn get_movie_by_id(
    State(db): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Movie>, crate::error::AppError> {
    let movie = db.get_movie_by_id(id).await.map_err(|e| {
        match e {
            sqlx::Error::RowNotFound => crate::error::AppError::NotFound(format!("Movie id {} not found", id)),
            _ => crate::error::AppError::Internal(e.into()),
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

async fn list_movies(
    State(db): State<AppState>,
    Query(query): Query<MovieQuery>,
) -> Result<Json<Vec<Movie>>, crate::error::AppError> {
    let movies = if let Some(q) = query.q {
        db.search_movies(&q).await.context("Failed to search movies")?
    } else {
        db.get_movies().await.context("Failed to get movies")?
    };
    
    Ok(Json(movies))
}

pub async fn direct_play(
    State(db): State<AppState>,
    Path(id): Path<i64>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<impl axum::response::IntoResponse, crate::error::AppError> {
    let movie = db.get_movie_by_id(id).await.map_err(|e| {
        match e {
            sqlx::Error::RowNotFound => crate::error::AppError::NotFound(format!("Movie id {} not found", id)),
            _ => crate::error::AppError::Internal(e.into()),
        }
    })?;

    let file_path = movie.file_path;
    let extension = file_path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

    if extension.eq_ignore_ascii_case("strm") {
        let content = tokio::fs::read_to_string(&file_path).await
            .context("Failed to read strm file")?;
        if let Some(source) = crate::strm::StrmParser::parse(&content) {
            use axum::response::{IntoResponse, Response};
            use axum::http::{header, StatusCode};
            let response = Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, source.url)
                .body(axum::body::Body::empty())
                .context("Failed to build response")?;
            Ok(response.into_response())
        } else {
            Err(crate::error::AppError::Internal(anyhow::anyhow!("Invalid strm file content")))
        }
    } else {
        use tower::ServiceExt;
        use axum::response::IntoResponse;
        match tower_http::services::ServeFile::new(file_path).oneshot(req).await {
            Ok(res) => Ok(res.into_response()),
            Err(_) => Err(crate::error::AppError::Internal(anyhow::anyhow!("ServeFile failed"))),
        }
    }
}

pub async fn hls_playlist(
    State(db): State<AppState>,
    Path(id): Path<i64>,
) -> Result<impl axum::response::IntoResponse, crate::error::AppError> {
    let movie = db.get_movie_by_id(id).await.map_err(|e| {
        match e {
            sqlx::Error::RowNotFound => crate::error::AppError::NotFound(format!("Movie id {} not found", id)),
            _ => crate::error::AppError::Internal(e.into()),
        }
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

    let content = tokio::fs::read_to_string(&m3u8_path).await
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
) -> Result<impl axum::response::IntoResponse, crate::error::AppError> {
    let movie = db.get_movie_by_id(id).await.map_err(|e| match e {
        sqlx::Error::RowNotFound => {
            crate::error::AppError::NotFound(format!("Movie id {} not found", id))
        }
        _ => crate::error::AppError::Internal(e.into()),
    })?;

    let (input_path, input_headers) = resolve_movie_input(&movie.file_path).await?;
    let transcode = get_transcode_manager();
    let session = transcode
        .start_stream_transcode(id, &input_path, input_headers.as_deref())
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
        return Err(crate::error::AppError::NotFound("Invalid segment path".to_string()));
    }

    let file_path = std::path::PathBuf::from(BASE_TRANSCODE_DIR)
        .join(id.to_string())
        .join(segment_clean);

    if !file_path.exists() {
        return Err(crate::error::AppError::NotFound(format!("Segment {} not found", segment_clean)));
    }

    let content = tokio::fs::read(file_path).await
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

    config.save_to(&get_config_path())
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
                            tracing::info!("Scraping metadata for movie: {}", movie.title);
                            match scraper.fetch_movie_metadata(&movie.title).await {
                                Ok(metadata) => {
                                    if let Err(e) = db_clone.update_movie_metadata(
                                        movie.id,
                                        metadata.poster_url,
                                        metadata.overview,
                                        metadata.tmdb_id,
                                        metadata.runtime_minutes,
                                    ).await {
                                        tracing::error!("Failed to update database metadata for movie {}: {:?}", movie.title, e);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to fetch TMDB metadata for {}: {:?}", movie.title, e);
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
    Json(vec![User { id: 1, username: "admin".to_string() }])
}

pub async fn report_progress() -> &'static str {
    "Progress Saved"
}


#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use axum::http::header::AUTHORIZATION;
    use std::os::unix::fs::PermissionsExt;
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
            Self { _lock: lock, entries: previous }
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let login_response: LoginResponse = serde_json::from_slice(&body).unwrap();
        login_response.token
    }

    #[tokio::test]
    async fn test_health_check() {
        let db = crate::db::Database::new("sqlite::memory:").await.unwrap();
        let app = app_router(db);
        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), 200);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "OK");
    }

    #[tokio::test]
    async fn test_get_movies_api() {
        use rmc_core::models::Movie;
        use crate::db::Database;
        
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Matrix".to_string(),
            year: Some(1999),
            file_path: std::path::PathBuf::from("/m/matrix.mp4"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            added_at: 0,
            file_size: None,
        }).await.unwrap();

        let app = app_router(db);
        
        let response = app
            .oneshot(axum::http::Request::builder().uri("/api/v1/movies").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("Matrix"));
    }

    #[tokio::test]
    async fn test_direct_play_range_header() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use rmc_core::models::Movie;
        
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
            added_at: 0,
            file_size: None,
        }).await.unwrap();
        let app = super::app_router(db); 
        
        let response = app.oneshot(
            Request::builder()
                .uri("/api/v1/movies/1/direct")
                .header("Range", "bytes=0-4")
                .body(Body::empty())
                .unwrap()
        ).await.unwrap();
        
        // 断言返回状态码为 206 Partial Content，且返回前 5 个字节内容
        assert_eq!(response.status().as_u16(), 206);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body_str, "Hello");
    }

    #[tokio::test]
    async fn test_playback_progress_api() {
        let app = app_router(crate::db::Database::new("sqlite::memory:").await.unwrap());
        let response = app
            .oneshot(Request::builder().method("POST").uri("/api/v1/playback/progress").body(Body::empty()).unwrap())
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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let login_response: LoginResponse = serde_json::from_slice(&body).unwrap();
        assert!(!login_response.token.is_empty());
    }

    #[tokio::test]
    async fn test_protected_config_requires_auth() {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/config").body(Body::empty()).unwrap())
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
    async fn test_get_libraries() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use crate::db::Database;
        
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);
        
        let response = app.clone()
            .oneshot(Request::builder().uri("/api/v1/libraries").body(Body::empty()).unwrap())
            .await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_get_movie_by_id() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use crate::db::Database;
        use rmc_core::models::Movie;
        
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Mock Movie".to_string(),
            year: Some(2020),
            file_path: std::path::PathBuf::from("/mock.mp4"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            added_at: 0,
            file_size: None,
        }).await.unwrap();
        let app = super::app_router(db);
        
        let response = app.clone()
            .oneshot(Request::builder().uri("/api/v1/movies/1").body(Body::empty()).unwrap())
            .await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_playback_start() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use crate::db::Database;
        
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);
        
        let response = app.clone()
            .oneshot(Request::builder().method("POST").uri("/api/v1/playback/start").body(Body::empty()).unwrap())
            .await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_health_check_tracing() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use crate::db::Database;
        
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);
        
        let response = app.oneshot(
            Request::builder().uri("/health").body(Body::empty()).unwrap()
        ).await.unwrap();
        
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_cors_headers() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use crate::db::Database;
        
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);
        
        let response = app.oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/v1/movies")
                .header("Origin", "http://localhost:5173")
                .header("Access-Control-Request-Method", "GET")
                .body(Body::empty())
                .unwrap()
        ).await.unwrap();
        
        assert_eq!(response.status(), 200);
        assert!(response.headers().contains_key("access-control-allow-origin"));
    }

    #[tokio::test]
    async fn test_api_search_movies() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use crate::db::Database;
        
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&rmc_core::models::Movie {
            id: 0,
            title: "Inception".to_string(),
            year: Some(2010),
            file_path: std::path::PathBuf::from("/m.mkv"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            added_at: 0,
            file_size: None,
        }).await.unwrap();
        
        let app = super::app_router(db); 
        
        // 测 GET /api/v1/movies?q=Inception
        let response = app.oneshot(
            Request::builder().uri("/api/v1/movies?q=Inception").body(Body::empty()).unwrap()
        ).await.unwrap();
        
        assert_eq!(response.status(), 200);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Inception"));
    }

    #[tokio::test]
    async fn test_direct_play_strm_302() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use crate::db::Database;
        use rmc_core::models::Movie;

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
            added_at: 0,
            file_size: None,
        }).await.unwrap();

        let app = super::app_router(db);

        let response = app.oneshot(
            Request::builder().uri("/api/v1/movies/1/direct").body(Body::empty()).unwrap()
        ).await.unwrap();

        assert_eq!(response.status().as_u16(), 302);
        let location = response.headers().get("location").unwrap().to_str().unwrap();
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
            ("RMC_CONFIG", config_path.to_str().ok_or("Invalid path string")?),
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
        let response = app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/config")
                    .header(AUTHORIZATION, format!("Bearer {}", token))
                    .body(Body::empty())?
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
        let response = app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/config")
                    .header(AUTHORIZATION, format!("Bearer {}", token))
                    .header("content-type", "application/json")
                    .body(Body::from(post_body))?
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
                    .body(Body::empty())?
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

