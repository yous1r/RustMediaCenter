use iced::widget::{column, container, text};
use iced::{Element, Length};
use rmc_core::models::Movie;

pub struct PosterWall {
    movies: Vec<Movie>,
}

impl PosterWall {
    pub fn new(movies: Vec<Movie>) -> Self {
        Self { movies }
    }

    pub fn count(&self) -> usize {
        self.movies.len()
    }

    // 返回一个真实的 iced Element
    pub fn view<'a, Message: 'a>(&self) -> Element<'a, Message> {
        if self.movies.is_empty() {
            return container(text("No movies found. Please wait..."))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
        }

        // 用简单的列展示（未来再进化为真正的虚拟网格或包裹排版）
        let mut col = column![].spacing(10);
        for m in &self.movies {
            let year_str = m
                .year
                .map(|y| y.to_string())
                .unwrap_or_else(|| "Unknown".to_string());
            // 目前先显示文本和标题，后续配合网络加载图片
            col = col.push(text(format!("{} ({})", m.title, year_str)));
        }

        container(col).padding(20).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_poster_wall_grid_count() {
        let wall = PosterWall::new(vec![]);
        assert_eq!(wall.count(), 0);
    }

    #[test]
    fn test_poster_wall_view_type() {
        let wall = PosterWall::new(vec![]);
        // iced 的 view 方法应该返回一个能够被类型检查为 widget 的组件结构
        let _widget = wall.view::<()>();
        assert_eq!(wall.count(), 0);
    }

    #[test]
    fn test_poster_wall_view_with_movies() {
        use std::path::PathBuf;
        let movies = vec![
            Movie {
                id: 1,
                title: "Movie with year".to_string(),
                year: Some(2023),
                file_path: PathBuf::from("/m/1.mp4"),
                poster_url: None,
                overview: None,
                tmdb_id: None,
                runtime_minutes: None,
                runtime_seconds: None,
                episode_number: None,
                added_at: 0,
                file_size: None,
            },
            Movie {
                id: 2,
                title: "Movie without year".to_string(),
                year: None,
                file_path: PathBuf::from("/m/2.mp4"),
                poster_url: None,
                overview: None,
                tmdb_id: None,
                runtime_minutes: None,
                runtime_seconds: None,
                episode_number: None,
                added_at: 0,
                file_size: None,
            },
        ];
        let wall = PosterWall::new(movies);
        let _widget = wall.view::<()>();
        assert_eq!(wall.count(), 2);
    }
}
