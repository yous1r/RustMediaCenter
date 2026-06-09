use axum::{extract::{Path, State, Query}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use anyhow::Context;

use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use crate::db::Database;
use rmc_core::models::{Movie, User};
use std::sync::OnceLock;
use crate::transcode::TranscodeManager;

pub type AppState = Database;

const BASE_TRANSCODE_DIR: &str = "/tmp/rmc-transcode";
const M3U8_WAIT_RETRIES: usize = 50;
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

pub fn app_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/health", get(|| async { "OK" }))
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
        .route("/api/v1/config", get(get_config).post(update_config))
        .route("/api/v1/scan", post(trigger_scan))
        .nest_service("/", ServeDir::new("web-client"))
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
}

pub async fn login() -> String {
    crate::auth::create_jwt("admin").unwrap_or_else(|_| "Error".to_string())
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
        if let Some(url) = crate::strm::StrmParser::parse(&content) {
            use axum::response::{IntoResponse, Response};
            use axum::http::{header, StatusCode};
            let response = Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, url)
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

    let file_path_str = movie.file_path.to_string_lossy().into_owned();
    let input_path = if movie.file_path.extension().and_then(|ext| ext.to_str()).unwrap_or("").eq_ignore_ascii_case("strm") {
        let content = tokio::fs::read_to_string(&movie.file_path).await
            .context("Failed to read strm file")?;
        crate::strm::StrmParser::parse(&content)
            .context("Invalid strm file content")?
    } else {
        file_path_str
    };

    let transcode = get_transcode_manager();
    transcode.start_transcode_session(id, &input_path).await
        .map_err(|e| anyhow::anyhow!(e))
        .context("Failed to start transcode")?;

    let m3u8_path = transcode.get_m3u8_path(id);
    let m3u8_path_buf = std::path::PathBuf::from(&m3u8_path);

    let mut found = false;
    for _ in 0..M3U8_WAIT_RETRIES {
        if m3u8_path_buf.exists() {
            found = true;
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(M3U8_WAIT_DELAY_MS)).await;
    }

    if !found {
        return Err(crate::error::AppError::Internal(anyhow::anyhow!("Timeout waiting for master.m3u8")));
    }

    transcode.touch_session(id).await;

    let content = tokio::fs::read_to_string(&m3u8_path).await
        .context("Failed to read master.m3u8")?;

    let response = axum::response::Response::builder()
        .header("Content-Type", "application/vnd.apple.mpegurl")
        .body(axum::body::Body::from(content))
        .context("Failed to build response")?;
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

pub async fn get_config() -> Result<Json<crate::config::ServerConfig>, crate::error::AppError> {
    let config = crate::config::ServerConfig::load_from(&get_config_path())
        .context("Failed to load config")?;
    Ok(Json(config))
}

pub async fn update_config(
    Json(new_config): Json<crate::config::ServerConfig>,
) -> Result<Json<crate::config::ServerConfig>, crate::error::AppError> {
    new_config.save_to(&get_config_path())
        .context("Failed to save config")?;
    Ok(Json(new_config))
}

pub async fn trigger_scan(
    State(db): State<AppState>,
) -> Result<&'static str, crate::error::AppError> {
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
    use tower::ServiceExt;

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
    async fn test_auth_login() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        use crate::db::Database;

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db);
        
        let response = app
            .oneshot(Request::builder().method("POST").uri("/api/v1/auth/login").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
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
}

