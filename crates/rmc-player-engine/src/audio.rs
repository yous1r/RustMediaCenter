use cpal::traits::{DeviceTrait, HostTrait};

pub struct AudioOutput {
    playing: bool,
    device_name: Option<String>,
}

impl AudioOutput {
    pub fn new() -> Self {
        let host = cpal::default_host();
        let device_name = host.default_output_device().and_then(|d| d.name().ok());
        Self {
            playing: false,
            device_name,
        }
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn has_device(&self) -> bool {
        self.device_name.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_audio_output_init() {
        let audio = AudioOutput::new();
        assert_eq!(audio.playing, false);
    }

    #[test]
    fn test_audio_device_init() {
        let output = AudioOutput::new();
        // 我们期望底层能成功找到系统的默认音频宿设备
        assert!(output.has_device());
    }
}

