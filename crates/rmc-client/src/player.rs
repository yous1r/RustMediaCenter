pub struct PlayerBridge {
    ready: bool,
}

impl PlayerBridge {
    pub fn new() -> Self {
        Self { ready: false }
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }
}

impl Default for PlayerBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_player_bridge_init() {
        let bridge = PlayerBridge::new();
        assert_eq!(bridge.is_ready(), false);
    }
}
