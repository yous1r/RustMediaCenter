use thiserror::Error;
use url::Url;

pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:19000";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConfigError {
    #[error("server URL must not be empty")]
    Empty,
    #[error("server URL must be absolute HTTP(S): {0}")]
    InvalidUrl(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    base_url: String,
}

impl ServerConfig {
    pub fn from_base_url(raw: impl AsRef<str>) -> Result<Self, ConfigError> {
        let trimmed = raw.as_ref().trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(ConfigError::Empty);
        }

        let parsed =
            Url::parse(trimmed).map_err(|_| ConfigError::InvalidUrl(trimmed.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(ConfigError::InvalidUrl(trimmed.to_string()));
        }

        Ok(Self {
            base_url: trimmed.to_string(),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_SERVER_URL.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_server_url_matches_rmc_server_default() {
        let config = ServerConfig::default();
        assert_eq!(config.base_url(), "http://127.0.0.1:19000");
    }

    #[test]
    fn server_config_normalizes_trailing_slashes() {
        let config = ServerConfig::from_base_url("http://media.local:19000///").unwrap();
        assert_eq!(config.base_url(), "http://media.local:19000");
    }

    #[test]
    fn server_config_rejects_empty_and_relative_urls() {
        assert!(ServerConfig::from_base_url("").is_err());
        assert!(ServerConfig::from_base_url("/api/v1").is_err());
    }
}
