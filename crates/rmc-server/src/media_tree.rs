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
    title_override: Option<String>,
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
                let (parsed_title, parsed_year) =
                    crate::scanner::derive_title_year_from_path(&movie.file_path);
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
                log_series_detection_miss(&movie);
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
        title: info
            .title_override
            .clone()
            .unwrap_or_else(|| format!("Episode {}", info.episode_number)),
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
    let sanitized_stem = crate::scanner::strip_release_bracket_suffixes(stem);

    if let Some((episode_number, title_override)) = detect_tv_reproduction(stem) {
        return Some(EpisodeInfo {
            season_number: 0,
            episode_number,
            title_override: Some(title_override),
        });
    }

    if sanitized_stem != stem {
        if let Some((episode_number, title_override)) = detect_tv_reproduction(&sanitized_stem) {
            return Some(EpisodeInfo {
                season_number: 0,
                episode_number,
                title_override: Some(title_override),
            });
        }
    }

    if let Some(info) = detect_contextual_special_info(movie.file_path.parent(), stem) {
        return Some(info);
    }

    if sanitized_stem != stem {
        if let Some(info) =
            detect_contextual_special_info(movie.file_path.parent(), &sanitized_stem)
        {
            return Some(info);
        }
    }

    if let Some(info) = detect_episode_info_in_text(stem) {
        return Some(info);
    }

    if sanitized_stem != stem {
        if let Some(info) = detect_episode_info_in_text(&sanitized_stem) {
            return Some(info);
        }
    }

    if let Some((_, episode_number)) = crate::scanner::bracket_episode_marker(stem) {
        return Some(EpisodeInfo {
            season_number: detect_season_from_ancestors(movie.file_path.parent()).unwrap_or(1),
            episode_number,
            title_override: None,
        });
    }

    if sanitized_stem != stem {
        if let Some((_, episode_number)) = crate::scanner::bracket_episode_marker(&sanitized_stem) {
            return Some(EpisodeInfo {
                season_number: detect_season_from_ancestors(movie.file_path.parent()).unwrap_or(1),
                episode_number,
                title_override: None,
            });
        }
    }

    if let Some((_, episode_number)) = crate::scanner::loose_numbered_episode_suffix(stem) {
        return Some(EpisodeInfo {
            season_number: detect_season_from_ancestors(movie.file_path.parent()).unwrap_or(1),
            episode_number,
            title_override: None,
        });
    }

    if sanitized_stem != stem {
        if let Some((_, episode_number)) =
            crate::scanner::loose_numbered_episode_suffix(&sanitized_stem)
        {
            return Some(EpisodeInfo {
                season_number: detect_season_from_ancestors(movie.file_path.parent()).unwrap_or(1),
                episode_number,
                title_override: None,
            });
        }
    }

    if let Some((episode_number, title_override)) = detect_special_marker(&sanitized_stem) {
        let season_number = detect_season_from_ancestors(movie.file_path.parent())
            .or_else(|| detect_series_context(movie.file_path.parent()).then_some(0))
            .unwrap_or(0);
        return Some(EpisodeInfo {
            season_number,
            episode_number,
            title_override: Some(title_override),
        });
    }

    if let Some((episode_number, title_override)) = detect_named_special(&sanitized_stem) {
        if !detect_series_context(movie.file_path.parent()) {
            return None;
        }
        return Some(EpisodeInfo {
            season_number: 0,
            episode_number,
            title_override: Some(title_override),
        });
    }

    if let Some(episode_number) = plain_numeric_episode_number(stem) {
        let season_number = detect_season_from_ancestors(movie.file_path.parent())
            .or_else(|| detect_series_context(movie.file_path.parent()).then_some(1))?;
        return Some(EpisodeInfo {
            season_number,
            episode_number,
            title_override: None,
        });
    }

    if sanitized_stem != stem {
        if let Some(episode_number) = plain_numeric_episode_number(&sanitized_stem) {
            let season_number = detect_season_from_ancestors(movie.file_path.parent())
                .or_else(|| detect_series_context(movie.file_path.parent()).then_some(1))?;
            return Some(EpisodeInfo {
                season_number,
                episode_number,
                title_override: None,
            });
        }
    }

    let episode_number = episode_only_regex()
        .captures(stem)
        .and_then(|caps| caps.name("episode"))
        .and_then(|value| value.as_str().parse::<u16>().ok())
        .or_else(|| {
            (sanitized_stem != stem)
                .then(|| {
                    episode_only_regex()
                        .captures(&sanitized_stem)
                        .and_then(|caps| caps.name("episode"))
                        .and_then(|value| value.as_str().parse::<u16>().ok())
                })
                .flatten()
        })?;

    let season_number = detect_season_from_ancestors(movie.file_path.parent())
        .or_else(|| detect_series_context(movie.file_path.parent()).then_some(1))?;
    Some(EpisodeInfo {
        season_number,
        episode_number,
        title_override: None,
    })
}

fn detect_episode_info_in_text(text: &str) -> Option<EpisodeInfo> {
    if let Some(caps) = combined_episode_regex().captures(text) {
        let season_number = caps.name("season")?.as_str().parse::<u16>().ok()?;
        let episode_number = caps.name("episode")?.as_str().parse::<u16>().ok()?;
        return Some(EpisodeInfo {
            season_number,
            episode_number,
            title_override: None,
        });
    }

    if let Some(caps) = verbose_episode_regex().captures(text) {
        let season_number = caps.name("season")?.as_str().parse::<u16>().ok()?;
        let episode_number = caps.name("episode")?.as_str().parse::<u16>().ok()?;
        return Some(EpisodeInfo {
            season_number,
            episode_number,
            title_override: None,
        });
    }

    None
}

fn detect_season_from_ancestors(mut path: Option<&Path>) -> Option<u16> {
    while let Some(current) = path {
        let segment = current.file_name()?.to_str()?;
        if special_season_segment(segment) {
            return Some(0);
        }
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

fn detect_contextual_special_info(path: Option<&Path>, stem: &str) -> Option<EpisodeInfo> {
    let (special_context, label) = detect_special_context(path)?;
    let (episode_number, title) = detect_special_file_number_and_title(stem, &label)?;
    Some(EpisodeInfo {
        season_number: if special_context { 0 } else { 1 },
        episode_number,
        title_override: Some(title),
    })
}

fn detect_special_context(path: Option<&Path>) -> Option<(bool, String)> {
    let mut current = path;
    while let Some(segment_path) = current {
        let segment = segment_path.file_name()?.to_str()?;
        if let Some(label) = special_context_label(segment) {
            return Some((true, label));
        }
        current = segment_path.parent();
    }
    None
}

fn special_context_label(segment: &str) -> Option<String> {
    let normalized = normalize_segment(segment);
    let label = match normalized.as_str() {
        "sp" | "sps" | "special" | "specials" | "extras" => "Special",
        "ova" | "ovas" | "oad" | "oads" => "OVA",
        "pv" | "pvs" | "preview" | "previews" => "PV",
        "menu" | "menus" => "Menu",
        "cd" | "cds" => "CD",
        "ncop" => "NCOP",
        "nced" => "NCED",
        _ => return None,
    };
    Some(label.to_string())
}

fn special_season_segment(segment: &str) -> bool {
    special_context_label(segment).is_some()
}

fn detect_special_marker(stem: &str) -> Option<(u16, String)> {
    static BRACKETED_SPECIAL_RE: OnceLock<Regex> = OnceLock::new();
    static SUFFIX_SPECIAL_RE: OnceLock<Regex> = OnceLock::new();

    let bracketed = BRACKETED_SPECIAL_RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:^|[\] ._-])\[(?P<label>ncop|nced|op|ed|pv|cm|menu|tvcm)(?P<episode>\d{0,3})\](?:$|\[|[ ._-])",
        )
        .unwrap()
    });
    if let Some(caps) = bracketed.captures(stem) {
        let label = caps.name("label")?.as_str().to_ascii_uppercase();
        let episode = caps
            .name("episode")
            .and_then(|value| (!value.as_str().is_empty()).then_some(value.as_str()))
            .and_then(|value| value.parse::<u16>().ok());
        let title = episode
            .map(|num| format!("{}{}", label, num))
            .unwrap_or_else(|| label.clone());
        return Some((special_sort_number(&label, episode), title));
    }

    let suffix = SUFFIX_SPECIAL_RE.get_or_init(|| {
        Regex::new(r"(?i)[ ._-]+-\s*(?P<label>cm|ed|op|pv|menu|tvcm)(?P<episode>\d{1,3})\s*$")
            .unwrap()
    });
    let caps = suffix.captures(stem)?;
    let label = caps.name("label")?.as_str().to_ascii_uppercase();
    let episode = caps.name("episode")?.as_str().parse::<u16>().ok()?;
    Some((
        special_sort_number(&label, Some(episode)),
        format!("{}{}", label, episode),
    ))
}

fn detect_named_special(stem: &str) -> Option<(u16, String)> {
    static SUNNY_DAY_RE: OnceLock<Regex> = OnceLock::new();
    let sunny_day =
        SUNNY_DAY_RE.get_or_init(|| Regex::new(r"(?i)[ ._-]+-\s*(sunny[ ._-]+day)\s*$").unwrap());
    if sunny_day.is_match(stem) {
        return Some((
            special_sort_number("SUNNY DAY", None),
            "Sunny Day".to_string(),
        ));
    }
    None
}

fn detect_special_file_number_and_title(stem: &str, context_label: &str) -> Option<(u16, String)> {
    if let Some((episode_number, title)) = detect_named_special(stem) {
        return Some((episode_number, title));
    }

    if let Some((episode_number, title)) = detect_tv_reproduction(stem) {
        return Some((episode_number, title));
    }

    if let Some((episode_number, title)) = detect_special_marker(stem) {
        return Some((episode_number, title));
    }

    if let Some((episode_number, title)) = detect_plain_labeled_special(stem) {
        return Some((episode_number, title));
    }

    let episode_number = crate::scanner::bracket_episode_marker(stem)
        .map(|(_, episode)| episode)
        .or_else(|| crate::scanner::loose_numbered_episode_suffix(stem).map(|(_, episode)| episode))
        .or_else(|| trailing_plain_number(stem))
        .or_else(|| plain_numeric_episode_number(stem))?;

    let title = match context_label {
        "Special" => format!("Special {}", episode_number),
        "OVA" => format!("OVA {}", episode_number),
        "PV" => format!("PV{}", episode_number),
        "Menu" => format!("Menu {}", episode_number),
        "CD" => format!("CD {}", episode_number),
        "NCOP" => format!("NCOP{}", episode_number),
        "NCED" => format!("NCED{}", episode_number),
        _ => format!("{} {}", context_label, episode_number),
    };
    Some((
        special_sort_number(context_label, Some(episode_number)),
        title,
    ))
}

fn detect_tv_reproduction(stem: &str) -> Option<(u16, String)> {
    static TV_REPRODUCTION_RE: OnceLock<Regex> = OnceLock::new();
    let re =
        TV_REPRODUCTION_RE.get_or_init(|| Regex::new(r"(?i)\btv[ ._-]*reproduction\b").unwrap());
    if !re.is_match(stem) {
        return None;
    }

    let episode = crate::scanner::bracket_episode_marker(stem)
        .map(|(_, episode)| episode)
        .or_else(|| trailing_plain_number(stem))
        .unwrap_or(1);
    Some((
        special_sort_number("TVREPRODUCTION", Some(episode)),
        format!("TV Reproduction {}", episode),
    ))
}

fn detect_plain_labeled_special(stem: &str) -> Option<(u16, String)> {
    static PLAIN_LABELED_SPECIAL_RE: OnceLock<Regex> = OnceLock::new();
    let re = PLAIN_LABELED_SPECIAL_RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:^|[\] ._-])(?P<label>cm|ed|op|pv|menu|tvcm|ncop|nced|cd)[ ._-]*(?P<episode>\d{0,3})(?:$|\[|[ ._-])",
        )
        .unwrap()
    });
    let caps = re.captures(stem)?;
    let label = caps.name("label")?.as_str().to_ascii_uppercase();
    let episode = caps
        .name("episode")
        .and_then(|value| (!value.as_str().is_empty()).then_some(value.as_str()))
        .and_then(|value| value.parse::<u16>().ok());
    let title = match label.as_str() {
        "MENU" => episode
            .map(|num| format!("Menu {}", num))
            .unwrap_or_else(|| "Menu".to_string()),
        "CD" => episode
            .map(|num| format!("CD {}", num))
            .unwrap_or_else(|| "CD".to_string()),
        _ => episode
            .map(|num| format!("{}{}", label, num))
            .unwrap_or_else(|| label.clone()),
    };
    Some((special_sort_number(&label, episode), title))
}

fn special_sort_number(label: &str, episode: Option<u16>) -> u16 {
    let normalized = normalize_segment(label);
    let base = match normalized.as_str() {
        "cd" => 50,
        "cm" => 100,
        "tvcm" => 150,
        "pv" => 200,
        "menu" => 250,
        "op" | "ncop" => 300,
        "ed" | "nced" => 400,
        "tvreproduction" => 450,
        "sunnyday" => 500,
        "special" => 600,
        "ova" => 700,
        _ => 600,
    };
    base + episode.unwrap_or(1)
}

fn trailing_plain_number(stem: &str) -> Option<u16> {
    static TRAILING_NUMBER_RE: OnceLock<Regex> = OnceLock::new();
    let re = TRAILING_NUMBER_RE.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[\] ._-])(?P<episode>\d{1,3})(?:v\d+)?\s*$").unwrap()
    });
    re.captures(stem)
        .and_then(|caps| caps.name("episode"))
        .and_then(|value| value.as_str().parse::<u16>().ok())
        .filter(|episode| *episode > 0)
}

fn plain_numeric_episode_number(stem: &str) -> Option<u16> {
    let normalized: String = stem.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if normalized.len() < 1 || normalized.len() > 3 || normalized.len() != stem.trim().len() {
        return None;
    }

    normalized
        .parse::<u16>()
        .ok()
        .filter(|episode| *episode > 0)
}

fn normalize_segment(segment: &str) -> String {
    segment
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn detect_series_context(path: Option<&Path>) -> bool {
    let Some(parent) = path else {
        return false;
    };

    let candidate = if season_only_segment(parent) {
        parent.parent()
    } else {
        Some(parent)
    };

    candidate
        .and_then(|segment| segment.file_name())
        .and_then(|segment| segment.to_str())
        .map(|segment| crate::scanner::parse_filename(segment).0)
        .map(|title| title_has_series_signal(&title))
        .unwrap_or(false)
}

fn season_only_segment(path: &Path) -> bool {
    path.file_name()
        .and_then(|segment| segment.to_str())
        .and_then(|segment| season_only_regex().captures(segment))
        .is_some()
}

fn title_has_series_signal(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.is_empty()
        && trimmed.chars().any(|ch| ch.is_alphabetic())
        && !trimmed.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn log_series_detection_miss(movie: &Movie) {
    let path_text = movie.file_path.to_string_lossy();
    let normalized_path = path_text.to_ascii_lowercase();
    let path_looks_series = ["/tv/", "/anime/", "/series/"]
        .iter()
        .any(|segment| normalized_path.contains(segment));

    if !path_looks_series {
        return;
    }

    let stem = movie
        .file_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let bracket_episode = crate::scanner::bracket_episode_marker(stem).map(|(_, episode)| episode);
    let dashed_episode =
        crate::scanner::loose_numbered_episode_suffix(stem).map(|(_, episode)| episode);
    let plain_numeric = plain_numeric_episode_number(stem);
    let episode_only = episode_only_regex()
        .captures(stem)
        .and_then(|caps| caps.name("episode"))
        .map(|episode| episode.as_str().to_string());
    let season_from_path = detect_season_from_ancestors(movie.file_path.parent());
    let series_context = detect_series_context(movie.file_path.parent());

    tracing::info!(
        movie_id = movie.id,
        movie_title = %movie.title,
        file_path = %path_text,
        file_stem = %stem,
        bracket_episode = ?bracket_episode,
        dashed_episode = ?dashed_episode,
        plain_numeric = ?plain_numeric,
        episode_only = ?episode_only,
        season_from_path = ?season_from_path,
        series_context,
        "Series detection miss: item remained a movie despite TV/Anime/Series path"
    );
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

        let dashed_movie = movie(
            4,
            "Fate stay night UBW",
            "/media/Fate stay night UBW/Kamigami Fate stay night UBW - 03.mkv",
        );
        let info = detect_episode_info(&dashed_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 3);

        let numeric_movie = movie(5, "Tiny World", "/media/Tiny World/Season 1/01.mkv");
        let info = detect_episode_info(&numeric_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 1);

        let default_season_movie = movie(6, "Tiny World", "/media/Tiny World/EP03.mkv");
        let info = detect_episode_info(&default_season_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 3);

        let release_tail_movie = movie(
            7,
            "Clevatess",
            "/media/Clevatess/Season 1/[LoliHouse] Clevatess - 05 [WebRip 1080p HEVC-10bit AAC ASSx2].mkv",
        );
        let info = detect_episode_info(&release_tail_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 5);

        let special_suffix_movie = movie(
            8,
            "Fate stay night UBW - ED04",
            "/media/TV/Anime/Fate stay night UBW/[Kamigami] Fate stay night UBW - ED04 [1080p x265 Ma10p FLAC].mkv",
        );
        let info = detect_episode_info(&special_suffix_movie).unwrap();
        assert_eq!(info.season_number, 0);
        assert_eq!(info.episode_number, 404);

        let bracketed_release_episode_movie = movie(
            9,
            "Fate Zero",
            "/media/Fate Zero/[SumiSora&MAGI_ATELIER&CASO][Fate_Zero][BDRip][14][x264_flac](9CF3C185).mkv",
        );
        let info = detect_episode_info(&bracketed_release_episode_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 14);

        let bracketed_special_episode_movie = movie(
            10,
            "Fate Stay Night 2006",
            "/media/TV/Anime/Fate Stay Night(2006)/SPs/[VCB-Studio] Fate Stay Night 2006 [PV02][Ma10p_1080p][x265_flac].mkv",
        );
        let info = detect_episode_info(&bracketed_special_episode_movie).unwrap();
        assert_eq!(info.season_number, 0);
        assert_eq!(info.episode_number, 202);

        let named_special_movie = movie(
            11,
            "Fate stay night UBW",
            "/media/TV/Anime/Fate stay night UBW/[Kamigami] Fate stay night UBW - Sunny Day [1080p x265 Ma10p FLAC].mkv",
        );
        let info = detect_episode_info(&named_special_movie).unwrap();
        assert_eq!(info.season_number, 0);
        assert_eq!(info.episode_number, 501);

        let versioned_episode_movie = movie(
            12,
            "Fate Stay Night 2006",
            "/media/TV/Anime/Fate Stay Night(2006)/[VCB-Studio] Fate Stay Night 2006 [13v2][Ma10p_1080p][x265_flac].mkv",
        );
        let info = detect_episode_info(&versioned_episode_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 13);

        let late_bracket_movie = movie(
            13,
            "Fate Stay Night",
            "/media/TV/Anime/Fate Stay Night(2006)/[VCB-Studio] Fate Stay Night 2006 [24v2][Ma10p_1080p][x265_flac].mkv",
        );
        let info = detect_episode_info(&late_bracket_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 24);

        let tv_reproduction_movie = movie(
            14,
            "Fate Stay Night",
            "/media/TV/Anime/Fate Stay Night(2006)/[VCB-Studio] Fate Stay Night TV Reproduction [02][Ma10p_1080p][x265_flac].mkv",
        );
        let info = detect_episode_info(&tv_reproduction_movie).unwrap();
        assert_eq!(info.season_number, 0);
        assert_eq!(info.episode_number, 452);
        assert_eq!(info.title_override.as_deref(), Some("TV Reproduction 2"));

        let goblin_plain_number_movie = movie(
            15,
            "Goblin Slayer",
            "/media/TV/Anime/Goblin Slayer/[Nekomoe kissaten] Goblin Slayer 12 [BDRip 1080p HEVC-10bit FLACx2].mkv",
        );
        let info = detect_episode_info(&goblin_plain_number_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 12);

        let goblin_menu_movie = movie(
            16,
            "Goblin Slayer",
            "/media/TV/Anime/Goblin Slayer/Menus/[Nekomoe kissaten] Goblin Slayer Menu 01 [BDRip 1080p HEVC-10bit FLAC].mkv",
        );
        let info = detect_episode_info(&goblin_menu_movie).unwrap();
        assert_eq!(info.season_number, 0);
        assert_eq!(info.episode_number, 251);
        assert_eq!(info.title_override.as_deref(), Some("Menu 1"));

        let goblin_cd_movie = movie(
            17,
            "Goblin Slayer",
            "/media/TV/Anime/Goblin Slayer/CDs/[Nekomoe kissaten] Goblin Slayer CD 02.mkv",
        );
        let info = detect_episode_info(&goblin_cd_movie).unwrap();
        assert_eq!(info.season_number, 0);
        assert_eq!(info.episode_number, 52);
        assert_eq!(info.title_override.as_deref(), Some("CD 2"));

        let goblin_pv_movie = movie(
            18,
            "Goblin Slayer",
            "/media/TV/Anime/Goblin Slayer/SPs/[Nekomoe kissaten] Goblin Slayer PV 01 [BDRip 1080p HEVC-10bit FLAC].mkv",
        );
        let info = detect_episode_info(&goblin_pv_movie).unwrap();
        assert_eq!(info.season_number, 0);
        assert_eq!(info.episode_number, 201);
        assert_eq!(info.title_override.as_deref(), Some("PV1"));

        let hunter_movie = movie(
            19,
            "Hunter X Hunter",
            "/media/TV/Anime/Hunter X Hunter/[Kamigami] Hunter X Hunter - 06 [x264 1280x720 AAC Sub(CH,JP)].mkv",
        );
        let info = detect_episode_info(&hunter_movie).unwrap();
        assert_eq!(info.season_number, 1);
        assert_eq!(info.episode_number, 6);
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
    fn test_media_tree_groups_numeric_episode_files_under_series_folder() {
        let tree = MediaTree::from_movies(vec![
            movie(1, "Tiny World", "/media/Tiny World/01.mkv"),
            movie(2, "Tiny World", "/media/Tiny World/02.mkv"),
            movie(3, "Inception", "/media/Inception.2010.mkv"),
        ]);

        let root = tree.paged_items(None, None, 0, None);
        assert_eq!(root.total_record_count, 2);

        let series = root
            .items
            .iter()
            .find(|item| item.kind == LibraryItemKind::Series)
            .unwrap();
        let seasons = tree.paged_items(Some(&series.id), None, 0, None);
        assert_eq!(seasons.total_record_count, 1);
        let episodes = tree.paged_items(Some(&seasons.items[0].id), None, 0, None);
        assert_eq!(episodes.total_record_count, 2);
        assert_eq!(episodes.items[0].episode_number, Some(1));
        assert_eq!(episodes.items[1].episode_number, Some(2));
    }

    #[test]
    fn test_media_tree_groups_release_tagged_numbered_episodes() {
        let tree = MediaTree::from_movies(vec![
            movie(
                1,
                "Clevatess - 05",
                "/media/Clevatess/Season 1/[LoliHouse] Clevatess - 05 [WebRip 1080p HEVC-10bit AAC ASSx2].mkv",
            ),
            movie(
                2,
                "Clevatess - 06",
                "/media/Clevatess/Season 1/[LoliHouse] Clevatess - 06 [WebRip 1080p HEVC-10bit AAC ASSx2].mkv",
            ),
        ]);

        let root = tree.paged_items(None, None, 0, None);
        assert_eq!(root.total_record_count, 1);
        assert_eq!(root.items[0].kind, LibraryItemKind::Series);
    }

    #[test]
    fn test_media_tree_groups_special_suffix_anime_entries_into_series() {
        let tree = MediaTree::from_movies(vec![movie(
            1,
            "Fate stay night UBW - ED04",
            "/media/TV/Anime/Fate stay night UBW/[Kamigami] Fate stay night UBW - ED04 [1080p x265 Ma10p FLAC].mkv",
        )]);

        let root = tree.paged_items(None, None, 0, None);
        assert_eq!(root.total_record_count, 1);
        assert_eq!(root.items[0].kind, LibraryItemKind::Series);

        let seasons = tree.paged_items(Some(&root.items[0].id), None, 0, None);
        assert_eq!(seasons.total_record_count, 1);
        assert_eq!(seasons.items[0].season_number, Some(0));
    }

    #[test]
    fn test_media_tree_groups_bracketed_release_episode_entries() {
        let tree = MediaTree::from_movies(vec![movie(
            1,
            "Fate Zero",
            "/media/TV/Anime/Fate Zero/[SumiSora&MAGI_ATELIER&CASO][Fate_Zero][BDRip][14][x264_flac](9CF3C185).mkv",
        )]);

        let root = tree.paged_items(None, None, 0, None);
        assert_eq!(root.total_record_count, 1);
        assert_eq!(root.items[0].kind, LibraryItemKind::Series);
    }

    #[test]
    fn test_media_tree_groups_bracketed_special_episode_entries() {
        let tree = MediaTree::from_movies(vec![movie(
            1,
            "VCB-Studio Fate Stay Night",
            "/media/TV/Anime/Fate Stay Night(2006)/SPs/[VCB-Studio] Fate Stay Night 2006 [PV02][Ma10p_1080p][x265_flac].mkv",
        )]);

        let root = tree.paged_items(None, None, 0, None);
        assert_eq!(root.total_record_count, 1);
        assert_eq!(root.items[0].kind, LibraryItemKind::Series);
        let seasons = tree.paged_items(Some(&root.items[0].id), None, 0, None);
        assert_eq!(seasons.items[0].season_number, Some(0));
    }

    #[test]
    fn test_media_tree_groups_named_special_entries() {
        let tree = MediaTree::from_movies(vec![movie(
            1,
            "Fate stay night UBW",
            "/media/TV/Anime/Fate stay night UBW/[Kamigami] Fate stay night UBW - Sunny Day [1080p x265 Ma10p FLAC].mkv",
        )]);

        let root = tree.paged_items(None, None, 0, None);
        assert_eq!(root.total_record_count, 1);
        assert_eq!(root.items[0].kind, LibraryItemKind::Series);
        let seasons = tree.paged_items(Some(&root.items[0].id), None, 0, None);
        assert_eq!(seasons.items[0].season_number, Some(0));
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

    #[test]
    fn test_media_tree_groups_user_anime_samples_into_main_and_specials() {
        let tree = MediaTree::from_movies(vec![
            movie(
                1,
                "Fate Stay Night",
                "/vol2/1000/media/TV/Anime/Fate Stay Night(2006)/[VCB-Studio] Fate Stay Night 2006 [20][Ma10p_1080p][x265_flac].mkv",
            ),
            movie(
                2,
                "Fate Stay Night",
                "/vol2/1000/media/TV/Anime/Fate Stay Night(2006)/[VCB-Studio] Fate Stay Night 2006 [24v2][Ma10p_1080p][x265_flac].mkv",
            ),
            movie(
                3,
                "Fate Stay Night",
                "/vol2/1000/media/TV/Anime/Fate Stay Night(2006)/[VCB-Studio] Fate Stay Night TV Reproduction [01][Ma10p_1080p][x265_flac].mkv",
            ),
            movie(
                4,
                "Goblin Slayer",
                "/vol2/1000/media/TV/Anime/Goblin Slayer/[Nekomoe kissaten] Goblin Slayer 01 [BDRip 1080p HEVC-10bit FLACx2].mkv",
            ),
            movie(
                5,
                "Goblin Slayer",
                "/vol2/1000/media/TV/Anime/Goblin Slayer/SPs/[Nekomoe kissaten] Goblin Slayer PV 01 [BDRip 1080p HEVC-10bit FLAC].mkv",
            ),
            movie(
                6,
                "Hunter X Hunter",
                "/vol2/1000/media/TV/Anime/Hunter X Hunter/[Kamigami] Hunter X Hunter - 06 [x264 1280x720 AAC Sub(CH,JP)].mkv",
            ),
        ]);

        let root = tree.paged_items(None, None, 0, None);
        let titles = root
            .items
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(root.total_record_count, 3);
        assert!(titles.contains(&"Fate Stay Night"));
        assert!(titles.contains(&"Goblin Slayer"));
        assert!(titles.contains(&"Hunter X Hunter"));

        let fate = root
            .items
            .iter()
            .find(|item| item.title == "Fate Stay Night")
            .unwrap();
        let fate_seasons = tree.paged_items(Some(&fate.id), None, 0, None);
        let fate_season_numbers = fate_seasons
            .items
            .iter()
            .map(|item| item.season_number)
            .collect::<Vec<_>>();
        assert!(fate_season_numbers.contains(&Some(0)));
        assert!(fate_season_numbers.contains(&Some(1)));

        let specials = fate_seasons
            .items
            .iter()
            .find(|item| item.season_number == Some(0))
            .unwrap();
        let special_episodes = tree.paged_items(Some(&specials.id), None, 0, None);
        assert_eq!(
            special_episodes.items[0].title,
            "TV Reproduction 1".to_string()
        );
    }
}
