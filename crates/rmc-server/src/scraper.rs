use rmc_core::models::Movie;
use serde::Deserialize;
use std::sync::OnceLock;

const TMDB_API_URL: &str = "https://api.themoviedb.org/3/search/movie";
const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w500";
const QUERY_RELEASE_PREFIXES: &[&str] = &[
    "DMG&MH&LoliHouse",
    "Nekomoe kissaten",
    "SumiSora&MAGI ATELIER&CASO",
    "Kirara Fantasia",
    "VCB-Studio",
    "LoliHouse",
    "Kamigami",
    "ANi",
    "沸班亚马制作组",
    "天月动漫&发布组",
];

pub struct TmdbScraper {
    api_key: String,
    client: reqwest::Client,
    api_base: Option<String>,
}

#[derive(Deserialize, Debug)]
struct TmdbSearchResponse {
    results: Vec<TmdbMovie>,
}

#[derive(Deserialize, Debug)]
struct TmdbMovie {
    id: i64,
    title: String,
    poster_path: Option<String>,
    overview: Option<String>,
    release_date: Option<String>,
}

impl TmdbScraper {
    pub fn new(api_key: String, proxy_url: Option<String>, api_base: Option<String>) -> Self {
        let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10));

        let proxy_url = proxy_url.filter(|s| !s.trim().is_empty());
        let api_base = api_base.filter(|s| !s.trim().is_empty());

        if let Some(ref proxy) = proxy_url {
            if let Ok(reqwest_proxy) = reqwest::Proxy::all(proxy) {
                builder = builder.proxy(reqwest_proxy);
            }
        }

        let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
        Self {
            api_key,
            client,
            api_base,
        }
    }

    pub async fn fetch_movie_metadata(
        &self,
        title: &str,
        year: Option<u16>,
    ) -> Result<Movie, Box<dyn std::error::Error + Send + Sync>> {
        let query_candidates = build_query_candidates(title, year);

        let mut last_error = None;
        for (query_title, query_year) in query_candidates {
            tracing::debug!(
                query = query_title,
                year = query_year,
                "Searching TMDB movie metadata"
            );
            match self.search_movie(&query_title, query_year).await {
                Ok(movie) => return Ok(movie),
                Err(err) => last_error = Some(err),
            }
        }

        Err(last_error.unwrap_or_else(|| "No matching movie found on TMDB".into()))
    }

    async fn search_movie(
        &self,
        title: &str,
        year: Option<u16>,
    ) -> Result<Movie, Box<dyn std::error::Error + Send + Sync>> {
        let url = if let Some(ref base) = self.api_base {
            let base_trimmed = base.trim_end_matches('/');
            format!("{}/3/search/movie", base_trimmed)
        } else {
            TMDB_API_URL.to_string()
        };

        let mut query_params = vec![
            ("api_key", self.api_key.clone()),
            ("query", title.to_string()),
            ("language", "zh-CN".to_string()),
        ];
        if let Some(year) = year {
            query_params.push(("year", year.to_string()));
        }

        let response = self
            .client
            .get(&url)
            .query(&query_params)
            .send()
            .await
            .map_err(|e| format!("TMDB request failed for {}: {}", url, e))?;

        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await.unwrap_or_default();
            return Err(format!(
                "TMDB API error: status code {}, response: {}",
                status, err_body
            )
            .into());
        }

        let text = response.text().await?;
        let search_response: TmdbSearchResponse = serde_json::from_str(&text).map_err(|e| {
            format!(
                "Failed to parse TMDB JSON response: {}, response body: {}",
                e, text
            )
        })?;

        if search_response.results.is_empty() {
            return Err("No matching movie found on TMDB".into());
        }

        let best_match = &search_response.results[0];

        let poster_url = best_match
            .poster_path
            .as_ref()
            .map(|path| format!("{}{}", TMDB_IMAGE_BASE, path));

        let year = best_match
            .release_date
            .as_ref()
            .and_then(|date| date.split('-').next().and_then(|y| y.parse::<u16>().ok()));

        Ok(Movie {
            id: 0,
            title: best_match.title.clone(),
            year,
            file_path: std::path::PathBuf::new(),
            poster_url,
            overview: best_match.overview.clone(),
            tmdb_id: Some(best_match.id),
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        })
    }
}

fn build_query_candidates(title: &str, year: Option<u16>) -> Vec<(String, Option<u16>)> {
    let raw_title = title.trim();
    let normalized_title = normalize_query_title(raw_title);
    let primary_title = if normalized_title.is_empty() {
        raw_title.to_string()
    } else {
        normalized_title
    };

    let mut candidates = Vec::new();
    push_query_candidate(&mut candidates, primary_title.clone(), year);
    push_query_candidate(&mut candidates, primary_title.clone(), None);

    if primary_title != raw_title {
        push_query_candidate(&mut candidates, raw_title.to_string(), year);
        push_query_candidate(&mut candidates, raw_title.to_string(), None);
    }

    candidates
}

fn push_query_candidate(
    candidates: &mut Vec<(String, Option<u16>)>,
    title: String,
    year: Option<u16>,
) {
    if title.trim().is_empty() {
        return;
    }

    if !candidates
        .iter()
        .any(|(existing_title, existing_year)| existing_title == &title && existing_year == &year)
    {
        candidates.push((title, year));
    }
}

pub(crate) fn normalize_query_title(title: &str) -> String {
    let raw_title = title.trim();
    let prefix_stripped = strip_known_query_prefix(raw_title);
    let (parsed_title, _) = crate::scanner::parse_filename(prefix_stripped);

    let mut normalized = strip_known_query_prefix(parsed_title.trim()).to_string();
    normalized = strip_query_suffix_noise(&normalized);
    normalized = strip_catalog_prefix(&normalized);
    normalized = strip_query_suffix_noise(&normalized);

    normalized
        .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, '.' | '_' | '-'))
        .to_string()
}

fn strip_known_query_prefix(title: &str) -> &str {
    let trimmed = title.trim();
    for prefix in QUERY_RELEASE_PREFIXES {
        let Some(rest) = trimmed.get(prefix.len()..) else {
            continue;
        };
        if trimmed[..prefix.len()].eq_ignore_ascii_case(prefix)
            && rest
                .chars()
                .next()
                .is_some_and(|ch| ch.is_whitespace() || matches!(ch, '-' | '_' | '.'))
        {
            return rest.trim_start_matches(|ch: char| {
                ch.is_whitespace() || matches!(ch, '-' | '_' | '.')
            });
        }
    }
    trimmed
}

fn strip_query_suffix_noise(title: &str) -> String {
    static TRAILING_QUERY_NOISE_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = TRAILING_QUERY_NOISE_RE.get_or_init(|| {
        regex::Regex::new(
            r"(?ix)
            (?P<suffix>
                [ ._-]*
                (?:
                    ma\d+p
                    |(?:cm|ed|op|pv|menu|tvcm)\s*\d{0,3}
                    |nc(?:ed|op)
                    |preview
                    |tv\s+reproduction
                    |sunny\s+day
                    |live\s*\d*
                    |mv(?:\s+making\s+video)?
                    |music\s+video
                    |making\s+video
                    |animation\s+music\s+video
                    |artist\s+visual(?:\s+satsuei)?(?:\s+making\s+video)?
                    |satsuei(?:\s+making\s+video)?
                )
            )$
            ",
        )
        .unwrap()
    });

    let mut current = title.trim().to_string();
    loop {
        let next = re.replace(&current, "").trim().to_string();
        if next == current {
            return current;
        }
        current = next;
    }
}

fn strip_catalog_prefix(title: &str) -> String {
    static CATALOG_PREFIX_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = CATALOG_PREFIX_RE
        .get_or_init(|| regex::Regex::new(r"(?i)^(?:[a-z]{2,8}[ -_]?\d{3,6}[ ._-]+)+").unwrap());
    re.replace(title.trim(), "").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_query_title_removes_release_noise() {
        assert_eq!(
            normalize_query_title("Movie.Name.2021.1080p.BluRay.x265-GROUP"),
            "Movie Name"
        );
        assert_eq!(
            normalize_query_title("Tiny.World.S01E04.HDR.2160p.WEB.h265-KOGi"),
            "Tiny World"
        );
        assert_eq!(
            normalize_query_title("VCB-Studio Fate Stay Night"),
            "Fate Stay Night"
        );
        assert_eq!(
            normalize_query_title("SumiSora&MAGI ATELIER&CASO Fate Zero"),
            "Fate Zero"
        );
        assert_eq!(
            normalize_query_title("Nekomoe kissaten Goblin Slayer PV 01"),
            "Goblin Slayer"
        );
        assert_eq!(normalize_query_title("Inception"), "Inception");
    }

    #[tokio::test]
    async fn test_fetch_metadata() {
        let scraper = TmdbScraper::new("dummy_key".to_string(), None, None);
        // 期望在没有网络或错误 key 时返回清晰的 Error，而不是目前的 stub string
        let res = scraper.fetch_movie_metadata("Inception", None).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_fetch_metadata_live_mock() {
        let scraper = TmdbScraper::new("invalid_api_key_test_123".to_string(), None, None);
        let res = scraper.fetch_movie_metadata("Inception", None).await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("401")
                || err_msg.contains("Unauthorized")
                || err_msg.contains("API key")
                || err_msg.contains("status code")
                || err_msg.contains("TMDB request failed"),
            "Error message was not clear: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_fetch_metadata_custom_base() {
        let scraper = TmdbScraper::new(
            "dummy_key".to_string(),
            None,
            Some("https://invalid.domain.tmdb-proxy.com".to_string()),
        );
        let res = scraper.fetch_movie_metadata("Inception", None).await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("invalid.domain.tmdb-proxy.com"),
            "Error message did not contain custom base URL: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_fetch_metadata_empty_base() {
        let scraper = TmdbScraper::new(
            "invalid_api_key_test_123".to_string(),
            None,
            Some("   ".to_string()),
        );
        let res = scraper.fetch_movie_metadata("Inception", None).await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("401")
                || err_msg.contains("Unauthorized")
                || err_msg.contains("API key")
                || err_msg.contains("status code")
                || err_msg.contains("TMDB request failed"),
            "Error message was not clear: {}",
            err_msg
        );
    }
}
