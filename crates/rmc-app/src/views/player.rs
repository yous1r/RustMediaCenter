use crate::playback::{PlaybackMode, PlaybackUrl};
use dioxus::prelude::*;

#[component]
pub fn PlayerView(play_url: PlaybackUrl, mode: PlaybackMode) -> Element {
    let mode_label = match mode {
        PlaybackMode::Direct => "Direct",
        PlaybackMode::Transcode => "Transcode",
    };

    rsx! {
        section { class: "player-page",
            header {
                h1 { "Player" }
                p { "{mode_label}" }
            }
            video {
                controls: true,
                playsinline: true,
                preload: "metadata",
                src: "{play_url.as_str()}",
            }
        }
    }
}
