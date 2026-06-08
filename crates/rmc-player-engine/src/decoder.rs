// crates/rmc-player-engine/src/decoder.rs
pub struct Decoder {
    status: String,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            status: String::from("Initialized"),
        }
    }

    pub fn status(&self) -> &str {
        &self.status
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decoder_initialization() {
        let decoder = Decoder::new();
        assert_eq!(decoder.status(), "Initialized");
    }
}
