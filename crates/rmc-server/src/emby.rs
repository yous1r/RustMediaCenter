use axum::{
    extract::{OriginalUri, Path, Query, State},
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use rmc_core::api_types::{LibraryItem, LibraryItemKind};
use rmc_core::models::Movie;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::AppState;
use crate::auth::Claims;
use crate::db::Database;
use crate::error::AppError;
use crate::media_tree::{MediaTree, ROOT_COLLECTION_ID};

const SERVER_ID: &str = "rust-media-center";
const VIRTUAL_USER_ID: &str = "1";
const DEFAULT_SERVER_NAME: &str = "RustMediaCenter";
const MOVIES_VIEW_ID: &str = "movies";
const TVSHOWS_VIEW_ID: &str = "tvshows";
const MAX_RUNTIME_PROBE_ITEMS_PER_IDS_QUERY: usize = 8;

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
        .route("/Shows/:series_id/Seasons", get(show_seasons))
        .route("/Shows/:series_id/Episodes", get(show_episodes))
        .route(
            "/Users/:user_id/Shows/:series_id/Seasons",
            get(user_show_seasons),
        )
        .route(
            "/Users/:user_id/Shows/:series_id/Episodes",
            get(user_show_episodes),
        )
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
    #[serde(default, alias = "Recursive", alias = "recursive")]
    recursive: Option<bool>,
    #[serde(default, alias = "IncludeItemTypes", alias = "includeItemTypes")]
    include_item_types: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ShowChildrenQuery {
    #[serde(default, alias = "api_key", alias = "ApiKey")]
    api_key: Option<String>,
    #[serde(default, alias = "SeasonId", alias = "seasonId")]
    season_id: Option<String>,
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
        .runtime_seconds
        .map(|seconds| seconds as u64 * 10_000_000)
        .or_else(|| {
            movie
                .runtime_minutes
                .map(|minutes| minutes as u64 * 60 * 10_000_000)
        })
}

fn library_item_runtime_ticks(item: &LibraryItem) -> Option<u64> {
    item.runtime_seconds
        .map(|seconds| seconds as u64 * 10_000_000)
        .or_else(|| {
            item.runtime_minutes
                .map(|minutes| minutes as u64 * 60 * 10_000_000)
        })
}

fn build_media_source(
    item: &LibraryItem,
    movie: &Movie,
    headers: &HeaderMap,
    prefix: &str,
    token: &str,
) -> Value {
    let direct_url = with_api_key(
        absolute_url(headers, prefix, &format!("/Videos/{}/original", item.id)),
        token,
    );
    let transcode_url = with_api_key(
        absolute_url(headers, prefix, &format!("/Videos/{}/stream.mp4", item.id)),
        token,
    );

    json!({
        "Id": format!("item-{}", item.id.replace(':', "-")),
        "ItemId": item.id,
        "Name": item.title,
        "Type": "Default",
        "Protocol": "Http",
        "Path": direct_url,
        "Container": movie_container(movie),
        "Size": item.file_size.or(movie.file_size),
        "RunTimeTicks": library_item_runtime_ticks(item).or_else(|| runtime_ticks(movie)),
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

fn emby_item_type(kind: &LibraryItemKind) -> &'static str {
    match kind {
        LibraryItemKind::Movie => "Movie",
        LibraryItemKind::Series => "Series",
        LibraryItemKind::Season => "Season",
        LibraryItemKind::Episode => "Episode",
    }
}

fn emby_collection_type(kind: &LibraryItemKind) -> Option<&'static str> {
    match kind {
        LibraryItemKind::Movie => Some("movies"),
        LibraryItemKind::Series => Some("tvshows"),
        LibraryItemKind::Season | LibraryItemKind::Episode => None,
    }
}

fn is_folder_item(kind: &LibraryItemKind) -> bool {
    matches!(kind, LibraryItemKind::Series | LibraryItemKind::Season)
}

fn is_virtual_view_id(id: &str) -> bool {
    matches!(id, MOVIES_VIEW_ID | TVSHOWS_VIEW_ID)
}

fn virtual_view_item(id: &str, child_count: usize) -> Option<Value> {
    match id {
        MOVIES_VIEW_ID => Some(json!({
            "Name": "Movies",
            "ServerId": SERVER_ID,
            "Id": MOVIES_VIEW_ID,
            "Type": "CollectionFolder",
            "CollectionType": "movies",
            "MediaType": "Video",
            "IsFolder": true,
            "ChildCount": child_count,
            "RecursiveItemCount": child_count,
            "ImageTags": {}
        })),
        TVSHOWS_VIEW_ID => Some(json!({
            "Name": "TV Shows",
            "ServerId": SERVER_ID,
            "Id": TVSHOWS_VIEW_ID,
            "Type": "CollectionFolder",
            "CollectionType": "tvshows",
            "MediaType": "Video",
            "IsFolder": true,
            "ChildCount": child_count,
            "RecursiveItemCount": child_count,
            "ImageTags": {}
        })),
        _ => None,
    }
}

fn virtual_view_items(tree: &MediaTree) -> Vec<Value> {
    let root_items = tree.paged_items(None, None, 0, None).items;
    let movie_count = root_items
        .iter()
        .filter(|item| matches!(item.kind, LibraryItemKind::Movie))
        .count();
    let series_count = root_items
        .iter()
        .filter(|item| matches!(item.kind, LibraryItemKind::Series))
        .count();

    vec![
        virtual_view_item(MOVIES_VIEW_ID, movie_count).unwrap(),
        virtual_view_item(TVSHOWS_VIEW_ID, series_count).unwrap(),
    ]
}

fn filter_items_for_parent(items: Vec<LibraryItem>, parent_id: Option<&str>) -> Vec<LibraryItem> {
    match parent_id.map(str::trim) {
        Some(MOVIES_VIEW_ID) => items
            .into_iter()
            .filter(|item| matches!(item.kind, LibraryItemKind::Movie))
            .collect(),
        Some(TVSHOWS_VIEW_ID) => items
            .into_iter()
            .filter(|item| matches!(item.kind, LibraryItemKind::Series))
            .collect(),
        _ => items,
    }
}

fn append_recursive_items(
    tree: &MediaTree,
    parent_id: Option<&str>,
    output: &mut Vec<LibraryItem>,
) {
    for item in tree.paged_items(parent_id, None, 0, None).items {
        let child_parent_id = item.id.clone();
        output.push(item);
        append_recursive_items(tree, Some(&child_parent_id), output);
    }
}

fn recursive_items_for_parent(tree: &MediaTree, parent_id: Option<&str>) -> Vec<LibraryItem> {
    match parent_id.map(str::trim) {
        Some(MOVIES_VIEW_ID) => tree
            .paged_items(None, None, 0, None)
            .items
            .into_iter()
            .filter(|item| matches!(item.kind, LibraryItemKind::Movie))
            .collect(),
        Some(TVSHOWS_VIEW_ID) => {
            let mut items = Vec::new();
            for series in tree
                .paged_items(None, None, 0, None)
                .items
                .into_iter()
                .filter(|item| matches!(item.kind, LibraryItemKind::Series))
            {
                let series_id = series.id.clone();
                items.push(series);
                append_recursive_items(tree, Some(&series_id), &mut items);
            }
            items
        }
        _ => {
            let mut items = Vec::new();
            append_recursive_items(tree, normalize_parent_id(parent_id), &mut items);
            items
        }
    }
}

fn parse_include_item_types(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn item_matches_include_types(item: &LibraryItem, include_types: &[String]) -> bool {
    if include_types.is_empty() {
        return true;
    }

    let item_type = emby_item_type(&item.kind).to_ascii_lowercase();
    include_types
        .iter()
        .any(|value| value == &item_type || (value == "video" && item.play_id.is_some()))
}

fn build_library_item(
    item: &LibraryItem,
    headers: &HeaderMap,
    prefix: &str,
    token: Option<&str>,
) -> Value {
    let is_playable = item.play_id.is_some();
    let is_folder = is_folder_item(&item.kind);
    let image_url = item.poster_url.as_ref().map(|_| {
        let url = absolute_url(
            headers,
            prefix,
            &format!("/Items/{}/Images/Primary", item.id),
        );
        match token {
            Some(value) => with_api_key(url, value),
            None => url,
        }
    });

    json!({
        "Name": item.title,
        "ServerId": SERVER_ID,
        "Id": item.id,
        "Type": emby_item_type(&item.kind),
        "MediaType": "Video",
        "CollectionType": emby_collection_type(&item.kind),
        "Overview": item.overview,
        "ProductionYear": item.year,
        "RunTimeTicks": library_item_runtime_ticks(item),
        "ImageTags": item.poster_url.as_ref().map(|_| json!({ "Primary": format!("poster-{}", item.id.replace(':', "-")) })).unwrap_or_else(|| json!({})),
        "ImageBlurHashes": {},
        "PrimaryImageAspectRatio": serde_json::Value::Null,
        "PrimaryImageItemId": item.poster_url.as_ref().map(|_| item.id.clone()),
        "ImagePath": image_url,
        "CanPlay": is_playable,
        "CanDownload": is_playable,
        "PlayAccess": if is_playable { "Full" } else { "None" },
        "LocationType": if is_playable { "FileSystem" } else { "Virtual" },
        "RecursiveItemCount": item.child_count,
        "ChildCount": item.child_count,
        "IsFolder": is_folder,
        "IsPlayable": is_playable,
        "Container": serde_json::Value::Null,
        "ParentId": item.parent_id,
        "IndexNumber": item.episode_number.or(item.season_number),
        "ParentIndexNumber": if matches!(item.kind, LibraryItemKind::Episode) { item.season_number } else { None },
        "UserData": {
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": false
        }
    })
}

fn parse_item_ids(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn matches_search_term(item: &LibraryItem, search_term: &str) -> bool {
    let normalized = search_term.trim().to_ascii_lowercase();
    item.title.to_ascii_lowercase().contains(&normalized)
        || item
            .overview
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(&normalized)
}

fn sort_library_items(items: &mut [LibraryItem], sort_by: Option<&str>, sort_order: Option<&str>) {
    let descending = sort_order
        .map(|value| value.eq_ignore_ascii_case("descending"))
        .unwrap_or(false);

    match sort_by {
        Some(value)
            if value.eq_ignore_ascii_case("DateCreated")
                || value.eq_ignore_ascii_case("DateAdded") =>
        {
            items.sort_by_key(|item| item.added_at);
        }
        Some(_) => {
            items.sort_by(|a, b| {
                a.title
                    .to_ascii_lowercase()
                    .cmp(&b.title.to_ascii_lowercase())
            });
        }
        None => {}
    }

    if descending {
        items.reverse();
    }
}

fn normalize_parent_id(parent_id: Option<&str>) -> Option<&str> {
    let parent_id = parent_id?.trim();
    if parent_id.is_empty()
        || is_virtual_view_id(parent_id)
        || parent_id == ROOT_COLLECTION_ID
        || parent_id == VIRTUAL_USER_ID
    {
        None
    } else {
        Some(parent_id)
    }
}

async fn load_media_tree(db: &Database) -> Result<MediaTree, AppError> {
    let movies = db
        .get_available_movies()
        .await
        .map_err(|err| AppError::Internal(err.into()))?;
    Ok(MediaTree::from_movies(movies))
}

async fn backfill_runtime_for_items(
    db: &Database,
    items: Vec<LibraryItem>,
) -> Result<Vec<LibraryItem>, AppError> {
    let mut hydrated = Vec::with_capacity(items.len());

    for mut item in items {
        if let Some(play_id) = item.play_id.filter(|_| item.runtime_seconds.is_none()) {
            let movie = crate::media_probe::load_movie_with_runtime(db, play_id)
                .await
                .map_err(|err| AppError::Internal(err))?;
            item.runtime_seconds = movie.runtime_seconds;
            item.runtime_minutes = movie.runtime_minutes;
        }
        hydrated.push(item);
    }

    Ok(hydrated)
}

async fn load_library_items(
    db: &Database,
    query: &ItemsQuery,
) -> Result<Vec<LibraryItem>, AppError> {
    let tree = load_media_tree(db).await?;
    let parent_id = query.parent_id.as_deref().map(str::trim);
    let mut items = if let Some(ids) = query
        .ids
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let requested_ids = parse_item_ids(ids);
        let should_probe_runtime = requested_ids.len() <= MAX_RUNTIME_PROBE_ITEMS_PER_IDS_QUERY;
        let items = requested_ids
            .into_iter()
            .filter(|id| !is_virtual_view_id(id))
            .filter_map(|id| tree.item(&id))
            .collect::<Vec<_>>();
        if should_probe_runtime {
            backfill_runtime_for_items(db, items).await?
        } else {
            items
        }
    } else if query.recursive.unwrap_or(false) {
        recursive_items_for_parent(&tree, parent_id)
    } else {
        tree.paged_items(
            normalize_parent_id(parent_id),
            query.search_term.as_deref(),
            0,
            None,
        )
        .items
    };

    if !query.recursive.unwrap_or(false) {
        items = filter_items_for_parent(items, parent_id);
    }
    let include_types = parse_include_item_types(query.include_item_types.as_deref());
    items.retain(|item| item_matches_include_types(item, &include_types));

    if let Some(search_term) = query
        .search_term
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if query.ids.is_some() || query.recursive.unwrap_or(false) {
            items.retain(|item| matches_search_term(item, search_term));
        }
    }

    sort_library_items(
        &mut items,
        query.sort_by.as_deref(),
        query.sort_order.as_deref(),
    );
    Ok(items)
}

async fn get_library_item(db: &Database, id: &str) -> Result<LibraryItem, AppError> {
    let tree = load_media_tree(db).await?;
    tree.item(id)
        .ok_or_else(|| AppError::NotFound(format!("Item {} not found", id)))
}

async fn get_virtual_view_detail(db: &Database, id: &str) -> Result<Value, AppError> {
    let tree = load_media_tree(db).await?;
    virtual_view_items(&tree)
        .into_iter()
        .find(|item| item["Id"].as_str() == Some(id))
        .ok_or_else(|| AppError::NotFound(format!("Item {} not found", id)))
}

async fn get_playable_item_and_movie(
    db: &Database,
    id: &str,
) -> Result<(LibraryItem, Movie), AppError> {
    let item = get_library_item(db, id).await?;
    let play_id = item
        .play_id
        .ok_or_else(|| AppError::NotFound(format!("Item {} is not playable", id)))?;
    let movie = crate::media_probe::load_movie_with_runtime(db, play_id)
        .await
        .map_err(|err| match err.downcast::<sqlx::Error>() {
            Ok(sqlx::Error::RowNotFound) => {
                AppError::NotFound(format!("Movie id {} not found", play_id))
            }
            Ok(other) => AppError::Internal(other.into()),
            Err(other) => AppError::Internal(other),
        })?;
    Ok((item, movie))
}

async fn get_playable_movie_id(db: &Database, id: &str) -> Result<i64, AppError> {
    let (item, _) = get_playable_item_and_movie(db, id).await?;
    item.play_id
        .ok_or_else(|| AppError::NotFound(format!("Item {} is not playable", id)))
}

fn paged_items_response(items: Vec<Value>, total: usize, start_index: usize) -> Value {
    json!({
        "Items": items,
        "TotalRecordCount": total,
        "StartIndex": start_index
    })
}

async fn system_info_public(headers: HeaderMap) -> Result<Json<Value>, AppError> {
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

    let access_token =
        crate::auth::create_jwt(&username).map_err(|err| AppError::Internal(err.into()))?;
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
    let tree = load_media_tree(&db).await?;
    let items = virtual_view_items(&tree);

    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": 2,
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
    let items = load_library_items(&db, &query).await?;
    let total = items.len();
    let start_index = query.start_index.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(total.saturating_sub(start_index));
    let items = items
        .into_iter()
        .skip(start_index)
        .take(limit)
        .map(|item| build_library_item(&item, &headers, prefix, Some(&auth.token)))
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
    let items = load_media_tree(&db)
        .await?
        .playable_items_sorted_by_added_at()
        .into_iter()
        .take(50)
        .map(|item| build_library_item(&item, &headers, prefix, Some(&auth.token)))
        .collect::<Vec<_>>();

    Ok(Json(Value::Array(items)))
}

async fn show_seasons(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ShowChildrenQuery>,
    State(db): State<AppState>,
    Path(series_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    let prefix = route_prefix(&original_uri.0);
    let tree = load_media_tree(&db).await?;
    let mut seasons = tree.paged_items(Some(&series_id), None, 0, None).items;
    seasons.retain(|item| matches!(item.kind, LibraryItemKind::Season));
    sort_library_items(
        &mut seasons,
        query.sort_by.as_deref(),
        query.sort_order.as_deref(),
    );

    let total = seasons.len();
    let start_index = query.start_index.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(total.saturating_sub(start_index));
    let items = seasons
        .into_iter()
        .skip(start_index)
        .take(limit)
        .map(|item| build_library_item(&item, &headers, prefix, Some(&auth.token)))
        .collect::<Vec<_>>();

    Ok(Json(paged_items_response(items, total, start_index)))
}

async fn user_show_seasons(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ShowChildrenQuery>,
    State(db): State<AppState>,
    Path((_user_id, series_id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    show_seasons(
        headers,
        original_uri,
        Query(query),
        State(db),
        Path(series_id),
    )
    .await
}

async fn show_episodes(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ShowChildrenQuery>,
    State(db): State<AppState>,
    Path(series_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    let prefix = route_prefix(&original_uri.0);
    let tree = load_media_tree(&db).await?;
    let mut episodes = if let Some(season_id) = query
        .season_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        tree.paged_items(Some(season_id.trim()), None, 0, None)
            .items
    } else {
        recursive_items_for_parent(&tree, Some(&series_id))
    };
    episodes.retain(|item| matches!(item.kind, LibraryItemKind::Episode));
    sort_library_items(
        &mut episodes,
        query.sort_by.as_deref(),
        query.sort_order.as_deref(),
    );

    let total = episodes.len();
    let start_index = query.start_index.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(total.saturating_sub(start_index));
    let items = episodes
        .into_iter()
        .skip(start_index)
        .take(limit)
        .map(|item| build_library_item(&item, &headers, prefix, Some(&auth.token)))
        .collect::<Vec<_>>();

    Ok(Json(paged_items_response(items, total, start_index)))
}

async fn user_show_episodes(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ShowChildrenQuery>,
    State(db): State<AppState>,
    Path((_user_id, series_id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    show_episodes(
        headers,
        original_uri,
        Query(query),
        State(db),
        Path(series_id),
    )
    .await
}

async fn items(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ItemsQuery>,
    State(db): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let auth = require_auth(&headers, query.api_key.as_deref())?;
    let prefix = route_prefix(&original_uri.0);
    let items = load_library_items(&db, &query).await?;
    let total = items.len();
    let start_index = query.start_index.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(total.saturating_sub(start_index));
    let items = items
        .into_iter()
        .skip(start_index)
        .take(limit)
        .map(|item| build_library_item(&item, &headers, prefix, Some(&auth.token)))
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
    if is_virtual_view_id(&id) {
        return Ok(Json(get_virtual_view_detail(&db, &id).await?));
    }

    let item = get_library_item(&db, &id).await?;
    let prefix = route_prefix(&original_uri.0);
    let mut response = build_library_item(&item, &headers, prefix, Some(&auth.token));

    if item.play_id.is_some() {
        let (_, movie) = get_playable_item_and_movie(&db, &id).await?;
        response["RunTimeTicks"] = json!(runtime_ticks(&movie));
        response["MediaSources"] = json!([build_media_source(
            &item,
            &movie,
            &headers,
            prefix,
            &auth.token,
        )]);
    }

    Ok(Json(response))
}

async fn user_item_detail(
    headers: HeaderMap,
    original_uri: OriginalUri,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path((_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    item_detail(
        headers,
        original_uri,
        Query(query),
        State(db),
        Path(item_id),
    )
    .await
}

async fn primary_image(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    let item = get_library_item(&db, &id).await?;
    let poster_url = item
        .poster_url
        .ok_or_else(|| AppError::NotFound(format!("Item {} has no poster", id)))?;
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
    let (item, movie) = get_playable_item_and_movie(&db, &id).await?;
    let prefix = route_prefix(&original_uri.0);
    let play_session_id = format!("play-{}-{}", movie.id, Utc::now().timestamp_millis());

    Ok(Json(json!({
        "PlaySessionId": play_session_id,
        "MediaSources": [build_media_source(&item, &movie, &headers, prefix, &auth.token)]
    })))
}

fn redirect_to_api_path(headers: &HeaderMap, path: &str) -> Result<Response, AppError> {
    let location = absolute_url(headers, "", path);
    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(header::LOCATION, location)
        .body(axum::body::Body::empty())
        .map_err(|err| AppError::Internal(err.into()))
}

async fn video_original(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    let movie_id = get_playable_movie_id(&db, &id).await?;
    redirect_to_api_path(&headers, &format!("/api/v1/movies/{}/direct", movie_id))
}

async fn video_stream(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    let movie_id = get_playable_movie_id(&db, &id).await?;
    redirect_to_api_path(&headers, &format!("/api/v1/movies/{}/direct", movie_id))
}

async fn video_stream_mp4(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    let movie_id = get_playable_movie_id(&db, &id).await?;
    redirect_to_api_path(&headers, &format!("/api/v1/movies/{}/stream.mp4", movie_id))
}

async fn video_hls_playlist(
    headers: HeaderMap,
    Query(query): Query<ApiKeyQuery>,
    State(db): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let _auth = require_auth(&headers, query.api_key.as_deref())?;
    let movie_id = get_playable_movie_id(&db, &id).await?;
    redirect_to_api_path(
        &headers,
        &format!("/api/v1/movies/{}/hls/master.m3u8", movie_id),
    )
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
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;
    use tower::ServiceExt;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        entries: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn set(entries: &[(&'static str, &'static str)]) -> Self {
            let lock = ENV_MUTEX
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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

        fn set_owned(entries: &[(&'static str, String)]) -> Self {
            let lock = ENV_MUTEX
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let temp_root = tempfile::tempdir().unwrap().keep();
        let movie_path = temp_root.join("movies").join("blade-runner-2049.mkv");
        let episode_one_path = temp_root
            .join("shows")
            .join("Tiny World")
            .join("Season 1")
            .join("Tiny.World.S01E01.mkv");
        let episode_two_path = temp_root
            .join("shows")
            .join("Tiny World")
            .join("Season 1")
            .join("Tiny.World.S01E02.mkv");

        std::fs::create_dir_all(movie_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(episode_one_path.parent().unwrap()).unwrap();
        std::fs::write(&movie_path, b"movie").unwrap();
        std::fs::write(&episode_one_path, b"episode-one").unwrap();
        std::fs::write(&episode_two_path, b"episode-two").unwrap();

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Blade Runner 2049".to_string(),
            year: Some(2017),
            file_path: movie_path,
            poster_url: Some("https://example.com/poster.jpg".to_string()),
            overview: Some("Officer K uncovers a secret.".to_string()),
            tmdb_id: Some(335984),
            runtime_minutes: Some(164),
            runtime_seconds: Some(9_840),
            added_at: 1_717_896_000,
            file_size: Some(4_000_000_000),
        })
        .await
        .unwrap();
        db.insert_movie(&Movie {
            id: 2,
            title: "Tiny World".to_string(),
            year: Some(2020),
            file_path: episode_one_path,
            poster_url: Some("https://example.com/tiny-world.jpg".to_string()),
            overview: Some("Nature documentary episode one.".to_string()),
            tmdb_id: None,
            runtime_minutes: Some(30),
            runtime_seconds: Some(1_800),
            added_at: 1_717_896_100,
            file_size: Some(1_000_000_000),
        })
        .await
        .unwrap();
        db.insert_movie(&Movie {
            id: 3,
            title: "Tiny World".to_string(),
            year: Some(2020),
            file_path: episode_two_path,
            poster_url: Some("https://example.com/tiny-world.jpg".to_string()),
            overview: Some("Nature documentary episode two.".to_string()),
            tmdb_id: None,
            runtime_minutes: Some(31),
            runtime_seconds: Some(1_860),
            added_at: 1_717_896_200,
            file_size: Some(1_100_000_000),
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        value
            .get("AccessToken")
            .and_then(Value::as_str)
            .unwrap()
            .to_string()
    }

    fn write_fake_ffprobe(temp_dir: &tempfile::TempDir) -> std::path::PathBuf {
        let script_path = temp_dir.path().join("fake_ffprobe.sh");
        let script = r#"#!/bin/sh
printf '{"format":{"duration":"187.4"}}\n'
"#;
        std::fs::write(&script_path, script).unwrap();
        let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).unwrap();
        script_path
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value.get("ProductName").and_then(Value::as_str),
            Some("Emby")
        );
        assert_eq!(
            value.get("LocalAddress").and_then(Value::as_str),
            Some("http://media.test")
        );
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
                    .uri(format!(
                        "/Users/{}/Views?api_key={}",
                        VIRTUAL_USER_ID, token
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(views_response.status(), 200);

        let views_body = axum::body::to_bytes(views_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let views_value: Value = serde_json::from_slice(&views_body).unwrap();
        assert_eq!(views_value["TotalRecordCount"].as_u64(), Some(2));
        assert!(views_value["Items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["Id"].as_str() == Some(MOVIES_VIEW_ID)
                && item["CollectionType"].as_str() == Some("movies")));
        assert!(views_value["Items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["Id"].as_str() == Some(TVSHOWS_VIEW_ID)
                && item["CollectionType"].as_str() == Some("tvshows")));

        let items_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/Items?ParentId={}&api_key={}",
                        MOVIES_VIEW_ID, token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(items_response.status(), 200);

        let body = axum::body::to_bytes(items_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value.get("TotalRecordCount").and_then(Value::as_u64),
            Some(1)
        );
        assert!(value["Items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["Name"].as_str() == Some("Blade Runner 2049")));

        let tv_response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/Items?ParentId={}&api_key={}",
                        TVSHOWS_VIEW_ID, token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tv_response.status(), 200);

        let tv_body = axum::body::to_bytes(tv_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tv_value: Value = serde_json::from_slice(&tv_body).unwrap();
        assert_eq!(
            tv_value.get("TotalRecordCount").and_then(Value::as_u64),
            Some(1)
        );
        assert!(tv_value["Items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["Name"].as_str() == Some("Tiny World")
                && item["Type"].as_str() == Some("Series")));
    }

    #[tokio::test]
    async fn test_emby_items_follow_series_season_episode_hierarchy() {
        let _guard = EnvGuard::set(&[
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "emby-test-secret"),
        ]);
        let app = build_app().await;
        let token = emby_login(app.clone()).await;

        let root_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/Items?ParentId={}&api_key={}",
                        TVSHOWS_VIEW_ID, token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(root_response.status(), 200);

        let root_body = axum::body::to_bytes(root_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let root_value: Value = serde_json::from_slice(&root_body).unwrap();
        let series_id = root_value["Items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["Type"].as_str() == Some("Series"))
            .and_then(|item| item["Id"].as_str())
            .unwrap()
            .to_string();

        let seasons_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/Items?ParentId={}&api_key={}", series_id, token))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(seasons_response.status(), 200);

        let seasons_body = axum::body::to_bytes(seasons_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let seasons_value: Value = serde_json::from_slice(&seasons_body).unwrap();
        assert_eq!(seasons_value["TotalRecordCount"].as_u64(), Some(1));
        let season_id = seasons_value["Items"][0]["Id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(seasons_value["Items"][0]["Type"].as_str(), Some("Season"));

        let episodes_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/Items?ParentId={}&api_key={}", season_id, token))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(episodes_response.status(), 200);

        let episodes_body = axum::body::to_bytes(episodes_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let episodes_value: Value = serde_json::from_slice(&episodes_body).unwrap();
        assert_eq!(episodes_value["TotalRecordCount"].as_u64(), Some(2));
        assert_eq!(episodes_value["Items"][0]["Type"].as_str(), Some("Episode"));
        assert_eq!(episodes_value["Items"][0]["CanPlay"].as_bool(), Some(true));
        assert_eq!(
            episodes_value["Items"][0]["IsPlayable"].as_bool(),
            Some(true)
        );
        assert_eq!(
            episodes_value["Items"][0]["PlayAccess"].as_str(),
            Some("Full")
        );
        assert_eq!(
            episodes_value["Items"][0]["LocationType"].as_str(),
            Some("FileSystem")
        );
        let episode_id = episodes_value["Items"][0]["Id"]
            .as_str()
            .unwrap()
            .to_string();

        let detail_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/emby/Users/{}/Items/{}?api_key={}",
                        VIRTUAL_USER_ID, episode_id, token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail_response.status(), 200);

        let detail_body = axum::body::to_bytes(detail_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail_value: Value = serde_json::from_slice(&detail_body).unwrap();
        assert_eq!(detail_value["Type"].as_str(), Some("Episode"));
        assert_eq!(detail_value["CanPlay"].as_bool(), Some(true));
        assert_eq!(detail_value["IsPlayable"].as_bool(), Some(true));
        assert_eq!(detail_value["PlayAccess"].as_str(), Some("Full"));
        assert_eq!(
            detail_value["MediaSources"][0]["DirectStreamUrl"]
                .as_str()
                .map(|value| value.contains("/emby/Videos/episode:")),
            Some(true)
        );

        let playback_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/emby/Items/{}/PlaybackInfo?api_key={}",
                        episode_id, token
                    ))
                    .header("host", "media.test")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(playback_response.status(), 200);

        let playback_body = axum::body::to_bytes(playback_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let playback_value: Value = serde_json::from_slice(&playback_body).unwrap();
        assert!(playback_value["MediaSources"][0]["DirectStreamUrl"]
            .as_str()
            .unwrap()
            .contains("/emby/Videos/episode:2/original"));
    }

    #[tokio::test]
    async fn test_emby_show_seasons_and_episodes_endpoints_drive_series_detail() {
        let _guard = EnvGuard::set(&[
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "emby-test-secret"),
        ]);
        let app = build_app().await;
        let token = emby_login(app.clone()).await;

        let series_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/Items?ParentId={}&api_key={}",
                        TVSHOWS_VIEW_ID, token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(series_response.status(), 200);

        let series_body = axum::body::to_bytes(series_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let series_value: Value = serde_json::from_slice(&series_body).unwrap();
        let series_id = series_value["Items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["Type"].as_str() == Some("Series"))
            .and_then(|item| item["Id"].as_str())
            .unwrap()
            .to_string();

        let seasons_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/emby/Shows/{}/Seasons?api_key={}",
                        series_id, token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(seasons_response.status(), 200);

        let seasons_body = axum::body::to_bytes(seasons_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let seasons_value: Value = serde_json::from_slice(&seasons_body).unwrap();
        assert_eq!(seasons_value["TotalRecordCount"].as_u64(), Some(1));
        assert_eq!(seasons_value["Items"][0]["Type"].as_str(), Some("Season"));
        assert_eq!(seasons_value["Items"][0]["IsFolder"].as_bool(), Some(true));
        let season_id = seasons_value["Items"][0]["Id"]
            .as_str()
            .unwrap()
            .to_string();

        let season_episodes_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/emby/Shows/{}/Episodes?SeasonId={}&api_key={}",
                        series_id, season_id, token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(season_episodes_response.status(), 200);

        let season_episodes_body =
            axum::body::to_bytes(season_episodes_response.into_body(), usize::MAX)
                .await
                .unwrap();
        let season_episodes_value: Value = serde_json::from_slice(&season_episodes_body).unwrap();
        assert_eq!(season_episodes_value["TotalRecordCount"].as_u64(), Some(2));
        assert!(season_episodes_value["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| {
                item["Type"].as_str() == Some("Episode")
                    && item["CanPlay"].as_bool() == Some(true)
                    && item["IsPlayable"].as_bool() == Some(true)
                    && item["PlayAccess"].as_str() == Some("Full")
            }));

        let all_episodes_response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/emby/Users/{}/Shows/{}/Episodes?api_key={}",
                        VIRTUAL_USER_ID, series_id, token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(all_episodes_response.status(), 200);

        let all_episodes_body = axum::body::to_bytes(all_episodes_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let all_episodes_value: Value = serde_json::from_slice(&all_episodes_body).unwrap();
        assert_eq!(all_episodes_value["TotalRecordCount"].as_u64(), Some(2));
    }

    #[tokio::test]
    async fn test_emby_recursive_items_return_playable_leaf_items() {
        let _guard = EnvGuard::set(&[
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "emby-test-secret"),
        ]);
        let app = build_app().await;
        let token = emby_login(app.clone()).await;

        let tv_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/Items?ParentId={}&Recursive=true&IncludeItemTypes=Episode&api_key={}",
                        TVSHOWS_VIEW_ID, token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tv_response.status(), 200);

        let tv_body = axum::body::to_bytes(tv_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let tv_value: Value = serde_json::from_slice(&tv_body).unwrap();
        assert_eq!(tv_value["TotalRecordCount"].as_u64(), Some(2));
        assert!(tv_value["Items"].as_array().unwrap().iter().all(|item| {
            item["Type"].as_str() == Some("Episode")
                && item["IsFolder"].as_bool() == Some(false)
                && item["IsPlayable"].as_bool() == Some(true)
                && item["CanPlay"].as_bool() == Some(true)
                && item["PlayAccess"].as_str() == Some("Full")
                && item["CanDownload"].as_bool() == Some(true)
        }));

        let all_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/Items?Recursive=true&IncludeItemTypes=Movie,Episode&api_key={}",
                        token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(all_response.status(), 200);

        let all_body = axum::body::to_bytes(all_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let all_value: Value = serde_json::from_slice(&all_body).unwrap();
        let item_types = all_value["Items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["Type"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(all_value["TotalRecordCount"].as_u64(), Some(3));
        assert!(item_types.contains(&"Movie"));
        assert_eq!(
            item_types
                .iter()
                .filter(|item_type| **item_type == "Episode")
                .count(),
            2
        );

        let video_response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/Items?Recursive=true&IncludeItemTypes=Video&api_key={}",
                        token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(video_response.status(), 200);

        let video_body = axum::body::to_bytes(video_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let video_value: Value = serde_json::from_slice(&video_body).unwrap();
        assert_eq!(video_value["TotalRecordCount"].as_u64(), Some(3));
        assert!(video_value["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["IsFolder"].as_bool() == Some(false)));
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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let media_source = &value["MediaSources"][0];
        assert!(media_source["DirectStreamUrl"]
            .as_str()
            .unwrap()
            .contains("/emby/Videos/1/original"));
        assert!(media_source["TranscodingUrl"]
            .as_str()
            .unwrap()
            .contains("/emby/Videos/1/stream.mp4"));
        assert_eq!(media_source["RunTimeTicks"].as_u64(), Some(98_400_000_000));
    }

    #[tokio::test]
    async fn test_video_original_redirects_episode_to_absolute_api_url() {
        let _guard = EnvGuard::set(&[
            ("RMC_ADMIN_USERNAME", "admin"),
            ("RMC_ADMIN_PASSWORD", "admin"),
            ("RMC_JWT_SECRET", "emby-test-secret"),
        ]);
        let app = build_app().await;
        let token = emby_login(app.clone()).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/emby/Videos/episode:2/original?api_key={}", token))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("http://media.test/api/v1/movies/2/direct")
        );

        let encoded_response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/emby/Videos/episode%3A2/original?api_key={}",
                        token
                    ))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(encoded_response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            encoded_response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("http://media.test/api/v1/movies/2/direct")
        );
    }

    #[tokio::test]
    async fn test_items_query_by_ids_backfills_runtime_ticks() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ffprobe_path = write_fake_ffprobe(&temp_dir);
        let movie_path = temp_dir.path().join("movie.mp4");
        std::fs::write(&movie_path, b"movie").unwrap();

        let ffprobe_path_string = ffprobe_path.to_string_lossy().into_owned();
        let _guard = EnvGuard::set_owned(&[
            ("RMC_ADMIN_USERNAME", "admin".to_string()),
            ("RMC_ADMIN_PASSWORD", "admin".to_string()),
            ("RMC_JWT_SECRET", "emby-test-secret".to_string()),
            ("RMC_FFPROBE_CMD", ffprobe_path_string),
        ]);

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 1,
            title: "Unknown Runtime".to_string(),
            year: Some(2024),
            file_path: movie_path,
            poster_url: Some("https://example.com/poster.jpg".to_string()),
            overview: Some("Needs ffprobe.".to_string()),
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 1_717_896_000,
            file_size: Some(123_456_789),
        })
        .await
        .unwrap();
        let app = crate::api::app_router(db);
        let token = emby_login(app.clone()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/emby/Items?Ids=1&api_key={}", token))
                    .header("host", "media.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["Items"][0]["RunTimeTicks"].as_u64(),
            Some(1_870_000_000)
        );
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
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
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
