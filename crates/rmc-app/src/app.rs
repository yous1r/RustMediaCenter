use crate::api::ApiClient;
use crate::config::{ServerConfig, DEFAULT_SERVER_URL};
use crate::playback::{PlaybackMode, WebViewPlaybackAdapter};
use crate::state::AppRoute;
use crate::views::detail::DetailView;
use crate::views::player::PlayerView;
use dioxus::prelude::*;
use rmc_core::api_types::LibraryItem;

#[component]
pub fn App() -> Element {
    let mut server_input = use_signal(|| DEFAULT_SERVER_URL.to_string());
    let mut server_config = use_signal(ServerConfig::default);
    let mut config_error = use_signal(|| None::<String>);
    let mut route = use_signal(|| AppRoute::Library);
    let mut selected_item = use_signal(|| None::<LibraryItem>);
    let mut library = use_resource(move || {
        let config = server_config();
        async move { ApiClient::new(config).fetch_playable_items().await }
    });

    let current_route = route();
    let current_config = server_config();
    let current_config_error = config_error();
    let current_selected_item = selected_item();
    let playback = WebViewPlaybackAdapter::new(current_config.clone());

    rsx! {
        main { class: "rmc-app",
            header { class: "app-header",
                div {
                    h1 { "RustMediaCenter" }
                    p { "Desktop / Android / iOS client" }
                }
                div { class: "server-config",
                    label { r#for: "server-url", "rmc-server" }
                    input {
                        id: "server-url",
                        r#type: "url",
                        value: "{server_input()}",
                        oninput: move |event| server_input.set(event.value()),
                    }
                    button {
                        r#type: "button",
                        onclick: move |_| match ServerConfig::from_base_url(server_input()) {
                            Ok(next_config) => {
                                config_error.set(None);
                                server_config.set(next_config);
                                route.set(AppRoute::Library);
                                library.restart();
                            }
                            Err(err) => config_error.set(Some(err.to_string())),
                        },
                        "保存并刷新"
                    }
                    button {
                        r#type: "button",
                        onclick: move |_| library.restart(),
                        "刷新媒体库"
                    }
                }
                if let Some(error) = current_config_error {
                    p { class: "error-message", "{error}" }
                }
            }

            nav { class: "app-nav",
                button {
                    r#type: "button",
                    onclick: move |_| route.set(AppRoute::Library),
                    "媒体库"
                }
                button {
                    r#type: "button",
                    onclick: move |_| route.set(AppRoute::Settings),
                    "设置"
                }
            }

            match current_route {
                AppRoute::Library => rsx! {
                    section { class: "library-page",
                        h2 { "可播放条目" }
                        match &*library.value().read_unchecked() {
                            Some(Ok(items)) => {
                                let items = items.clone();
                                rsx! {
                                    if items.is_empty() {
                                        p { "没有找到 movie 或 episode 类型的可播放条目。" }
                                    } else {
                                        div { class: "library-grid",
                                            for item in items {
                                                article { class: "media-card", key: "{item.id}",
                                                    h3 { "{item.title}" }
                                                    if let Some(year) = item.year {
                                                        p { "{year}" }
                                                    }
                                                    div { class: "media-actions",
                                                        {
                                                            let detail_item = item.clone();
                                                            rsx! {
                                                                button {
                                                                    r#type: "button",
                                                                    onclick: move |_| {
                                                                        selected_item.set(Some(detail_item.clone()));
                                                                        route.set(AppRoute::Detail {
                                                                            item_id: detail_item.id.clone(),
                                                                        });
                                                                    },
                                                                    "详情"
                                                                }
                                                            }
                                                        }
                                                        if let Some(play_id) = item.play_id {
                                                            button {
                                                                r#type: "button",
                                                                onclick: move |_| route.set(AppRoute::Player {
                                                                    play_id,
                                                                    mode: PlaybackMode::Direct,
                                                                }),
                                                                "直连播放"
                                                            }
                                                            button {
                                                                r#type: "button",
                                                                onclick: move |_| route.set(AppRoute::Player {
                                                                    play_id,
                                                                    mode: PlaybackMode::Transcode,
                                                                }),
                                                                "转码播放"
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Some(Err(err)) => rsx! {
                                p { class: "error-message", "加载媒体库失败：{err}" }
                            },
                            None => rsx! {
                                p { "正在从 {current_config.base_url()} 加载媒体库..." }
                            },
                        }
                    }
                },
                AppRoute::Detail { .. } => rsx! {
                    section { class: "detail-route",
                        if let Some(item) = current_selected_item {
                            DetailView {
                                title: item.title.clone(),
                                overview: item.overview.clone(),
                                runtime_seconds: item.runtime_seconds,
                                runtime_minutes: item.runtime_minutes,
                            }
                            dl { class: "detail-facts",
                                if let Some(year) = item.year {
                                    dt { "年份" }
                                    dd { "{year}" }
                                }
                                if let Some(size) = item.file_size {
                                    dt { "文件大小" }
                                    dd { "{size} bytes" }
                                }
                            }
                            if let Some(play_id) = item.play_id {
                                div { class: "media-actions",
                                    button {
                                        r#type: "button",
                                        onclick: move |_| route.set(AppRoute::Player {
                                            play_id,
                                            mode: PlaybackMode::Direct,
                                        }),
                                        "直连播放（支持 302）"
                                    }
                                    button {
                                        r#type: "button",
                                        onclick: move |_| route.set(AppRoute::Player {
                                            play_id,
                                            mode: PlaybackMode::Transcode,
                                        }),
                                        "转码播放"
                                    }
                                }
                            }
                        } else {
                            p { "尚未选择媒体条目。" }
                        }
                    }
                },
                AppRoute::Player { play_id, mode } => {
                    let play_url = playback.video_source(play_id, mode);
                    rsx! {
                        section { class: "player-route",
                            button {
                                r#type: "button",
                                onclick: move |_| route.set(AppRoute::Library),
                                "返回媒体库"
                            }
                            PlayerView { play_url, mode }
                        }
                    }
                },
                AppRoute::Settings => rsx! {
                    section { class: "settings-page",
                        h2 { "连接设置" }
                        p { "当前服务端：{current_config.base_url()}" }
                        p { "移动端真机调试时，请把 127.0.0.1 改为运行 rmc-server 的局域网地址。" }
                    }
                },
            }
        }
    }
}
