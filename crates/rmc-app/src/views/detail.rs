use dioxus::prelude::*;

pub fn format_runtime(runtime_seconds: Option<u32>, runtime_minutes: Option<u16>) -> String {
    let total_minutes = runtime_seconds
        .map(|seconds| seconds / 60)
        .or_else(|| runtime_minutes.map(u32::from));

    let Some(total_minutes) = total_minutes else {
        return "Unknown runtime".to_string();
    };

    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[component]
pub fn DetailView(
    title: String,
    overview: Option<String>,
    runtime_seconds: Option<u32>,
    runtime_minutes: Option<u16>,
) -> Element {
    let runtime = format_runtime(runtime_seconds, runtime_minutes);
    rsx! {
        section { class: "detail-page",
            h1 { "{title}" }
            p { "{runtime}" }
            if let Some(overview) = overview {
                p { "{overview}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_runtime_prefers_seconds() {
        assert_eq!(format_runtime(Some(5400), Some(80)), "1h 30m");
    }

    #[test]
    fn format_runtime_falls_back_to_minutes() {
        assert_eq!(format_runtime(None, Some(95)), "1h 35m");
    }
}
