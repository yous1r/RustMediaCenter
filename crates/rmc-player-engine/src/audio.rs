pub struct AudioOutput {
    playing: bool,
}

impl AudioOutput {
    pub fn new() -> Self {
        Self { playing: false }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }
}

impl Default for AudioOutput {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_audio_output_init() {
        let audio = AudioOutput::new();
        assert_eq!(audio.is_playing(), false);
    }
}
