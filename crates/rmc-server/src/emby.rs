use axum::{
    extract::{OriginalUri, Path, Query, State},
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use rmc_core::models::Movie;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::AppState;
use crate::auth::Claims;
use crate::db::Database;
use crate::error::AppError;

const SERVER_ID: &str = "rust-media-center";
const VIRTUAL_USER_ID: &str = "1";
const DEFAULT_SERVER_NAME: &str = "RustMediaCenter";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/System/Info/Public", get(system_info_public))
        .route("/System/Info", get(system_info))
        .route("/Users/Public", get(users_public))
        .route("/Users/AuthenticateByName", post(authenticate_by_name))
        .route("/Users/Me", get(current_user))
        .route("/Users/:user_id", get(get_user))
        .route("/Users/:user_id/Views", get(user_views))
        .route("/Users/:user_id/Items", get(user_items))
        .route("/Users/:user_id/Items/Latest", get(latest_items))
        .route("/Users/:user_id/Items/:item_id", get(user_item_detail))
        .route("/Items", get(items))
        .route("/Items/:id", get(item_detail))
        .route(
            "/Items/:id/PlaybackInfo",
            get(playback_info).post(playback_info),
        )
        .route("/Items/:id/Images/Primary", get(primary_image))
        .route("/Videos/:id/original", get(video_original))
        .route("/Videos/:id/stream", get(video_stream))
        .route("/Videos/:id/stream.mp4", get(video_stream_mp4))
        .route("/Videos/:id/hls/master.m3u8", get(video_hls_playlist))
        .route("/Sessions/Playing", post(report_playing))
        .route("/Sessions/Playing/Progress", post(report_playing_progress))
        .route("/Sessions/Playing/Stopped", post(report_playing_stopped))
}

#[derive(Debug)]
struct AuthContext {
    claims: Claims,
    token: String,
}

#[derive(Debug, Deserialize, Default)]
struct ApiKeyQuery {
    #[serde(default, alias = "api_key", alias = "ApiKey")]
    api_key: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ItemsQuery {
    #[serde(default, alias = "api_key", alias = "ApiKey")]
    api_key: Option<String>,
    #[serde(default, alias = "ParentId", alias = "parentId")]
    parent_id: Option<String>,
    #[serde(default, alias = "Ids", alias = "ids")]
    ids: Option<String>,
    #[serde(default, alias = "SearchTerm", alias = "searchTerm")]
    search_term: Option<String>,
    #[serde(default, alias = "StartIndex", alias = "startIndex")]
    start_index: Option<usize>,
    #[serde(default, alias = "Limit", alias = "limit")]
    limit: Option<usize>,
    #[serde(default, alias = "SortBy", alias = "sortBy")]
    sort_by: Option<String>,
    #[serde(default, alias = "SortOrder", alias = "sortOrder")]
    sort_order: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthenticateByNameRequest {
    #[serde(default, alias = "Username", alias = "username")]
    username: Option<String>,
    #[serde(default, alias = "Pw", alias = "pw")]
    pw: Option<String>,
    #[serde(default, alias = "Password", alias = "password")]
    password: Option<String>,
}

fn server_name() -> String {
    std::env::var("RMC_EMBY_SERVER_NAME").unwrap_or_else(|_| DEFAULT_SERVER_NAME.to_string())
}

fn auth_error() -> AppError {
    AppError::Unauthorized("Missing or invalid Emby token".to_string())
}

fn token_from_authorization_header(value: &str) -> Option<String> {
    if let Some(token) = value.strip_prefix("Bearer ") {
        let trimmed = token.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    for key in ["Token", "ApiKey"] {
        let needle = format!("{}=\"", key);
        if let Some(start) = value.find(&needle) {
            let rest = &value[start + needle.len()..];
            if let Some(end) = rest.find('"') {
                let token = rest[..end].trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    None
}

fn extract_token(headers: &HeaderMap, query_token: Option<&str>) -> Result<String, AppError> {
    if let Some(token) = query_token.filter(|value| !value.trim().is_empty()) {
        return Ok(token.trim().to_string());
    }

    for header_name in ["X-Emby-Token", "X-MediaBrowser-Token"] {
        if let Some(token) = headers
            .get(header_name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(token.to_string());
        }
    }

    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(token_from_authorization_header)
    {
        return Ok(value);
    }

    Err(auth_error())
}

fn require_auth(headers: &HeaderMap, query_token: Option<&str>) -> Result<AuthContext, AppError> {
    let token = extract_token(headers, query_token)?;
    let claims = crate::auth::verify_jwt(&token).map_err(|_| auth_error())?;
    Ok(AuthContext { claims, token })
}

fn route_prefix(uri: &Uri) -> &'static str {
    let path = uri.path();
    if path == "/emby" || path.starts_with("/emby/") {
        "/emby"
    } else if path == "/jellyfin" || path.starts_with("/jellyfin/") {
        "/jellyfin"
    } else {
        ""
    }
}

fn base_url(headers: &HeaderMap) -> String {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("localhost");

    format!("{}://{}", proto, host)
}

fn absolute_url(headers: &HeaderMap, prefix: &str, path: &str) -> String {
    format!("{}{}{}", base_url(headers), prefix, path)
}

fn with_api_key(url: String, token: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{}{}api_key={}", url, separator, urlencoding::encode(token))
}

fn virtual_user(name: &str) -> Value {
    json!({
        "Name": name,
        "Id": VIRTUAL_USER_ID,
        "ServerId": SERVER_ID,
        "HasPassword": true,
        "HasConfiguredPassword": true,
        "EnableAutoLogin": false,
        "Configuration": {},
        "Policy": {
            "IsAdministrator": true,
            "EnableMediaPlayback": true
        }
    })
}

fn movie_container(movie: &Movie) -> Option<String> {
    let ext = movie
        .file_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match ext.as_deref() {
        Some("strm") => None,
        Some(value) if !value.is_empty() => Some(value.to_string()),
        _ => None,
    }
}

fn runtime_ticks(movie: &Movie) -> Option<u64> {
    movie
        .runtime_minutes
        .map(|minutes| minutes as u64 * 60 * 10_000_000)
}

fn build_media_source(movie: &Movie, headers: &HeaderMap, prefix: &str, token: &str) -> Value {
    let direct_url = with_api_key(
        absolute_url(
            headers,
            prefix,
            &format!("/Videos/{}/original", movie.id),
        ),
        token,
    );
    let transcode_url = with_api_key(
        absolute_url(
            headers,
            prefix,
            &format!("/Videos/{}/stream.mp4", movie.id),
        ),
        token,
    );

    json!({
        "Id": format!("movie-{}", movie.id),
        "ItemId": movie.id.to_string(),
        "Name": movie.title,
        "Type": "Default",
        "Protocol": "Http",
        "Path": direct_url,
        "Container": movie_container(movie),
        "Size": movie.file_size,
        "RunTimeTicks": runtime_ticks(movie),
        "IsRemote": movie.file_path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.eq_ignore_ascii_case("strm")).unwrap_or(false),
        "SupportsDirectPlay": true,
        "SupportsDirectStream": true,
        "SupportsTranscoding": true,
        "DirectStreamUrl": direct_url,
        "TranscodingUrl": transcode_url,
        "TranscodingSubProtocol": "mp4",
        "TranscodingContainer": "mp4",
        "RequiredHttpHeaders": {},
        "MediaStreams": []
    })
}

fn build_movie_item(movie: &Movie, headers: &HeaderMap, prefix: &str, token: Option<&str>) -> Value {
    let image_url = movie.poster_url.as_ref().map(|_| {
        let url = absolute_url(
            headers,
            prefix,
            &format!("/Items/{}/Images/Primary", movie.id),
        );
        match token {
            Some(value) => with_api_key(url, value),
            None => url,
        }
    });

    json!({
        "Name": movie.title,
        "ServerId": SERVER_ID,
        "Id": movie.id.to_string(),
        "Type": "Movie",
        "MediaType": "Video",
        "CollectionType": "movies",
        "Overview": movie.overview,
        "ProductionYear": movie.year,
        "RunTimeTicks": runtime_ticks(movie),
        "ImageTags": movie.poster_url.as_ref().map(|_| json!({ "Primary": format!("poster-{}", movie.id) })).unwrap_or_else(|| json!({})),
        "ImageBlurHashes": {},
        "PrimaryImageAspectRatio": serde_json::Value::Null,
        "PrimaryImageItemId": movie.poster_url.as_ref().map(|_| movie.id.to_string()),
        "ImagePath": image_url,
        "CanDownload": true,
        "RecursiveItemCount": 0,
        "IsFolder": false,
        "Container": movie_container(movie),
        "UserData": {
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": false
        }
    })
}

fn parse_item_ids(raw: &str) -> Vec<i64> {
    raw.split(',')
        .filter_map(|value| value.trim().parse::<i64>().ok())
        .collect()
}

fn sort_movies(movies: &mut [Movie], sort_by: Option<&str>, sort_order: Option<&str>) {
    let descending = sort_order
        .map(|value| value.eq_ignore_ascii_case("descending"))
        .unwrap_or(false);

    if sort_by
        .map(|value| value.eq_ignore_ascii_case("DateCreated") || value.eq_ignore_ascii_case("DateAdded"))
        .unwrap_or(false)
    {
        movies.sort_by_key(|movie| movie.added_at);
    } else {
        movies.sort_by(|a, b| a.title.cmp(&b.title));
    }

    if descending {
        movies.reverse();
    }
}

async fn load_movies(db: &Database, query: &ItemsQuery) -> Result<Vec<Movie>, AppError> {
    let mut movies = if let Some(search_term) = query.search_term.as_deref().filter(|value| !value.trim().is_empty()) {
        db.search_movies(search_term)
            .await
            .map_err(|err| AppError::Internal(err.into()))?
    } else {
        db.get_movies()
            .await
            .map_err(|err| AppError::Internal(err.into()))?
    };

    if let Some(ids) = query.ids.as_deref().filter(|value| !value.trim().is_empty()) {
        let ids = parse_item_ids(ids);
        movies.retain(|movie| ids.contains(&movie.id));
    }

    if let Some(parent_id) = query.parent_id.as_deref().filter(|value| !value.trim().is_empty()) {
        let is_library_root = parent_id == "movies" || parent_id == VIRTUAL_USER_ID;
        if !is_library_root {
            if let Ok(parent_movie_id) = parent_id.parse::<i64>() {
                movies.retain(|movie| movie.id == parent_movie_id);
            } else {
                movies.clear();
            }
        }
    }

    sort_movies(&mut movies, query.sort_by.as_deref(), query.sort_order.as_deref());
    Ok(movies)
}

fn paged_items_response(items: Vec<Value>, total: usize, start_index: usize) -> Value {
    json!({
        "Items": items,
        "TotalRecordCount": total,
        "StartIndex": start_index
    })
}

async fn get_movie(db: &Database, id: i64) -> Result<Movie, AppError> {
    db.get_movie_by_id(id).await.map_err(|err| match err {
        sqlx::Error::RowNotFound => AppError::NotFound(format!("Movie id {} not found", id)),
        _ => AppError::Internal(err.into()),
    })
}

async fn system_info_public(
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({
        "ServerName": server_name(),
        "Version": env!("CARGO_PKG_VERSION"),
        "ProductName": "Emby",
        "Id": SERVER_ID,
        "StartupWizardCompleted": true,
        "LocalAddress": base_url(&headers),
        "WanAddress": base_url(&headers)
    })))
}

async fn system_info(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
) -> Result<Json<Value>, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    Ok(Json(json!({
        "ServerName": server_name(),
        "Version": env!("CARGO_PKG_VERSION"),
        "ProductName": "Emby",
        "Id": SERVER_ID,
        "OperatingSystemDisplayName": std::env::consts::OS,
        "LocalAddress": base_url(&headers),
        "WanAddress": base_url(&headers),
        "StartupWizardCompleted": true
    })))
}

async fn users_public() -> Result<Json<Value>, AppError> {
    let username = crate::auth::admin_username();
    Ok(Json(json!([virtual_user(&username)])))
}

async fn authenticate_by_name(
    Json(payload): Json<AuthenticateByNameRequest>,
) -> Result<Json<Value>, AppError> {
    let username = payload.username.unwrap_or_default();
    let password = payload.pw.or(payload.password).unwrap_or_default();

    if !crate::auth::validate_admin_login(&username, &password) {
        return Err(AppError::Unauthorized("Invalid credentials".to_string()));
    }

    let access_token = crate::auth::create_jwt(&username)
        .map_err(|err| AppError::Internal(err.into()))?;
    let session_id = format!("session-{}-{}", username, Utc::now().timestamp_millis());

    Ok(Json(json!({
        "User": virtual_user(&username),
        "SessionInfo": {
            "Id": session_id,
            "UserId": VIRTUAL_USER_ID,
            "UserName": username,
            "AdditionalUsers": [],
            "Capabilities": {}
        },
        "AccessToken": access_token,
        "ServerId": SERVER_ID
    })))
}

async fn current_user(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    Ok(Json(virtual_user(&auth.claims.sub)))
}

async fn get_user(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    Path(_user_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    Ok(Json(virtual_user(&auth.claims.sub)))
}

async fn user_views(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(_user_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    let count = db
        .get_movie_count()
        .await
        .map_err(|err| AppError::Internal(err.into()))?;

    Ok(Json(json!({
        "Items": [{
            "Name": "Movies",
            "ServerId": SERVER_ID,
            "Id": "movies",
            "Type": "CollectionFolder",
            "CollectionType": "movies",
            "IsFolder": true,
            "ChildCount": count,
            "ImageTags": {}
        }],
        "TotalRecordCount": 1,
        "StartIndex": 0
    })))
}

async fn user_items(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ItemsQuery>,
    State(db): State<AppState>,
    Path(_user_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    let prefix = route_prefix(&original_uri.0);
    let movies = load_movies(&db, &query).await?;
    let total = movies.len();
    let start_index = query.start_index.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(total.saturating_sub(start_index));
    let items = movies
        .into_iter()
        .skip(start_index)
        .take(limit)
        .map(|movie| build_movie_item(&movie, &headers, prefix, Some(&auth.token)))
        .collect::<Vec<_>>();

    Ok(Json(paged_items_response(items, total, start_index)))
}

async fn latest_items(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(_user_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    let prefix = route_prefix(&original_uri.0);
    let mut movies = db
        .get_movies()
        .await
        .map_err(|err| AppError::Internal(err.into()))?;
    movies.sort_by_key(|movie| movie.added_at);
    movies.reverse();

    let items = movies
        .into_iter()
        .take(50)
        .map(|movie| build_movie_item(&movie, &headers, prefix, Some(&auth.token)))
        .collect::<Vec<_>>();

    Ok(Json(Value::Array(items)))
}

async fn items(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ItemsQuery>,
    State(db): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    let prefix = route_prefix(&original_uri.0);
    let movies = load_movies(&db, &query).await?;
    let total = movies.len();
    let start_index = query.start_index.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(total.saturating_sub(start_index));
    let items = movies
        .into_iter()
        .skip(start_index)
        .take(limit)
        .map(|movie| build_movie_item(&movie, &headers, prefix, Some(&auth.token)))
        .collect::<Vec<_>>();

    Ok(Json(paged_items_response(items, total, start_index)))
}

async fn item_detail(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    let movie_id = id
        .parse::<i64>()
        .map_err(|_| AppError::NotFound(format!("Invalid movie id {}", id)))?;
    let movie = get_movie(&db, movie_id).await?;
    let prefix = route_prefix(&original_uri.0);
    let mut item = build_movie_item(&movie, &headers, prefix, Some(&auth.token));
    item["MediaSources"] = json!([build_media_source(&movie, &headers, prefix, &auth.token)]);
    Ok(Json(item))
}

async fn user_item_detail(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path((_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    item_detail(headers, original_uri, Query(query), State(db), Path(item_id)).await
}

async fn primary_image(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    let movie_id = id
        .parse::<i64>()
        .map_err(|_| AppError::NotFound(format!("Invalid movie id {}", id)))?;
    let movie = get_movie(&db, movie_id).await?;
    let poster_url = movie
        .poster_url
        .ok_or_else(|| AppError::NotFound(format!("Movie id {} has no poster", movie_id)))?;
    Ok(Redirect::temporary(&poster_url))
}

async fn playback_info(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    let movie_id = id
        .parse::<i64>()
        .map_err(|_| AppError::NotFound(format!("Invalid movie id {}", id)))?;
    let movie = get_movie(&db, movie_id).await?;
    let prefix = route_prefix(&original_uri.0);
    let play_session_id = format!("play-{}-{}", movie.id, Utc::now().timestamp_millis());

    Ok(Json(json!({
        "PlaySessionId": play_session_id,
        "MediaSources": [build_media_source(&movie, &headers, prefix, &auth.token)]
    })))
}

fn redirect_to_api_path(path: &str) -> Result<Response, AppError> {
    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(header::LOCATION, path)
        .body(axum::body::Body::empty())
        .map_err(|err| AppError::Internal(err.into()))
}

async fn video_original(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    redirect_to_api_path(&format!("/api/v1/movies/{}/direct", id))
}

async fn video_stream(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    redirect_to_api_path(&format!("/api/v1/movies/{}/direct", id))
}

async fn video_stream_mp4(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    redirect_to_api_path(&format!("/api/v1/movies/{}/stream.mp4", id))
}

async fn video_hls_playlist(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    redirect_to_api_path(&format!("/api/v1/movies/{}/hls/master.m3u8", id))
}

async fn report_playing(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    Json(payload): Json<Value>,
) -> Result<StatusCode, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    tracing::debug!(user = %auth.claims.sub, payload = ?payload, "Emby session playing event");
    Ok(StatusCode::NO_CONTENT)
}

async fn report_playing_progress(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    Json(payload): Json<Value>,
) -> Result<StatusCode, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    tracing::debug!(user = %auth.claims.sub, payload = ?payload, "Emby session progress event");
    Ok(StatusCode::NO_CONTENT)
}

async fn report_playing_stopped(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    Json(payload): Json<Value>,
) -> Result<StatusCode, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    tracing::debug!(user = %auth.claims.sub, payload = ?payload, "Emby session stopped event");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use std::sync::Mutex;
    use tower::ServiceExt;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        entries: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn set(entries: &[(&'static str, &'static str)]) -> Self {
            let lock = ENV_MUTEX.lock().unwrap();
            let mut previous = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                previous.push((*key, std::env::var(key).ok()));
                std::env::set_var(key, value);
            }
            Self {
                _lock: lock,
                entries: previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, previous) in &self.entries {
                if let Some(value) = previous {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    async fn build_app() -> axum::Router {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Blade Runner 2049".to_string(),
            year: Some(2017),
            file_path: std::path::PathBuf::from("/movies/blade-runner-2049.mkv"),
            poster_url: Some("https://example.com/poster.jpg".to_string()),
            overview: Some("Officer K uncovers a secret.".to_string()),
            tmdb_id: Some(335984),
            runtime_minutes: Some(164),
            added_at: 1_717_896_000,
            file_size: Some(4_000_000_000),
        })
        .await
        .unwrap();
        crate::api::app_router(db)
    }

    async fn emby_login(app: axum::Router) -> String {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/Users/AuthenticateByName")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "Username": "admin",
                            "Pw": "admin"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        value
            .get("AccessToken")
            .and_then(Value::as_str)
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn test_public_system_info_is_available_on_emby_prefix() {
        let app = build_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/emby/System/Info/Public")
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value.get("ProductName").and_then(Value::as_str), Some("Emby"));
        assert_eq!(value.get("LocalAddress").and_then(Value::as_str), Some("http://media.test"));
    }

    #[tokio::test]
    async fn test_auth_views_and_items_flow() {
        let _guard = EnvGuard::set(&[
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "emby-test-secret"),
        ]);
        let app = build_app().await;
        let token = emby_login(app.clone()).await;

        let views_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/Users/{}/Views?api_key={}", VIRTUAL_USER_ID, token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(views_response.status(), 200);

        let items_response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/Items?ParentId=movies&api_key={}", token))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(items_response.status(), 200);

        let body = axum::body::to_bytes(items_response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value.get("TotalRecordCount").and_then(Value::as_u64), Some(1));
        assert_eq!(value["Items"][0]["Name"].as_str(), Some("Blade Runner 2049"));
        assert!(value["Items"][0]["ImagePath"].as_str().unwrap().contains("/Items/1/Images/Primary"));
    }

    #[tokio::test]
    async fn test_playback_info_exposes_direct_and_transcode_urls() {
        let _guard = EnvGuard::set(&[
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "emby-test-secret"),
        ]);
        let app = build_app().await;
        let token = emby_login(app.clone()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/emby/Items/1/PlaybackInfo?api_key={}", token))
                    .header("host", "media.test")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let media_source = &value["MediaSources"][0];
        assert!(media_source["DirectStreamUrl"].as_str().unwrap().contains("/emby/Videos/1/original"));
        assert!(media_source["TranscodingUrl"].as_str().unwrap().contains("/emby/Videos/1/stream.mp4"));
    }

    #[tokio::test]
    async fn test_primary_image_redirects_to_poster_url() {
        let _guard = EnvGuard::set(&[
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "emby-test-secret"),
        ]);
        let app = build_app().await;
        let token = emby_login(app.clone()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/Items/1/Images/Primary?api_key={}", token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response.headers().get(header::LOCATION).and_then(|value| value.to_str().ok()),
            Some("https://example.com/poster.jpg")
        );
    }

    #[tokio::test]
    async fn test_session_progress_endpoints_accept_emby_token() {
        let _guard = EnvGuard::set(&[
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "emby-test-secret"),
        ]);
        let app = build_app().await;
        let token = emby_login(app.clone()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/Sessions/Playing/Progress")
                    .header("X-Emby-Token", token)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
}