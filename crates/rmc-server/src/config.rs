use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub media_dirs: Vec<String>,
    pub db_path: String,
    pub tmdb_api_key: Option<String>,
    pub tmdb_proxy_url: Option<String>,
    pub tmdb_api_base: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8000,
            media_dirs: vec!["/media".to_string()],
            db_path: "rmc.db".to_string(),
            tmdb_api_key: None,
            tmdb_proxy_url: None,
            tmdb_api_base: None,
        }
    }
}

impl ServerConfig {
    pub fn load_from<P: AsRef<Path>>(path: P) -> Result<Self, anyhow::Error> {
        let path = path.as_ref();
        if !path.exists() {
            let default_config = Self::default();
            default_config.save_to(path)?;
            return Ok(default_config);
        }
        let content = fs::read_to_string(path)?;
        let config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save_to<P: AsRef<Path>>(&self, path: P) -> Result<(), anyhow::Error> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string(self)?;
        fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = ServerConfig::default();
        assert_eq!(config.port, 8000);
        assert_eq!(config.media_dirs, vec!["/media".to_string()]);
        assert_eq!(config.db_path, "rmc.db");
        assert_eq!(config.tmdb_api_key, None);
        assert_eq!(config.tmdb_proxy_url, None);
        assert_eq!(config.tmdb_api_base, None);
    }

    #[test]
    fn test_config_load_and_save() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        // 1. 首次加载，文件不存在，应该自动创建并保存默认配置
        let mut config = ServerConfig::load_from(&config_path).unwrap();
        assert_eq!(config.port, 8000);
        assert_eq!(config.media_dirs, vec!["/media".to_string()]);
        assert_eq!(config.db_path, "rmc.db".to_string());
        assert_eq!(config.tmdb_api_key, None);
        assert_eq!(config.tmdb_proxy_url, None);
        assert_eq!(config.tmdb_api_base, None);
        assert!(config_path.exists());

        // 2. 修改配置并保存
        config.port = 9000;
        config.media_dirs = vec!["/media1".to_string(), "/media2".to_string()];
        config.db_path = "rmc_test.db".to_string();
        config.tmdb_api_key = Some("test_api_key".to_string());
        config.tmdb_proxy_url = Some("http://proxy.example.com".to_string());
        config.tmdb_api_base = Some("https://api.example.com".to_string());
        config.save_to(&config_path).unwrap();

        // 3. 再次加载，确保读取修改后的配置
        let loaded = ServerConfig::load_from(&config_path).unwrap();
        assert_eq!(loaded.port, 9000);
        assert_eq!(
            loaded.media_dirs,
            vec!["/media1".to_string(), "/media2".to_string()]
        );
        assert_eq!(loaded.db_path, "rmc_test.db".to_string());
        assert_eq!(loaded.tmdb_api_key, Some("test_api_key".to_string()));
        assert_eq!(
            loaded.tmdb_proxy_url,
            Some("http://proxy.example.com".to_string())
        );
        assert_eq!(
            loaded.tmdb_api_base,
            Some("https://api.example.com".to_string())
        );
    }
}
