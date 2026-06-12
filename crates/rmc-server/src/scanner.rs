use crate::db::Database;
use rmc_core::models::Movie;
use std::path::Path;
use std::sync::OnceLock;
use walkdir::WalkDir;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "mpg", "mpeg", "strm",
];
const TITLE_STOP_TOKENS: &[&str] = &[
    "2160p",
    "1080p",
    "1080i",
    "720p",
    "576p",
    "480p",
    "bluray",
    "blu-ray",
    "bdrip",
    "brrip",
    "bdr",
    "bdremux",
    "remux",
    "webdl",
    "webrip",
    "web",
    "hdrip",
    "dvdrip",
    "dvd",
    "hdtv",
    "uhd",
    "hdr",
    "hdr10",
    "hdr10plus",
    "dv",
    "dolbyvision",
    "x264",
    "x265",
    "h264",
    "h265",
    "hevc",
    "av1",
    "aac",
    "dts",
    "truehd",
    "atmos",
    "ac3",
    "ddp5",
    "ddp51",
    "10bit",
    "8bit",
    "proper",
    "repack",
    "extended",
    "unrated",
    "limited",
    "dual",
    "audio",
    "dubbed",
    "subbed",
    "multi",
    "imax",
    "criterion",
    "yify",
    "yts",
];
const LEADING_RELEASE_PREFIXES: &[&str] = &[
    "ANi",
    "Kirara Fantasia",
    "SumiSora&MAGI ATELIER&CASO",
    "DMG&MH&LoliHouse",
    "Nekomoe kissaten",
    "沸班亚马制作组",
    "天月动漫&发布组",
    "VCB-Studio",
    "LoliHouse",
    "Kamigami",
];

#[derive(Clone)]
pub struct MediaScanner {
    db: Database,
}

impl MediaScanner {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    pub async fn scan_directory(
        &self,
        dir: &str,
    ) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
        let dir_owned = dir.to_string();

        // 在独立线程池中执行同步阻塞的文件目录树遍历
        let matching_files = tokio::task::spawn_blocking(move || {
            let mut list = Vec::new();
            let path = Path::new(&dir_owned);
            if !path.exists() {
                return list;
            }

            for entry in WalkDir::new(path)
                .follow_links(true)
                .into_iter()
                .filter_map(|e| match e {
                    Ok(e) => Some(e),
                    Err(err) => {
                        tracing::warn!("Failed to read directory entry during scan: {:?}", err);
                        None
                    }
                })
            {
                let file_path = entry.path();
                if !file_path.is_file() {
                    continue;
                }

                let ext = file_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .unwrap_or_default();

                if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
                    list.push(file_path.to_path_buf());
                }
            }
            list
        })
        .await?;

        let mut count = 0u32;
        for file_path in matching_files {
            let path_str = file_path.to_string_lossy().into_owned();
            let (title, year) = derive_title_year_from_path(&file_path);

            if self.db.movie_exists_by_path(&path_str).await? {
                self.db
                    .update_movie_title_year_by_path(&path_str, &title, year)
                    .await?;
                continue;
            }
            let file_size = std::fs::metadata(&file_path).map(|m| m.len()).ok();

            let movie = Movie {
                id: 0,
                title,
                year,
                file_path: file_path.clone(),
                poster_url: None,
                overview: None,
                tmdb_id: None,
                runtime_minutes: None,
                runtime_seconds: None,
                added_at: chrono::Utc::now().timestamp(),
                file_size,
            };

            self.db.insert_movie(&movie).await?;
            count += 1;
        }

        Ok(count)
    }
}

pub fn parse_filename(stem: &str) -> (String, Option<u16>) {
    let (stem, stripped_release_prefix) = strip_leading_release_groups(stem);
    let bracket_episode_index = bracket_episode_marker(stem).map(|(index, _)| index);
    let release_suffix_index =
        release_group_suffix_marker(stem, stripped_release_prefix).map(|(index, _)| index);
    let title_source = bracket_episode_index
        .into_iter()
        .chain(release_suffix_index)
        .min()
        .map(|index| &stem[..index])
        .unwrap_or(stem)
        .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, '.' | '_' | '-'));
    let tokens = split_title_tokens(title_source);
    if tokens.is_empty() {
        return (title_source.trim().to_string(), None);
    }

    let has_explicit_tail_marker =
        bracket_episode_index.is_some() || release_suffix_index.is_some();
    let year_index = if has_explicit_tail_marker {
        None
    } else {
        tokens
            .iter()
            .enumerate()
            .find_map(|(index, token)| {
                parse_year_token(token)
                    .filter(|_| index > 0)
                    .map(|year| (index, year))
            })
            .or_else(|| {
                (tokens.len() == 1)
                    .then(|| parse_year_token(tokens[0]).map(|year| (0, year)))
                    .flatten()
            })
    };

    let release_index = tokens.iter().position(|token| is_release_tag(token));
    let episode_index = episode_marker_start(&tokens);

    let title_end = year_index
        .map(|(index, _)| index)
        .into_iter()
        .chain(release_index)
        .chain(episode_index)
        .min()
        .unwrap_or(tokens.len());

    let title_tokens = tokens[..title_end].to_vec();

    let title = if title_tokens.is_empty() {
        tokens[0].trim().to_string()
    } else {
        title_tokens.join(" ")
    };

    let year = year_index
        .filter(|(index, _)| *index <= title_end)
        .map(|(_, year)| year);

    (title, year)
}

pub(crate) fn derive_title_year_from_path(path: &Path) -> (String, Option<u16>) {
    let file_stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("Unknown");
    let sanitized_stem = strip_release_bracket_suffixes(file_stem);
    let (title, year) = parse_filename(&sanitized_stem);
    let parent_title = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .map(parse_filename)
        .filter(|(parent_title, _)| !parent_title.trim().is_empty() && !title_looks_hash_like(parent_title));

    match parent_title {
        Some((parent_title, parent_year))
            if file_stem_needs_parent_title(file_stem)
                || should_prefer_parent_title(&title, year, &parent_title, parent_year) =>
        {
            (parent_title, parent_year.or(year))
        }
        _ => (title, year),
    }
}

fn file_stem_needs_parent_title(file_stem: &str) -> bool {
    title_looks_hash_like(file_stem) || plain_numeric_stem(file_stem).is_some()
}

fn should_prefer_parent_title(
    file_title: &str,
    file_year: Option<u16>,
    parent_title: &str,
    parent_year: Option<u16>,
) -> bool {
    if parent_title.trim().is_empty() || title_looks_hash_like(parent_title) {
        return false;
    }

    let normalized_file_title = normalize_title_for_comparison(file_title);
    let normalized_parent_title = normalize_title_for_comparison(parent_title);

    if normalized_file_title.is_empty() || normalized_parent_title.is_empty() {
        return false;
    }

    if normalized_file_title == normalized_parent_title {
        return file_year.is_none() && parent_year.is_some();
    }

    parent_year
        .and_then(|year| strip_trailing_year_token(file_title, year))
        .map(|stripped| normalize_title_for_comparison(stripped) == normalized_parent_title)
        .unwrap_or(false)
}

fn title_looks_hash_like(title: &str) -> bool {
    let normalized: String = title
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();

    let len = normalized.len();
    len >= 8 && normalized.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn plain_numeric_stem(stem: &str) -> Option<u16> {
    let trimmed = stem.trim();
    if trimmed.is_empty() || trimmed.len() > 3 || !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    trimmed.parse::<u16>().ok().filter(|value| *value > 0)
}

pub(crate) fn strip_release_bracket_suffixes(stem: &str) -> String {
    let mut current = stem.trim().to_string();
    loop {
        let Some(open_index) = current.rfind('[') else {
            return current;
        };
        let Some(close_index) = current[open_index..].find(']') else {
            return current;
        };
        if open_index + close_index != current.len() - 1 {
            return current;
        }
        let content = &current[open_index + 1..current.len() - 1];
        let is_release_suffix = content
            .split(|ch: char| ch.is_whitespace() || matches!(ch, '_' | '.' | '-'))
            .filter(|token| !token.is_empty())
            .any(is_release_tag);
        if !is_release_suffix {
            return current;
        }
        current = current[..open_index].trim_end().to_string();
    }
}

fn strip_leading_release_groups(stem: &str) -> (&str, bool) {
    let has_bracketed_release_tag = contains_bracketed_release_tag(stem);
    if episode_marker_start(&split_title_tokens(stem)).is_none()
        && bracket_episode_marker(stem).is_none()
        && release_group_suffix_marker(stem, has_known_release_prefix(stem)).is_none()
        && !has_bracketed_release_tag
        && strip_known_release_prefix(stem).is_none()
    {
        return (stem.trim(), false);
    }

    let mut remaining = stem.trim();
    let mut stripped_bracket_group = false;
    while let Some(rest) = remaining.strip_prefix('[') {
        let Some(closing_index) = rest.find(']') else {
            break;
        };
        let content = &rest[..closing_index];
        if !leading_bracket_group_looks_like_release_metadata(content) {
            break;
        }
        let next = rest[closing_index + 1..].trim_start();
        if next.is_empty() {
            break;
        }
        remaining = next;
        stripped_bracket_group = true;
    }

    if let Some(rest) = strip_known_release_prefix(remaining) {
        if strip_known_release_prefix(stem).is_some()
            || episode_marker_start(&split_title_tokens(rest)).is_some()
            || bracket_episode_marker(rest).is_some()
            || release_group_suffix_marker(rest, true).is_some()
            || has_bracketed_release_tag
        {
            return (rest, true);
        }
    }

    (remaining, stripped_bracket_group)
}

fn leading_bracket_group_looks_like_release_metadata(content: &str) -> bool {
    let trimmed = content.trim();
    !trimmed.is_empty()
        && (is_known_release_group_name(trimmed)
            || split_title_tokens(trimmed)
                .iter()
                .any(|token| is_release_tag(token)))
}

pub(crate) fn bracket_episode_marker(stem: &str) -> Option<(usize, u16)> {
    static BRACKET_EPISODE_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = BRACKET_EPISODE_RE.get_or_init(|| {
        regex::Regex::new(
            r"(?:^|[\] ._-])(?P<marker>\[(?P<episode>\d{1,3})(?:v\d+)?\])(?:$|\[|[ ._-])",
        )
            .unwrap()
    });
    let captures = re.captures(stem)?;
    let marker = captures.name("marker")?;
    let episode = captures.name("episode")?.as_str().parse::<u16>().ok()?;
    Some((marker.start(), episode))
}

pub(crate) fn loose_numbered_episode_suffix(stem: &str) -> Option<(usize, u16)> {
    static DASHED_EPISODE_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = DASHED_EPISODE_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)(?P<marker>[ ._-]+-\s*(?P<episode>\d{1,3})\s*)$").unwrap()
    });
    let captures = re.captures(stem)?;
    let marker = captures.name("marker")?;
    let episode = captures.name("episode")?.as_str().parse::<u16>().ok()?;
    Some((marker.start(), episode))
}

fn release_group_suffix_marker(
    stem: &str,
    allow_plain_number: bool,
) -> Option<(usize, Option<u16>)> {
    if let Some((index, episode)) = loose_numbered_episode_suffix(stem) {
        return Some((index, Some(episode)));
    }

    if let Some(index) = special_release_suffix_start(stem) {
        return Some((index, None));
    }

    if allow_plain_number {
        if let Some((index, episode)) = plain_numbered_episode_suffix(stem) {
            return Some((index, Some(episode)));
        }

        if let Some(index) = trailing_dash_suffix_start(stem) {
            return Some((index, None));
        }
    }

    None
}

fn plain_numbered_episode_suffix(stem: &str) -> Option<(usize, u16)> {
    static PLAIN_EPISODE_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = PLAIN_EPISODE_RE
        .get_or_init(|| regex::Regex::new(r"(?P<marker>[ ._-]+(?P<episode>\d{1,3})\s*)$").unwrap());
    let captures = re.captures(stem)?;
    let marker = captures.name("marker")?;
    let episode = captures.name("episode")?.as_str().parse::<u16>().ok()?;
    Some((marker.start(), episode))
}

fn special_release_suffix_start(stem: &str) -> Option<usize> {
    static SPECIAL_SUFFIX_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = SPECIAL_SUFFIX_RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(?P<marker>[ ._-]+-\s*(?:(?:cm|ed|op|pv|menu|tvcm)\s*\d{0,3}|nc(?:ed|op)|sunny[ ._-]+day)\s*)$",
        )
        .unwrap()
    });
    re.captures(stem)
        .and_then(|captures| captures.name("marker").map(|marker| marker.start()))
}

fn trailing_dash_suffix_start(stem: &str) -> Option<usize> {
    static TRAILING_DASH_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re =
        TRAILING_DASH_RE.get_or_init(|| regex::Regex::new(r"(?P<marker>[ ._-]+-\s*)$").unwrap());
    re.captures(stem)
        .and_then(|captures| captures.name("marker").map(|marker| marker.start()))
}

fn has_known_release_prefix(stem: &str) -> bool {
    strip_known_release_prefix(stem).is_some()
}

fn is_known_release_group_name(value: &str) -> bool {
    let normalized_value = normalize_token(value);
    LEADING_RELEASE_PREFIXES
        .iter()
        .any(|prefix| normalize_token(prefix) == normalized_value)
}

fn strip_known_release_prefix(stem: &str) -> Option<&str> {
    let trimmed = stem.trim();
    LEADING_RELEASE_PREFIXES.iter().find_map(|prefix| {
        let rest = trimmed.get(prefix.len()..)?;
        if !trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return None;
        }
        rest.chars()
            .next()
            .filter(|ch| ch.is_whitespace() || matches!(ch, '-' | '_' | '.'))
            .map(|_| {
                rest.trim_start_matches(|ch: char| {
                    ch.is_whitespace() || matches!(ch, '-' | '_' | '.')
                })
            })
            .filter(|rest| !rest.is_empty())
    })
}

fn contains_bracketed_release_tag(stem: &str) -> bool {
    static BRACKET_CONTENT_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re =
        BRACKET_CONTENT_RE.get_or_init(|| regex::Regex::new(r"\[(?P<content>[^\]]+)\]").unwrap());

    re.captures_iter(stem).any(|captures| {
        captures
            .name("content")
            .map(|content| {
                split_title_tokens(content.as_str())
                    .iter()
                    .any(|token| is_release_tag(token))
            })
            .unwrap_or(false)
    })
}

fn split_title_tokens(stem: &str) -> Vec<&str> {
    static SPLIT_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = SPLIT_RE.get_or_init(|| regex::Regex::new(r"[\.\s_\[\]\(\)]+|+").unwrap());
    re.split(stem)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect()
}

fn parse_year_token(token: &str) -> Option<u16> {
    let normalized = normalize_token(token);
    if normalized.len() == 4 && (normalized.starts_with("19") || normalized.starts_with("20")) {
        normalized.parse::<u16>().ok()
    } else {
        None
    }
}

fn normalize_token(token: &str) -> String {
    token
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn normalize_title_for_comparison(title: &str) -> String {
    split_title_tokens(title)
        .into_iter()
        .map(normalize_token)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_trailing_year_token<'a>(title: &'a str, year: u16) -> Option<&'a str> {
    let year_string = year.to_string();
    let trimmed = title.trim_end();
    let suffix = trimmed.strip_suffix(&year_string)?;
    let suffix = suffix.trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, '.' | '_' | '-'));
    (!suffix.is_empty()).then_some(suffix)
}

fn episode_marker_start(tokens: &[&str]) -> Option<usize> {
    static COMBINED_EPISODE_RE: OnceLock<regex::Regex> = OnceLock::new();
    static EPISODE_ONLY_RE: OnceLock<regex::Regex> = OnceLock::new();

    let combined_episode_re = COMBINED_EPISODE_RE
        .get_or_init(|| regex::Regex::new(r"^s\d{1,2}e\d{1,3}(?:e\d{1,3})*$").unwrap());
    let episode_only_re =
        EPISODE_ONLY_RE.get_or_init(|| regex::Regex::new(r"^(?:ep|e)\d{1,3}$").unwrap());

    for (index, token) in tokens.iter().enumerate() {
        if index == 0 {
            continue;
        }

        let normalized = normalize_token(token);
        if normalized.is_empty() {
            continue;
        }

        if combined_episode_re.is_match(&normalized) || episode_only_re.is_match(&normalized) {
            return Some(index);
        }

        if normalized == "season" {
            if let Some(next) = tokens.get(index + 1).map(|token| normalize_token(token)) {
                if next.chars().all(|ch| ch.is_ascii_digit()) {
                    return Some(index);
                }
            }
        }

        if normalized == "s" {
            if let Some(next) = tokens.get(index + 1).map(|token| normalize_token(token)) {
                if next.chars().all(|ch| ch.is_ascii_digit()) {
                    return Some(index);
                }
            }
        }
    }

    None
}

pub(crate) fn is_release_tag(token: &str) -> bool {
    static RESOLUTION_RE: OnceLock<regex::Regex> = OnceLock::new();
    let normalized = normalize_token(token);
    if normalized.is_empty() {
        return false;
    }

    if TITLE_STOP_TOKENS.contains(&normalized.as_str()) {
        return true;
    }

    let resolution_re = RESOLUTION_RE.get_or_init(|| {
        regex::Regex::new(
            r"^(?:360|480|576|720|1080|1440|2160|4320)p$|^\d{3,4}x\d{3,4}$|^(?:5|7)1$",
        )
        .unwrap()
    });
    resolution_re.is_match(&normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::fs;

    #[tokio::test]
    async fn test_media_scanner_local_and_strm() {
        // 使用 tempfile::tempdir() 自动管理临时文件夹
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // 创建模拟 mp4 文件
        let mp4_file = temp_path.join("Inception.2010.Bluray.mp4");
        fs::write(&mp4_file, "mock video content").unwrap();

        // 创建模拟 strm 文件
        let strm_file = temp_path.join("The.Matrix.2003.strm");
        fs::write(&strm_file, "https://example.com/matrix.mkv").unwrap();

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        let scanner = MediaScanner::new(db.clone());
        let count = scanner
            .scan_directory(&temp_path.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(count, 2);

        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), 2);

        // 年份和标题提取检验
        let inception = movies.iter().find(|m| m.title == "Inception").unwrap();
        assert_eq!(inception.year, Some(2010));
        assert_eq!(inception.file_path, mp4_file);

        let matrix = movies.iter().find(|m| m.title == "The Matrix").unwrap();
        assert_eq!(matrix.year, Some(2003));
        assert_eq!(matrix.file_path, strm_file);

        // temp_dir 会在离开作用域时被自动清理
    }

    #[tokio::test]
    async fn test_media_scanner_updates_existing_dirty_titles() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        let video_file =
            temp_path.join("SumiSora&MAGI ATELIER&CASO Fate Zero [x264 1280x720 AAC].mkv");
        fs::write(&video_file, "mock video content").unwrap();

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 0,
            title: "SumiSora&MAGI ATELIER&CASO Fate Zero".to_string(),
            year: None,
            file_path: video_file.clone(),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        })
        .await
        .unwrap();

        let scanner = MediaScanner::new(db.clone());
        let count = scanner
            .scan_directory(&temp_path.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(count, 0);

        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].title, "Fate Zero");
    }

    #[tokio::test]
    async fn test_media_scanner_preserves_existing_year_when_filename_has_none() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        let video_file = temp_path.join("VCB-Studio Fate Stay Night.mkv");
        fs::write(&video_file, "mock video content").unwrap();

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        db.insert_movie(&Movie {
            id: 0,
            title: "VCB-Studio Fate Stay Night".to_string(),
            year: Some(2006),
            file_path: video_file.clone(),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        })
        .await
        .unwrap();

        let scanner = MediaScanner::new(db.clone());
        let count = scanner
            .scan_directory(&temp_path.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(count, 0);

        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].title, "Fate Stay Night");
        assert_eq!(movies[0].year, Some(2006));
    }

    #[tokio::test]
    async fn test_media_scanner_inserts_clean_titles_for_release_prefixed_episode_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        let samples = [
            (
                "[SumiSora&MAGI_ATELIER&CASO][Fate_Zero][BDRip][12][x264_flac](131BE92D).mkv",
                "Fate Zero",
            ),
            (
                "SumiSora&MAGI ATELIER&CASO Fate Zero.mkv",
                "Fate Zero",
            ),
            (
                "Kamigami Fate stay night UBW - PV02.mkv",
                "Fate stay night UBW",
            ),
            ("LoliHouse Clevatess - 06.mkv", "Clevatess"),
            (
                "DMG&MH&LoliHouse Goblin Slayer - 03.mkv",
                "Goblin Slayer",
            ),
            (
                "Kamigami Hunter X Hunter - 100.mkv",
                "Hunter X Hunter",
            ),
        ];

        for (filename, _) in &samples {
            fs::write(temp_path.join(filename), "mock video content").unwrap();
        }

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        let scanner = MediaScanner::new(db.clone());
        let count = scanner
            .scan_directory(&temp_path.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(count, samples.len() as u32);

        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), samples.len());

        for (filename, expected_title) in &samples {
            let file_path = temp_path.join(filename);
            let movie = movies
                .iter()
                .find(|movie| movie.file_path == file_path)
                .unwrap_or_else(|| panic!("missing movie row for {}", filename));
            assert_eq!(movie.title, *expected_title, "unexpected title for {}", filename);
        }
    }

    #[tokio::test]
    async fn test_media_scanner_rescan_updates_dirty_titles_for_release_prefixed_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        let samples = [
            (
                "[SumiSora&MAGI_ATELIER&CASO][Fate_Zero][BDRip][12][x264_flac](131BE92D).mkv",
                "131BE92D",
                "Fate Zero",
            ),
            (
                "SumiSora&MAGI ATELIER&CASO Fate Zero.mkv",
                "SumiSora&MAGI ATELIER&CASO Fate Zero",
                "Fate Zero",
            ),
            (
                "Kamigami Fate stay night UBW - PV02.mkv",
                "Kamigami Fate stay night UBW - PV02",
                "Fate stay night UBW",
            ),
            (
                "LoliHouse Clevatess - 06.mkv",
                "LoliHouse Clevatess - 06",
                "Clevatess",
            ),
            (
                "DMG&MH&LoliHouse Goblin Slayer - 03.mkv",
                "DMG&MH&LoliHouse Goblin Slayer - 03",
                "Goblin Slayer",
            ),
            (
                "Kamigami Hunter X Hunter - 100.mkv",
                "Kamigami Hunter X Hunter - 100",
                "Hunter X Hunter",
            ),
        ];

        for (filename, _, _) in &samples {
            fs::write(temp_path.join(filename), "mock video content").unwrap();
        }

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        for (filename, dirty_title, _) in &samples {
            db.insert_movie(&Movie {
                id: 0,
                title: (*dirty_title).to_string(),
                year: None,
                file_path: temp_path.join(filename),
                poster_url: None,
                overview: None,
                tmdb_id: None,
                runtime_minutes: None,
                runtime_seconds: None,
                added_at: 0,
                file_size: None,
            })
            .await
            .unwrap();
        }

        let scanner = MediaScanner::new(db.clone());
        let count = scanner
            .scan_directory(&temp_path.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(count, 0);

        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), samples.len());

        for (filename, _, expected_title) in &samples {
            let file_path = temp_path.join(filename);
            let movie = movies
                .iter()
                .find(|movie| movie.file_path == file_path)
                .unwrap_or_else(|| panic!("missing movie row for {}", filename));
            assert_eq!(movie.title, *expected_title, "unexpected title for {}", filename);
        }
    }

    #[tokio::test]
    async fn test_media_scanner_uses_parent_directory_when_file_stem_is_hash_like() {
        let temp_dir = tempfile::tempdir().unwrap();
        let series_dir = temp_dir.path().join("Fate Zero");
        fs::create_dir_all(&series_dir).unwrap();

        let video_file = series_dir.join("21162F95.mkv");
        fs::write(&video_file, "mock video content").unwrap();

        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();

        let scanner = MediaScanner::new(db.clone());
        let count = scanner
            .scan_directory(&temp_dir.path().to_string_lossy())
            .await
            .unwrap();
        assert_eq!(count, 1);

        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].title, "Fate Zero");
    }

    #[test]
    fn test_parse_filename_edge_cases() {
        assert_eq!(
            parse_filename("Simple Movie"),
            ("Simple Movie".to_string(), None)
        );
        assert_eq!(
            parse_filename("Movie.Name.2021.1080p"),
            ("Movie Name".to_string(), Some(2021))
        );
        assert_eq!(parse_filename("NoYear"), ("NoYear".to_string(), None));
        assert_eq!(parse_filename("2020"), ("2020".to_string(), Some(2020)));
        assert_eq!(
            parse_filename("Movie.Name.1080p.BluRay.x265-GROUP"),
            ("Movie Name".to_string(), None)
        );
        assert_eq!(
            parse_filename("1917.2019.1080p.BluRay.x265"),
            ("1917".to_string(), Some(2019))
        );
        assert_eq!(
            parse_filename("Spider-Man.No.Way.Home.2021.WEB-DL.2160p"),
            ("Spider-Man No Way Home".to_string(), Some(2021))
        );
        assert_eq!(
            parse_filename("Tiny.World.S01E04.HDR.2160p.WEB.h265-KOGi"),
            ("Tiny World".to_string(), None)
        );
        assert_eq!(
            parse_filename("Tiny World Season 1 Episode 4 2160p WEB h265-KOGi"),
            ("Tiny World".to_string(), None)
        );
        assert_eq!(
            parse_filename("Tiny.World.EP04.1080p.WEB-DL"),
            ("Tiny World".to_string(), None)
        );
        assert_eq!(
            parse_filename("[Kirara Fantasia] 强者的新传说 S01E01"),
            ("强者的新传说".to_string(), None)
        );
        assert_eq!(
            parse_filename("[VCB-Studio] Fate Stay Night 2006 [01][Ma10p_1080p][x265_flac]"),
            ("Fate Stay Night 2006".to_string(), None)
        );
        assert_eq!(
            parse_filename("[Nekomoe kissaten] Goblin Slayer [01][1080p][x265_flac]"),
            ("Goblin Slayer".to_string(), None)
        );
        assert_eq!(
            parse_filename("DMG&MH&LoliHouse Goblin Slayer - 03"),
            ("Goblin Slayer".to_string(), None)
        );
        assert_eq!(
            parse_filename("Kamigami Fate stay night UBW - 00"),
            ("Fate stay night UBW".to_string(), None)
        );
        assert_eq!(
            parse_filename("Kamigami Fate stay night UBW - ED01"),
            ("Fate stay night UBW".to_string(), None)
        );
        assert_eq!(
            parse_filename("Kamigami Fate stay night UBW - Menu01"),
            ("Fate stay night UBW".to_string(), None)
        );
        assert_eq!(
            parse_filename("Kamigami Fate stay night UBW - PV02"),
            ("Fate stay night UBW".to_string(), None)
        );
        assert_eq!(
            parse_filename("Kamigami Fate stay night UBW - Sunny Day"),
            ("Fate stay night UBW".to_string(), None)
        );
        assert_eq!(
            parse_filename("Kamigami Hunter X Hunter - 100"),
            ("Hunter X Hunter".to_string(), None)
        );
        assert_eq!(
            parse_filename("SumiSora&MAGI ATELIER&CASO Fate Zero"),
            ("Fate Zero".to_string(), None)
        );
        assert_eq!(
            parse_filename("[SumiSora&MAGI_ATELIER&CASO][Fate_Zero][BDRip][12][x264_flac](131BE92D)"),
            ("Fate Zero".to_string(), None)
        );
        assert_eq!(
            parse_filename("[Kamigami] Hunter X Hunter [x264 1280x720 AAC MKV Sub(Chs,Jap)]"),
            ("Hunter X Hunter".to_string(), None)
        );
        assert_eq!(
            parse_filename("LoliHouse Clevatess - 10"),
            ("Clevatess".to_string(), None)
        );
        assert_eq!(
            parse_filename("Nekomoe kissaten Goblin Slayer 12"),
            ("Goblin Slayer".to_string(), None)
        );
        assert_eq!(
            parse_filename("[天月动漫&发布组] 差点在迷宫深处被信任的伙伴杀掉，但靠着天赐技能「无限扭蛋」获得等级9999的伙伴，我要向前队友和世界展开复仇&「给他们好看！」 S01E02"),
            (
                "差点在迷宫深处被信任的伙伴杀掉，但靠着天赐技能「无限扭蛋」获得等级9999的伙伴，我要向前队友和世界展开复仇&「给他们好看！」".to_string(),
                None
            )
        );
    }

    #[test]
    fn test_derive_title_year_from_path_prefers_parent_directory_when_filename_only_adds_year() {
        let path = Path::new(
            "/vol2/1000/media/TV/Anime/Fate Stay Night(2006)/[VCB-Studio] Fate Stay Night 2006 [01][Ma10p_1080p][x265_flac].mkv",
        );

        assert_eq!(
            derive_title_year_from_path(path),
            ("Fate Stay Night".to_string(), Some(2006))
        );
    }
}
