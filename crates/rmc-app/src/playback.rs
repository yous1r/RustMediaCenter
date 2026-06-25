use crate::config::ServerConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackMode {
    Direct,
    Transcode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackUrl {
    value: String,
}

impl PlaybackUrl {
    pub fn as_str(&self) -> &str {
        &self.value
    }
}

#[derive(Debug, Clone)]
pub struct PlaybackUrlBuilder {
    config: ServerConfig,
}

impl PlaybackUrlBuilder {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    pub fn url_for(&self, play_id: i64, mode: PlaybackMode) -> PlaybackUrl {
        let suffix = match mode {
            PlaybackMode::Direct => "direct",
            PlaybackMode::Transcode => "stream.mp4",
        };

        PlaybackUrl {
            value: format!(
                "{}/api/v1/movies/{}/{}",
                self.config.base_url(),
                play_id,
                suffix
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WebViewPlaybackAdapter {
    builder: PlaybackUrlBuilder,
}

impl WebViewPlaybackAdapter {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            builder: PlaybackUrlBuilder::new(config),
        }
    }

    pub fn video_source(&self, play_id: i64, mode: PlaybackMode) -> PlaybackUrl {
        self.builder.url_for(play_id, mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;

    #[test]
    fn direct_url_uses_movie_direct_endpoint() {
        let config = ServerConfig::from_base_url("http://server:19000/").unwrap();
        let builder = PlaybackUrlBuilder::new(config);
        assert_eq!(
            builder.url_for(42, PlaybackMode::Direct).as_str(),
            "http://server:19000/api/v1/movies/42/direct"
        );
    }

    #[test]
    fn transcode_url_uses_stream_mp4_endpoint() {
        let config = ServerConfig::from_base_url("http://server:19000").unwrap();
        let builder = PlaybackUrlBuilder::new(config);
        assert_eq!(
            builder.url_for(42, PlaybackMode::Transcode).as_str(),
            "http://server:19000/api/v1/movies/42/stream.mp4"
        );
    }

    #[test]
    fn webview_adapter_passes_direct_url_to_video_without_resolving_redirect() {
        let config = ServerConfig::from_base_url("http://server:19000").unwrap();
        let adapter = WebViewPlaybackAdapter::new(config);

        let source = adapter.video_source(42, PlaybackMode::Direct);

        assert_eq!(
            source.as_str(),
            "http://server:19000/api/v1/movies/42/direct"
        );
    }
}
