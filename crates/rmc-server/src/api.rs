use axum::{extract::{Path, State}, routing::get, Json, Router};

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
        .route("/stream/:id/direct", get(direct_stream))
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
    format!("{{\"id\":{},\"title\":\"Mock Movie\",\"stream_url\":\"/stream/{}/direct\"}}", id, id)
}

pub async fn playback_start() -> &'static str {
    "Playback Started"
}

async fn list_movies(State(state): State<AppState>) -> Result<Json<Vec<Movie>>, axum::http::StatusCode> {
    let movies = state.get_movies().await.map_err(|e| {
        tracing::error!("Database error: {:?}", e);
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    Ok(Json(movies))
}

pub async fn direct_stream(Path(id): Path<i64>) -> String {
    format!("Streaming movie id: {}", id)
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
    async fn test_direct_play_api() {
        let app = app_router(crate::db::Database::new("sqlite::memory:").await.unwrap());
        let response = app
            .oneshot(Request::builder().uri("/stream/1/direct").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
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
}
