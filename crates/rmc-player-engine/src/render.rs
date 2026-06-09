use wgpu::{Instance, InstanceDescriptor, Backends};

pub struct VideoRenderer {
    adapter_name: Option<String>,
}

impl VideoRenderer {
    pub async fn new() -> Self {
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::all(),
            ..Default::default()
        });
        
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        }).await;
        
        Self {
            adapter_name: adapter.map(|a| a.get_info().name),
        }
    }

    pub fn has_adapter(&self) -> bool {
        self.adapter_name.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_wgpu_instance_creation() {
        let renderer = VideoRenderer::new().await;
        // 期望能够成功获取到 wgpu 的适配器 (Adapter)
        assert!(renderer.has_adapter());
    }
}
