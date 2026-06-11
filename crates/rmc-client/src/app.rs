use iced::widget::{column, container, scrollable, text};
use iced::{Element, Task};
use rmc_core::models::Movie;

const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:19000";

fn server_base_url() -> String {
    std::env::var("RMC_SERVER_URL").unwrap_or_else(|_| DEFAULT_SERVER_URL.to_string())
}

pub struct RmcApp {
    pub movies: Vec<Movie>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    LoadMovies,
    MoviesLoaded(Result<Vec<Movie>, String>),
}

impl Default for RmcApp {
    fn default() -> Self {
        Self {
            movies: Vec::new(),
            error_message: None,
        }
    }
}

impl RmcApp {
    pub fn new() -> (Self, Task<Message>) {
        (
            Self::default(),
            Task::perform(
                async {
                    let client = crate::api_client::ApiClient::new(server_base_url());
                    client.fetch_movies().await
                },
                Message::MoviesLoaded,
            ),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LoadMovies => {
                self.error_message = None;
                Task::perform(
                    async {
                        let client = crate::api_client::ApiClient::new(server_base_url());
                        client.fetch_movies().await
                    },
                    Message::MoviesLoaded,
                )
            }
            Message::MoviesLoaded(Ok(movies)) => {
                self.movies = movies;
                self.error_message = None;
                Task::none()
            }
            Message::MoviesLoaded(Err(e)) => {
                self.error_message = Some(e);
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        if let Some(ref err) = self.error_message {
            return container(text(format!("Error loading movies: {}", err)))
                .center(iced::Length::Fill)
                .into();
        }

        if self.movies.is_empty() {
            return container(text("Loading movies or library is empty..."))
                .center(iced::Length::Fill)
                .into();
        }

        let mut col = column![].spacing(10);
        for movie in &self.movies {
            let title = movie.title.clone();
            let year_str = movie
                .year
                .map(|y| y.to_string())
                .unwrap_or_else(|| "Unknown".to_string());
            col = col.push(text(format!("{} ({})", title, year_str)));
        }

        scrollable(container(col).padding(20)).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_initial_state() {
        let (app, _) = RmcApp::new();
        assert_eq!(app.movies.len(), 0);
    }

    #[test]
    fn test_update_movies_message() {
        let (mut app, _) = RmcApp::new();
        let test_movies = vec![Movie {
            id: 1,
            title: "Test Movie".to_string(),
            year: Some(2025),
            file_path: std::path::PathBuf::from("/m/test.mp4"),
            poster_url: None,
            overview: None,
            tmdb_id: None,
            runtime_minutes: None,
            runtime_seconds: None,
            added_at: 0,
            file_size: None,
        }];

        let _ = app.update(Message::MoviesLoaded(Ok(test_movies)));
        assert_eq!(app.movies.len(), 1);
        assert_eq!(app.movies[0].title, "Test Movie");
    }
}
