use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use rmc_core::api_types::{LibraryItem, LibraryItemKind, LibraryItemsResponse, LibrarySummary};
use rmc_core::models::Movie;

pub const ROOT_COLLECTION_ID: &str = "library-root";

#[derive(Debug, Clone)]
pub struct MediaTree {
    items_by_id: HashMap<String, LibraryItem>,
    root_items: Vec<LibraryItem>,
    children: HashMap<String, Vec<LibraryItem>>,
    summaries: Vec<LibrarySummary>,
}

#[derive(Debug, Clone)]
struct EpisodeInfo {
    season_number: u16,
    episode_number: u16,
}

#[derive(Debug, Clone)]
struct EpisodeEntry {
    movie: Movie,
    info: EpisodeInfo,
}

#[derive(Debug, Default)]
struct SeriesGroup {
    title: String,
    year: Option<u16>,
    poster_url: Option<String>,
    overview: Option<String>,
    added_at: i64,
    seasons: BTreeMap<u16, Vec<EpisodeEntry>>,
}

impl MediaTree {
    pub fn from_movies(movies: Vec<Movie>) -> Self {
        let mut items_by_id = HashMap::new();
        let mut root_items = Vec::new();
        let mut children = HashMap::new();
        let mut series_groups: BTreeMap<String, SeriesGroup> = BTreeMap::new();
        let mut movie_count = 0usize;

        for movie in movies {
            if let Some(info) = detect_episode_info(&movie) {
                let (parsed_title, parsed_year) = movie
                    .file_path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(crate::scanner::parse_filename)
                    .unwrap_or_else(|| (movie.title.clone(), movie.year));
                let series_title = if parsed_title.trim().is_empty() {
                    movie.title.clone()
                } else {
                    parsed_title
                };
                let series_year = parsed_year.or(movie.year);
                let key = series_key(&series_title, series_year);
                let entry = series_groups.entry(key).or_default();
                if entry.title.is_empty() {
                    entry.title = series_title;
                }
                if entry.year.is_none() {
                    entry.year = series_year;
                }
                if entry.poster_url.is_none() {
                    entry.poster_url = movie.poster_url.clone();
                }
                if entry.overview.is_none() {
                    entry.overview = movie.overview.clone();
                }
                entry.added_at = entry.added_at.max(movie.added_at);
                entry
                    .seasons
                    .entry(info.season_number)
                    .or_default()
                    .push(EpisodeEntry { movie, info });
            } else {
                movie_count += 1;
                let item = build_movie_item(&movie);
                items_by_id.insert(item.id.clone(), item.clone());
                root_items.push(item);
            }
        }

        let series_count = series_groups.len();

        for (group_key, group) in series_groups {
            let series_id = format!("series:{}", group_key);
            let mut season_items = Vec::new();
            let mut total_episodes = 0usize;

            for (season_number, mut episodes) in group.seasons {
                episodes.sort_by(|left, right| {
                    left.info
                        .episode_number
                        .cmp(&right.info.episode_number)
                        .then_with(|| left.movie.added_at.cmp(&right.movie.added_at))
                });

                total_episodes += episodes.len();
                let season_id = format!("{}:season:{}", series_id, season_number);
                let mut episode_items = Vec::with_capacity(episodes.len());
                let mut season_added_at = 0i64;
                let mut season_poster_url = None;
                let mut season_overview = None;

                for episode in episodes {
                    season_added_at = season_added_at.max(episode.movie.added_at);
                    if season_poster_url.is_none() {
                        season_poster_url = episode.movie.poster_url.clone();
                    }
                    if season_overview.is_none() {
                        season_overview = episode.movie.overview.clone();
                    }

                    let item = build_episode_item(&episode.movie, &season_id, &episode.info);
                    items_by_id.insert(item.id.clone(), item.clone());
                    episode_items.push(item);
                }

                let season_item = LibraryItem {
                    id: season_id.clone(),
                    kind: LibraryItemKind::Season,
                    parent_id: Some(series_id.clone()),
                    title: format!("Season {}", season_number),
                    year: group.year,
                    poster_url: season_poster_url.or_else(|| group.poster_url.clone()),
                    overview: season_overview.or_else(|| group.overview.clone()),
                    child_count: episode_items.len(),
                    play_id: None,
                    season_number: Some(season_number),
                    episode_number: None,
                    runtime_minutes: None,
                    runtime_seconds: None,
                    added_at: season_added_at,
                    file_size: None,
                };

                items_by_id.insert(season_id.clone(), season_item.clone());
                children.insert(season_id.clone(), episode_items);
                season_items.push(season_item);
            }

            season_items.sort_by(|left, right| {
                left.season_number
                    .unwrap_or_default()
                    .cmp(&right.season_number.unwrap_or_default())
            });

            let series_item = LibraryItem {
                id: series_id.clone(),
                kind: LibraryItemKind::Series,
                parent_id: None,
                title: group.title,
                year: group.year,
                poster_url: group.poster_url,
                overview: group.overview,
                child_count: season_items.len(),
                play_id: None,
                season_number: None,
                episode_number: None,
                runtime_minutes: None,
                runtime_seconds: None,
                added_at: group.added_at,
                file_size: None,
            };

            items_by_id.insert(series_id.clone(), series_item.clone());
            children.insert(series_id, season_items);
            root_items.push(series_item);
            let _ = total_episodes;
        }

        sort_root_items(&mut root_items);

        let summaries = vec![
            LibrarySummary {
                id: "movies".to_string(),
                name: "Movies".to_string(),
                count: movie_count,
            },
            LibrarySummary {
                id: "series".to_string(),
                name: "Series".to_string(),
                count: series_count,
            },
        ];

        Self {
            items_by_id,
            root_items,
            children,
            summaries,
        }
    }

    pub fn summaries(&self) -> Vec<LibrarySummary> {
        self.summaries.clone()
    }

    pub fn item(&self, id: &str) -> Option<LibraryItem> {
        self.items_by_id.get(id).cloned()
    }

    pub fn find_by_play_id(&self, play_id: i64) -> Option<LibraryItem> {
        self.items_by_id
            .values()
            .find(|item| item.play_id == Some(play_id))
            .cloned()
    }

    pub fn paged_items(
        &self,
        parent_id: Option<&str>,
        search_term: Option<&str>,
        start_index: usize,
        limit: Option<usize>,
    ) -> LibraryItemsResponse {
        let mut items = match parent_id.filter(|value| !value.trim().is_empty()) {
            Some(parent_id) => self.children.get(parent_id).cloned().unwrap_or_default(),
            None => self.root_items.clone(),
        };

        if let Some(search_term) = search_term.filter(|value| !value.trim().is_empty()) {
            let normalized = search_term.trim().to_ascii_lowercase();
            items.retain(|item| {
                item.title.to_ascii_lowercase().contains(&normalized)
                    || item
                        .overview
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(&normalized)
            });
        }

        let total = items.len();
        let start_index = start_index.min(total);
        let limit = limit.unwrap_or(total.saturating_sub(start_index));
        let items = items.into_iter().skip(start_index).take(limit).collect();

        LibraryItemsResponse {
            items,
            total_record_count: total,
            start_index,
        }
    }

    pub fn playable_items_sorted_by_added_at(&self) -> Vec<LibraryItem> {
        let mut items = self
            .items_by_id
            .values()
            .filter(|item| item.play_id.is_some())
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .added_at
                .cmp(&left.added_at)
                .then_with(|| left.title.cmp(&right.title))
        });
        items
    }
}

fn build_movie_item(movie: &Movie) -> LibraryItem {
    LibraryItem {
        id: movie.id.to_string(),
        kind: LibraryItemKind::Movie,
        parent_id: None,
        title: movie.title.clone(),
        year: movie.year,
        poster_url: movie.poster_url.clone(),
        overview: movie.overview.clone(),
        child_count: 0,
        play_id: Some(movie.id),
        season_number: None,
        episode_number: None,
        runtime_minutes: movie.runtime_minutes,
        runtime_seconds: movie.runtime_seconds,
        added_at: movie.added_at,
        file_size: movie.file_size,
    }
}

fn build_episode_item(movie: &Movie, parent_id: &str, info: &EpisodeInfo) -> LibraryItem {
    LibraryItem {
        id: format!("episode:{}", movie.id),
        kind: LibraryItemKind::Episode,
        parent_id: Some(parent_id.to_string()),
        title: format!("Episode {}", info.episode_number),
        year: movie.year,
        poster_url: movie.poster_url.clone(),
        overview: movie.overview.clone(),
        child_count: 0,
        play_id: Some(movie.id),
        season_number: Some(info.season_number),
        episode_number: Some(info.episode_number),
        runtime_minutes: movie.runtime_minutes,
        runtime_seconds: movie.runtime_seconds,
        added_at: movie.added_at,
        file_size: movie.file_size,
    }
}

fn sort_root_items(items: &mut [LibraryItem]) {
    items.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| kind_rank(&left.kind).cmp(&kind_rank(&right.kind)))
    });
}

fn kind_rank(kind: &LibraryItemKind) -> u8 {
    match kind {
        LibraryItemKind::Series => 0,
        LibraryItemKind::Movie => 1,
        LibraryItemKind::Season => 2,
        LibraryItemKind::Episode => 3,
    }
}

fn series_key(title: &str, year: Option<u16>) -> String {
    match year {
        Some(year) => format!("{}-{}", slugify(title), year),
        None => slugify(title),
    }
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;

    for ch in value.chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

fn detect_episode_info(movie: &Movie) -> Option<EpisodeInfo> {
    let stem = movie.file_path.file_stem()?.to_str()?;

    if let Some(info) = detect_episode_info_in_text(stem) {
        return Some(info);
    }

    if let Some((_, episode_number)) = crate::scanner::bracket_episode_marker(stem) {
        return Some(EpisodeInfo {
            season_number: detect_season_from_ancestors(movie.file_path.parent()).unwrap_or(1),
            episode_number,
        });
    }

    let episode_number = episode_only_regex()
        .captures(stem)
        .and_then(|caps| caps.name("episode"))
        .and_then(|value| value.as_str().parse::<u16>().ok())?;

    let season_number = detect_season_from_ancestors(movie.file_path.parent())?;
    Some(EpisodeInfo {
        season_number,
        episode_number,
    })
}

fn detect_episode_info_in_text(text: &str) -> Option<EpisodeInfo> {
    if let Some(caps) = combined_episode_regex().captures(text) {
        let season_number = caps.name("season")?.as_str().parse::<u16>().ok()?;
        let episode_number = caps.name("episode")?.as_str().parse::<u16>().ok()?;
        return Some(EpisodeInfo {
            season_number,
            episode_number,
        });
    }

    if let Some(caps) = verbose_episode_regex().captures(text) {
        let season_number = caps.name("season")?.as_str().parse::<u16>().ok()?;
        let episode_number = caps.name("episode")?.as_str().parse::<u16>().ok()?;
        return Some(EpisodeInfo {
            season_number,
            episode_number,
        });
    }

    None
}

fn detect_season_from_ancestors(mut path: Option<&Path>) -> Option<u16> {
    while let Some(current) = path {
        let segment = current.file_name()?.to_str()?;
        if let Some(caps) = season_only_regex().captures(segment) {
            let value = caps
                .name("season")
                .or_else(|| caps.name("short"))
                .and_then(|value| value.as_str().parse::<u16>().ok());
            if value.is_some() {
                return value;
            }
        }
        path = current.parent();
    }
    None
}

fn combined_episode_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bs(?P<season>\d{1,2})[ ._-]*e(?P<episode>\d{1,3})\b").unwrap()
    })
}

fn verbose_episode_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\bseason[ ._-]*(?P<season>\d{1,2})\b.*?\b(?:episode|ep)[ ._-]*(?P<episode>\d{1,3})\b",
        )
        .unwrap()
    })
}

fn episode_only_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b(?:episode|ep|e)[ ._-]*(?P<episode>\d{1,3})\b").unwrap())
}

fn season_only_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bseason[ ._-]*(?P<season>\d{1,2})\b|\bs(?P<short>\d{1,2})\b").unwrap()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(id: i64, title: &str, file_path: &str) -> Movie {
        Movie {
            id,
            title: title.to_string(),
            year: Some(2020),
            file_path: file_path.into(),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: id,
            file_size: None,
        }
    }

    #[test]
    fn test_detect_episode_info_patterns() {
        let first_movie = movie(1, "Tiny World", "/media/Tiny World/Tiny.World.S01E04.mkv");
        let info = detect_episode_info(&first_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 4);

        let second_movie = movie(
            2,
            "Tiny World",
            "/media/Tiny World/Season 2/Tiny World EP03.mkv",
        );
        let info = detect_episode_info(&second_movie).unwrap();
        assert_eq!(info.season_number, 2);
        assert_eq!(info.episode_number, 3);

        let bracket_movie = movie(
            3,
            "Fate Stay Night 2006",
            "/media/Fate Stay Night/[VCB-Studio] Fate Stay Night 2006 [01][Ma10p_1080p][x265_flac].mkv",
        );
        let info = detect_episode_info(&bracket_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 1);
    }

    #[test]
    fn test_media_tree_groups_series_and_seasons() {
        let tree = MediaTree::from_movies(vec![
            movie(1, "Tiny World", "/media/Tiny World/Tiny.World.S01E01.mkv"),
            movie(2, "Tiny World", "/media/Tiny World/Tiny.World.S01E02.mkv"),
            movie(3, "Inception", "/media/Inception.2010.mkv"),
        ]);

        let root = tree.paged_items(None, None, 0, None);
        assert_eq!(root.total_record_count, 2);
        assert!(root
            .items
            .iter()
            .any(|item| item.kind == LibraryItemKind::Series));
        assert!(root
            .items
            .iter()
            .any(|item| item.kind == LibraryItemKind::Movie));

        let series = root
            .items
            .into_iter()
            .find(|item| item.kind == LibraryItemKind::Series)
            .unwrap();
        let seasons = tree.paged_items(Some(&series.id), None, 0, None);
        assert_eq!(seasons.total_record_count, 1);
        let episodes = tree.paged_items(Some(&seasons.items[0].id), None, 0, None);
        assert_eq!(episodes.total_record_count, 2);
        assert_eq!(episodes.items[0].play_id, Some(1));
    }

    #[test]
    fn test_media_tree_cleans_anime_release_names_for_library_display() {
        let tree = MediaTree::from_movies(vec![
            movie(
                1,
                "[Kirara Fantasia] 强者的新传说 S01E01",
                "/media/Anime/[Kirara Fantasia] 强者的新传说 S01E01.mp4",
            ),
            movie(
                2,
                "[VCB-Studio] Fate Stay Night 2006 [01][Ma10p_1080p][x265_flac]",
                "/media/Anime/[VCB-Studio] Fate Stay Night 2006 [01][Ma10p_1080p][x265_flac].mkv",
            ),
            movie(
                3,
                "[天月动漫&发布组] 差点在迷宫深处被信任的伙伴杀掉，但靠着天赐技能「无限扭蛋」获得等级9999的伙伴，我要向前队友和世界展开复仇&「给他们好看！」 S01E02",
                "/media/Anime/[天月动漫&发布组] 差点在迷宫深处被信任的伙伴杀掉，但靠着天赐技能「无限扭蛋」获得等级9999的伙伴，我要向前队友和世界展开复仇&「给他们好看！」 S01E02.mkv",
            ),
        ]);

        let root = tree.paged_items(None, None, 0, None);
        let titles = root
            .items
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>();

        assert!(titles.contains(&"强者的新传说"));
        assert!(titles.contains(&"Fate Stay Night 2006"));
        assert!(titles.contains(&"差点在迷宫深处被信任的伙伴杀掉，但靠着天赐技能「无限扭蛋」获得等级9999的伙伴，我要向前队友和世界展开复仇&「给他们好看！」"));
        assert!(root
            .items
            .iter()
            .all(|item| item.kind == LibraryItemKind::Series));
    }
}
