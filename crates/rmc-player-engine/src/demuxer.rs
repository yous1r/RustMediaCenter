use ffmpeg_next::format::context::Input;

pub struct Demuxer {
    file_path: String,
    // 实际持有 ffmpeg 的输入上下文
    input_ctx: Option<Input>,
}

impl Demuxer {
    pub fn new(file_path: String) -> Result<Self, ffmpeg_next::Error> {
        ffmpeg_next::init()?;
        // 尝试打开文件
        let input_ctx = ffmpeg_next::format::input(&file_path)?;
        Ok(Self {
            file_path,
            input_ctx: Some(input_ctx),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_demuxer_open_file() {
        // 由于 ffmpeg 需要初始化环境
        ffmpeg_next::init().unwrap();
        // 如果文件不存在，解封装器应该返回清晰的 ffmpeg 错误，而不是我们 stub 的 "ok"
        let res = Demuxer::new("dummy_non_existent_file.mp4".to_string());
        assert!(res.is_err());
    }
}
