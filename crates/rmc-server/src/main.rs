mod api;
mod db;

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let db = db::Database::new("movies.db").unwrap();
    db.init_schema().unwrap();
    
    let state = std::sync::Arc::new(std::sync::Mutex::new(db));
    let app = api::app_router(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
