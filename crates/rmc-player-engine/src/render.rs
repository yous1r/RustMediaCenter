pub struct VideoRenderer;

impl VideoRenderer {
    pub fn new() -> Self {
        Self
    }
    
    pub fn render_frame(&self) -> bool {
        true // Dummy return
    }
}

impl Default for VideoRenderer {
    fn default() -> Self {
        Self::new()
    }
}
