use crate::config::ServerConfig;
use crate::playback::{PlaybackMode, PlaybackUrl, PlaybackUrlBuilder};
use rmc_core::api_types::{LibraryItem, LibraryItemKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRoute {
    Library,
    Detail { item_id: String },
    Player { play_id: i64, mode: PlaybackMode },
    Settings,
}

impl AppRoute {
    pub fn play_id(&self) -> Option<i64> {
        match self {
            Self::Player { play_id, .. } => Some(*play_id),
            _ => None,
        }
    }
}

pub fn playable_items(items: &[LibraryItem]) -> Vec<LibraryItem> {
    items
        .iter()
        .filter(|item| {
            item.play_id.is_some()
                && matches!(item.kind, LibraryItemKind::Movie | LibraryItemKind::Episode)
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone)]
pub enum Loadable<T> {
    Idle,
    Loading,
    Loaded(T),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ClientState {
    pub config: ServerConfig,
    pub route: AppRoute,
    pub library: Loadable<Vec<LibraryItem>>,
    pub selected_item: Loadable<LibraryItem>,
}

impl ClientState {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            route: AppRoute::Library,
            library: Loadable::Idle,
            selected_item: Loadable::Idle,
        }
    }

    pub fn apply_library_items(&mut self, items: Vec<LibraryItem>) {
        self.library = Loadable::Loaded(playable_items(&items));
        self.route = AppRoute::Library;
    }

    pub fn apply_library_error(&mut self, error: impl Into<String>) {
        self.library = Loadable::Failed(error.into());
    }

    pub fn select_item(&mut self, item: LibraryItem) {
        self.route = AppRoute::Detail {
            item_id: item.id.clone(),
        };
        self.selected_item = Loadable::Loaded(item);
    }

    pub fn open_player(&mut self, play_id: i64, mode: PlaybackMode) {
        self.route = AppRoute::Player { play_id, mode };
    }

    pub fn playback_url(&self, play_id: i64, mode: PlaybackMode) -> PlaybackUrl {
        PlaybackUrlBuilder::new(self.config.clone()).url_for(play_id, mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmc_core::api_types::{LibraryItem, LibraryItemKind};

    fn item(kind: LibraryItemKind, play_id: Option<i64>, title: &str) -> LibraryItem {
        LibraryItem {
            id: title.to_string(),
            kind,
            parent_id: None,
            title: title.to_string(),
            year: None,
            poster_url: None,
            overview: None,
            child_count: 0,
            play_id,
            season_number: None,
            episode_number: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        }
    }

    #[test]
    fn playable_items_include_movies_and_episodes_with_play_id() {
        let items = vec![
            item(LibraryItemKind::Movie, Some(1), "movie"),
            item(LibraryItemKind::Episode, Some(2), "episode"),
            item(LibraryItemKind::Series, None, "series"),
        ];

        let playable = playable_items(&items);

        assert_eq!(playable.len(), 2);
        assert_eq!(playable[0].title, "movie");
        assert_eq!(playable[1].title, "episode");
    }

    #[test]
    fn route_player_keeps_play_id_and_mode() {
        let route = AppRoute::Player {
            play_id: 7,
            mode: crate::playback::PlaybackMode::Transcode,
        };

        assert_eq!(route.play_id(), Some(7));
    }

    #[test]
    fn client_state_keeps_only_playable_library_entries() {
        let mut state = ClientState::new(ServerConfig::default());

        state.apply_library_items(vec![
            item(LibraryItemKind::Movie, Some(1), "movie"),
            item(LibraryItemKind::Episode, Some(2), "episode"),
            item(LibraryItemKind::Series, None, "series"),
            item(LibraryItemKind::Season, None, "season"),
        ]);

        let Loadable::Loaded(items) = state.library else {
            panic!("library should be loaded");
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(state.route, AppRoute::Library));
    }

    #[test]
    fn client_state_selects_detail_and_builds_playback_urls() {
        let config = ServerConfig::from_base_url("http://rmc.local:19000/").unwrap();
        let mut state = ClientState::new(config);
        state.select_item(item(LibraryItemKind::Movie, Some(42), "movie"));

        assert_eq!(
            state.route,
            AppRoute::Detail {
                item_id: "movie".to_string()
            }
        );
        assert_eq!(
            state
                .playback_url(42, crate::playback::PlaybackMode::Direct)
                .as_str(),
            "http://rmc.local:19000/api/v1/movies/42/direct"
        );
        assert_eq!(
            state
                .playback_url(42, crate::playback::PlaybackMode::Transcode)
                .as_str(),
            "http://rmc.local:19000/api/v1/movies/42/stream.mp4"
        );
    }
}
