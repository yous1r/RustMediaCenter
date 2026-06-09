mod api;
mod db;
pub mod strm;
pub mod watcher;
pub mod transcode;
pub mod scraper;
pub mod config;
pub mod error;
pub mod auth;
pub mod scanner;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive("rmc_server=debug".parse().unwrap()))
        .init();
    tracing::info!("Starting RustMediaCenter server...");

    let config = config::ServerConfig::load_from("config.toml")?;

    if let Some(parent) = std::path::Path::new(&config.db_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let db = db::Database::new(&format!("sqlite://{}", config.db_path)).await?;
    db.init_schema().await?;

    watcher::start_watcher();
    
    let app = api::app_router(db);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
