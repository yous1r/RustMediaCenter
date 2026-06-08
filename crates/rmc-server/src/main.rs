mod api;
mod db;
pub mod strm;
pub mod watcher;
pub mod transcode;
pub mod scraper;
pub mod config;
pub mod error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = db::Database::new("movies.db")?;
    db.init_schema()?;
    
    let state = std::sync::Arc::new(std::sync::Mutex::new(db));
    let app = api::app_router(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
