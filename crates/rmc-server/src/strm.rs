pub struct StrmParser;

impl StrmParser {
    pub fn parse(content: &str) -> Option<String> {
        let trimmed = content.trim();
        if trimmed.starts_with("http") {
            Some(trimmed.to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]

mod tests {
    use super::*;
    #[test]
    fn test_parse_strm_file() {
        let url = StrmParser::parse("http://example.com/movie.mp4\n");
        assert_eq!(url, Some("http://example.com/movie.mp4".to_string()));
    }

    #[test]
    fn test_parse_invalid_strm() {
        let url = StrmParser::parse("not a url");
        assert_eq!(url, None);
    }
}
