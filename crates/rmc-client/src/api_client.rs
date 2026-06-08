use rmc_core::models::Movie;
use reqwest::Error;

pub async fn fetch_movies(url: &str) -> Result<Vec<Movie>, Error> {
    let response = reqwest::get(url).await?;
    let movies = response.json::<Vec<Movie>>().await?;
    Ok(movies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_movies_error_handling() {
        // 测试当目标不存在时返回错误
        let result = fetch_movies("http://127.0.0.1:9999/api/v1/movies").await;
        assert!(result.is_err());
    }
}
