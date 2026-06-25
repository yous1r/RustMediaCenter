use dioxus::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryCard {
    pub id: String,
    pub title: String,
    pub year: Option<u16>,
}

#[component]
pub fn LibraryView(items: Vec<LibraryCard>) -> Element {
    rsx! {
        section { class: "library-page",
            h1 { "RustMediaCenter" }
            div { class: "library-grid",
                for item in items {
                    article { class: "media-card", key: "{item.id}",
                        h2 { "{item.title}" }
                        if let Some(year) = item.year {
                            p { "{year}" }
                        }
                    }
                }
            }
        }
    }
}
