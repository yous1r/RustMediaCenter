use std::path::Path;
use std::sync::OnceLock;
use walkdir::WalkDir;
use crate::db::Database;
use rmc_core::models::Movie;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "mpg", "mpeg", "strm"
];

#[derive(Clone)]
pub struct MediaScanner {
    db: Database,
}

impl MediaScanner {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    pub async fn scan_directory(&self, dir: &str) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
        let dir_owned = dir.to_string();
        
        // 在独立线程池中执行同步阻塞的文件目录树遍历
        let matching_files = tokio::task::spawn_blocking(move || {
            let mut list = Vec::new();
            let path = Path::new(&dir_owned);
            if !path.exists() {
                return list;
            }

            for entry in WalkDir::new(path)
                .follow_links(true)
                .into_iter()
                .filter_map(|e| match e {
                    Ok(e) => Some(e),
                    Err(err) => {
                        tracing::warn!("Failed to read directory entry during scan: {:?}", err);
                        None
                    }
                })
            {
                let file_path = entry.path();
                if !file_path.is_file() {
                    continue;
                }

                let ext = file_path.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .unwrap_or_default();

                if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
                    list.push(file_path.to_path_buf());
                }
            }
            list
        }).await?;

        let mut count = 0u32;
        for file_path in matching_files {
            let path_str = file_path.to_string_lossy().into_owned();
            if self.db.movie_exists_by_path(&path_str).await? {
                continue;
            }

            let file_stem = file_path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown");

            let (title, year) = parse_filename(file_stem);
            let file_size = std::fs::metadata(&file_path).map(|m| m.len()).ok();

            let movie = Movie {
                id: 0,
                title,
                year,
                file_path: file_path.clone(),
                poster_url: None,
                overview: None,
                tmdb_id: None,
                runtime_minutes: None,
                added_at: chrono::Utc::now().timestamp(),
                file_size,
            };

            self.db.insert_movie(&movie).await?;
            count += 1;
        }

        Ok(count)
    }
}

pub fn parse_filename(stem: &str) -> (String, Option<u16>) {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"[\.\s\-_\(]?((?:19|20)\d{2})[\.\s\-_\)]?").unwrap()
    });
    
    if let Some(caps) = re.captures(stem) {
        let year: u16 = caps[1].parse().unwrap_or(0);
        let title_part = &stem[..caps.get(0).unwrap().start()];
        let title = title_part
            .replace('.', " ")
            .replace('_', " ")
            .trim()
            .to_string();
        (if title.is_empty() { stem.to_string() } else { title }, Some(year))
    } else {
        let title = stem.replace('.', " ").replace('_', " ").trim().to_string();
        (title, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::fs;

    #[tokio::test]
    async fn test_media_scanner_local_and_strm() {
        // 使用 tempfile::tempdir() 自动管理临时文件夹
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        
        // 创建模拟 mp4 文件
        let mp4_file = temp_path.join("Inception.2010.Bluray.mp4");
        fs::write(&mp4_file, "mock video content").unwrap();
        
        // 创建模拟 strm 文件
        let strm_file = temp_path.join("The.Matrix.2003.strm");
        fs::write(&strm_file, "https://example.com/matrix.mkv").unwrap();
        
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.init_schema().await.unwrap();
        
        let scanner = MediaScanner::new(db.clone());
        let count = scanner.scan_directory(&temp_path.to_string_lossy().into_owned()).await.unwrap();
        assert_eq!(count, 2);
        
        let movies = db.get_movies().await.unwrap();
        assert_eq!(movies.len(), 2);
        
        // 年份和标题提取检验
        let inception = movies.iter().find(|m| m.title == "Inception").unwrap();
        assert_eq!(inception.year, Some(2010));
        assert_eq!(inception.file_path, mp4_file);
        
        let matrix = movies.iter().find(|m| m.title == "The Matrix").unwrap();
        assert_eq!(matrix.year, Some(2003));
        assert_eq!(matrix.file_path, strm_file);
        
        // temp_dir 会在离开作用域时被自动清理
    }

    #[test]
    fn test_parse_filename_edge_cases() {
        assert_eq!(parse_filename("Simple Movie"), ("Simple Movie".to_string(), None));
        assert_eq!(parse_filename("Movie.Name.2021.1080p"), ("Movie Name".to_string(), Some(2021)));
        assert_eq!(parse_filename("NoYear"), ("NoYear".to_string(), None));
        assert_eq!(parse_filename("2020"), ("2020".to_string(), Some(2020)));
    }
}
