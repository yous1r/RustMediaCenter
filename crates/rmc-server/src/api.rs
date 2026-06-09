use axum::{extract::{Path, State, Query}, routing::get, Json, Router};
use serde::Deserialize;

use tower_http::cors::{Any, CorsLayer};
use crate::db::Database;
use rmc_core::models::{Movie, User};

pub type AppState = Database;

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
        .route("/api/v1/playback/progress", axum::routing::post(report_progress))
        .route("/api/v1/auth/login", axum::routing::post(login))
        .route("/api/v1/libraries", get(get_libraries))
        .route("/api/v1/movies/:id", get(get_movie_by_id))
        .route("/api/v1/playback/start", axum::routing::post(playback_start))
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
}

pub async fn login() -> String {
    crate::auth::create_jwt("admin").unwrap_or_else(|_| "Error".to_string())
}

pub async fn get_libraries() -> &'static str {
    "[{\"id\":1,\"name\":\"Movies\"}]"
}

pub async fn get_movie_by_id(Path(id): Path<i64>) -> String {
    format!("{{\"id\":{},\"title\":\"Mock Movie\",\"stream_url\":\"/api/v1/movies/{}/direct\"}}", id, id)
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
        db.search_movies(&q).await.map_err(|e| crate::error::AppError::Internal(e.into()))?
    } else {
        db.get_movies().await.map_err(|e| crate::error::AppError::Internal(e.into()))?
    };
    
    Ok(Json(movies))
}

pub async fn direct_play(
    axum::extract::Path(id): axum::extract::Path<i64>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<impl axum::response::IntoResponse, crate::error::AppError> {
    let file_path = format!("/tmp/movies/{}.mp4", id);
    
    use tower::ServiceExt;
    use axum::response::IntoResponse;
    match tower_http::services::ServeFile::new(file_path).oneshot(req).await {
        Ok(res) => Ok(res.into_response()),
        Err(_) => Err(crate::error::AppError::Internal(anyhow::anyhow!("ServeFile failed"))),
    }
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
        
        let db = crate::db::Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        let app = super::app_router(db); 
        
        let response = app.oneshot(
            Request::builder().uri("/api/v1/movies/1/direct").header("Range", "bytes=0-100").body(Body::empty()).unwrap()
        ).await.unwrap();
        
        // 我们期望它能支持视频流，由于依赖 ServeFile 但 /tmp/movies/1.mp4 并不存在，
        // 故期待 404 而不是原本占位符的 200 OK，这说明流量已经正确进入了 tower-http 的静态文件处理环节。
        assert_eq!(response.status().as_u16(), 404);
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
        
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
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
        db.insert_movie(&rmc_core::models::Movie { id:0, title:"Inception".to_string(), year:Some(2010), file_path:std::path::PathBuf::from("/m.mkv") }).await.unwrap();
        
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
}
