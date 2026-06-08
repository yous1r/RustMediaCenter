mod api;
mod db;
pub mod strm;
pub mod watcher;
pub mod transcode;
pub mod scraper;
pub mod config;
pub mod error;
pub mod auth;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive("rmc_server=debug".parse().unwrap()))
        .init();
    tracing::info!("Starting RustMediaCenter server...");

    let db = db::Database::new("movies.db")?;
    db.init_schema()?;
    
    let state = std::sync::Arc::new(std::sync::Mutex::new(db));
    let app = api::app_router(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
