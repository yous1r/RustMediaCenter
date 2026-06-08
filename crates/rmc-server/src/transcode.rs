pub struct TranscodeTask {
    input: String,
    output: String,
    status: String,
}

impl TranscodeTask {
    pub fn new(input: &str, output: &str) -> Self {
        Self {
            input: input.to_string(),
            output: output.to_string(),
            status: "Pending".to_string(),
        }
    }

    pub fn status(&self) -> &str {
        &self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transcoder_status() {
        let task = TranscodeTask::new("movie.mkv", "out.m3u8");
        assert_eq!(task.status(), "Pending");
    }
}
