mod api;

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let app = api::app_router();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
