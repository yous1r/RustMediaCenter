mod api;
pub mod auth;
pub mod config;
mod db;
mod emby;
pub mod error;
mod media_probe;
mod media_tree;
pub mod scanner;
pub mod scraper;
pub mod strm;
pub mod transcode;
pub mod watcher;

fn get_config_path() -> String {
    std::env::var("RMC_CONFIG").unwrap_or_else(|_| "config.toml".to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("rmc_server=debug".parse().unwrap()),
        )
        .init();
    tracing::info!("Starting RustMediaCenter server...");

    let config = config::ServerConfig::load_from(get_config_path())?;

    if let Some(parent) = std::path::Path::new(&config.db_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let db = db::Database::new(&format!("sqlite:{}", config.db_path)).await?;
    db.init_schema().await?;

    let mut media_watcher = watcher::MediaWatcher::new(db.clone());
    if let Err(e) = media_watcher.start(&config.media_dirs) {
        tracing::error!("Failed to start watcher: {:?}", e);
    }

    let app = api::app_router(db);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::get_config_path;

    #[test]
    fn test_get_config_path_prefers_rmc_config_env() {
        let key = "RMC_CONFIG";
        let original = std::env::var_os(key);

        unsafe {
            std::env::set_var(key, "/tmp/rmc-custom-config.toml");
        }
        assert_eq!(get_config_path(), "/tmp/rmc-custom-config.toml");

        match original {
            Some(value) => unsafe {
                std::env::set_var(key, value);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }
}
