use axum::{extract::{Path, State}, routing::get, Json, Router};
use std::sync::{Arc, Mutex};
use crate::db::Database;
use rmc_core::models::Movie;

pub type AppState = Arc<Mutex<Database>>;

pub fn app_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/api/v1/movies", get(list_movies))
        .route("/stream/:id/direct", get(direct_stream))
        .with_state(state)
}

async fn list_movies(State(state): State<AppState>) -> Result<Json<Vec<Movie>>, axum::http::StatusCode> {
    let movies = tokio::task::spawn_blocking(move || {
        let db = state.lock().unwrap();
        db.get_all_movies()
    })
    .await
    .map_err(|e| {
        eprintln!("Task join error: {:?}", e);
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map_err(|e| {
        eprintln!("Database error: {:?}", e);
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    Ok(Json(movies))
}

pub async fn direct_stream(Path(id): Path<i64>) -> String {
    format!("Streaming movie id: {}", id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_check() {
        let db = crate::db::Database::new_in_memory().unwrap();
        let app = app_router(std::sync::Arc::new(std::sync::Mutex::new(db)));
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
        use std::sync::Arc;
        
        let db = Database::new_in_memory().unwrap();
        db.init_schema().unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Matrix".to_string(),
            year: Some(1999),
            file_path: std::path::PathBuf::from("/m/matrix.mp4"),
        }).unwrap();

        let app = app_router(Arc::new(std::sync::Mutex::new(db)));
        
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
        let app = app_router(std::sync::Arc::new(std::sync::Mutex::new(crate::db::Database::new_in_memory().unwrap())));
        let response = app
            .oneshot(Request::builder().uri("/stream/1/direct").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
    }
}
