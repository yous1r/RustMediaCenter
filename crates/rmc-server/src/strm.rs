#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrmSource {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl StrmSource {
    pub fn ffmpeg_header_value(&self) -> Option<String> {
        if self.headers.is_empty() {
            return None;
        }

        Some(
            self.headers
                .iter()
                .map(|(name, value)| format!("{}: {}\r\n", name, value))
                .collect(),
        )
    }
}

pub struct StrmParser;

impl StrmParser {
    pub fn parse(content: &str) -> Option<StrmSource> {
        let mut lines = content.lines().map(str::trim).filter(|line| !line.is_empty());
        let url = lines.find(|line| line.starts_with("http"))?.to_string();
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                let name = name.trim();
                let value = value.trim();

                if name.is_empty() || value.is_empty() {
                    return None;
                }

                Some((name.to_string(), value.to_string()))
            })
            .collect();

        Some(StrmSource { url, headers })
    }
}

#[cfg(test)]

mod tests {
    use super::*;
    #[test]
    fn test_parse_strm_file() {
        let source = StrmParser::parse("http://example.com/movie.mp4\n");
        assert_eq!(
            source,
            Some(StrmSource {
                url: "http://example.com/movie.mp4".to_string(),
                headers: vec![],
            })
        );
    }

    #[test]
    fn test_parse_invalid_strm() {
        let source = StrmParser::parse("not a url");
        assert_eq!(source, None);
    }

    #[test]
    fn test_parse_strm_headers_for_ffmpeg() {
        let source = StrmParser::parse(
            "https://example.com/movie.mkv\nUser-Agent: VidHub\nReferer: https://example.com\n",
        )
        .unwrap();

        assert_eq!(source.url, "https://example.com/movie.mkv");
        assert_eq!(
            source.headers,
            vec![
                ("User-Agent".to_string(), "VidHub".to_string()),
                ("Referer".to_string(), "https://example.com".to_string()),
            ]
        );
        assert_eq!(
            source.ffmpeg_header_value().as_deref(),
            Some("User-Agent: VidHub\r\nReferer: https://example.com\r\n")
        );
    }
}
