//! Imported floor-plan raster used as a tracing underlay.

use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub struct UnderlayImage {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8 pixels, top row first.
    pub rgba: Arc<Vec<u8>>,
}

impl UnderlayImage {
    pub fn load(path: &Path) -> Result<Self, String> {
        let image = image::open(path)
            .map_err(|e| format!("读取图片失败: {e}"))?
            .to_rgba8();
        let (width, height) = image.dimensions();
        Ok(Self {
            width,
            height,
            rgba: Arc::new(image.into_raw()),
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.rgba
    }
}

#[derive(Clone)]
pub struct UnderlayDraw {
    pub image: Arc<UnderlayImage>,
    /// Screen-space rectangle: x0, y0, x1, y1.
    pub rect: [f32; 4],
    pub opacity: f32,
}
