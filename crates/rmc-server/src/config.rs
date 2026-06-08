use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub media_dir: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8000,
            media_dir: "/media".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_config_defaults() {
        let config = ServerConfig::default();
        assert_eq!(config.port, 8000);
        assert_eq!(config.media_dir, "/media");
    }
}
