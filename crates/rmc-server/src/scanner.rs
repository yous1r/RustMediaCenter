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
            if self.db.movie_exists_by_path(&path_str).await? {
                continue;
            }

            let file_stem = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown");

            let (title, year) = parse_filename(file_stem);
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
    let stem = strip_leading_release_groups(stem);
    let bracket_episode_index = bracket_episode_marker(stem).map(|(index, _)| index);
    let title_source = bracket_episode_index
        .map(|index| &stem[..index])
        .unwrap_or(stem)
        .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, '.' | '_' | '-'));
    let tokens = split_title_tokens(title_source);
    if tokens.is_empty() {
        return (title_source.trim().to_string(), None);
    }

    let year_index = if bracket_episode_index.is_some() {
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

fn strip_leading_release_groups(stem: &str) -> &str {
    if episode_marker_start(&split_title_tokens(stem)).is_none()
        && bracket_episode_marker(stem).is_none()
    {
        return stem.trim();
    }

    let mut remaining = stem.trim();
    while let Some(rest) = remaining.strip_prefix('[') {
        let Some(closing_index) = rest.find(']') else {
            break;
        };
        let next = rest[closing_index + 1..].trim_start();
        if next.is_empty() {
            break;
        }
        remaining = next;
    }
    remaining
}

pub(crate) fn bracket_episode_marker(stem: &str) -> Option<(usize, u16)> {
    static BRACKET_EPISODE_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = BRACKET_EPISODE_RE.get_or_init(|| {
        regex::Regex::new(r"(?:^|[ ._-])(?P<marker>\[(?P<episode>\d{1,3})\])(?:$|\[|[ ._-])")
            .unwrap()
    });
    let captures = re.captures(stem)?;
    let marker = captures.name("marker")?;
    let episode = captures.name("episode")?.as_str().parse::<u16>().ok()?;
    Some((marker.start(), episode))
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

fn is_release_tag(token: &str) -> bool {
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
            parse_filename("[天月动漫&发布组] 差点在迷宫深处被信任的伙伴杀掉，但靠着天赐技能「无限扭蛋」获得等级9999的伙伴，我要向前队友和世界展开复仇&「给他们好看！」 S01E02"),
            (
                "差点在迷宫深处被信任的伙伴杀掉，但靠着天赐技能「无限扭蛋」获得等级9999的伙伴，我要向前队友和世界展开复仇&「给他们好看！」".to_string(),
                None
            )
        );
    }
}
