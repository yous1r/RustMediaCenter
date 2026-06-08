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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_poster_wall_grid_count() {
        let wall = PosterWall::new(vec![]);
        assert_eq!(wall.count(), 0);
    }
}
