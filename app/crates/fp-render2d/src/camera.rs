//! Plan camera: millimetres in, pixels out.

use fp_core::Pt;

/// Minimum / maximum zoom, expressed as pixels per millimetre.
pub const MIN_SCALE: f64 = 0.005; // 1 px = 200 mm
pub const MAX_SCALE: f64 = 2.0; // 1 px = 0.5 mm

#[derive(Clone, Copy, Debug)]
pub struct Camera2D {
    /// World point shown at the centre of the viewport, in millimetres.
    pub center: [f64; 2],
    /// Pixels per millimetre.
    pub scale: f64,
    /// Viewport size in pixels.
    pub viewport: [f32; 2],
}

impl Camera2D {
    pub fn new(viewport: [f32; 2]) -> Self {
        Self {
            center: [0.0, 0.0],
            scale: 0.05,
            viewport,
        }
    }

    pub fn resize(&mut self, viewport: [f32; 2]) {
        self.viewport = viewport;
    }

    pub fn world_to_screen(&self, p: Pt) -> [f32; 2] {
        let w = self.viewport[0] as f64;
        let h = self.viewport[1] as f64;
        [
            ((p.x as f64 - self.center[0]) * self.scale + w * 0.5) as f32,
            (h * 0.5 - (p.y as f64 - self.center[1]) * self.scale) as f32,
        ]
    }

    pub fn screen_to_world(&self, s: [f32; 2]) -> Pt {
        let w = self.screen_to_world_f64(s);
        Pt::new(w[0].round() as i32, w[1].round() as i32)
    }

    /// Unrounded inverse mapping, used by zoom so the anchor never drifts.
    pub fn screen_to_world_f64(&self, s: [f32; 2]) -> [f64; 2] {
        let w = self.viewport[0] as f64;
        let h = self.viewport[1] as f64;
        [
            self.center[0] + (s[0] as f64 - w * 0.5) / self.scale,
            self.center[1] + (h * 0.5 - s[1] as f64) / self.scale,
        ]
    }

    /// Axis-aligned world bounds currently visible, as `[min_x, min_y, max_x, max_y]`.
    pub fn world_bounds(&self) -> [f64; 4] {
        let half_w = self.viewport[0] as f64 * 0.5 / self.scale;
        let half_h = self.viewport[1] as f64 * 0.5 / self.scale;
        [
            self.center[0] - half_w,
            self.center[1] - half_h,
            self.center[0] + half_w,
            self.center[1] + half_h,
        ]
    }

    /// Zoom around a screen anchor (usually the cursor); the world point under
    /// the anchor stays put.
    pub fn zoom_at(&mut self, anchor: [f32; 2], factor: f64) {
        let before = self.screen_to_world_f64(anchor);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let after = self.screen_to_world_f64(anchor);
        self.center[0] += before[0] - after[0];
        self.center[1] += before[1] - after[1];
    }

    /// Drag the sheet with the pointer: content follows the mouse.
    pub fn pan_px(&mut self, dx: f32, dy: f32) {
        self.center[0] -= dx as f64 / self.scale;
        self.center[1] += dy as f64 / self.scale;
    }

    /// Frame a world box inside the viewport with a pixel margin.
    pub fn fit(&mut self, min: Pt, max: Pt, padding_px: f32) {
        let w = (max.x - min.x).max(1) as f64;
        let h = (max.y - min.y).max(1) as f64;
        let avail_w = (self.viewport[0] - 2.0 * padding_px).max(64.0) as f64;
        let avail_h = (self.viewport[1] - 2.0 * padding_px).max(64.0) as f64;
        self.scale = (avail_w / w).min(avail_h / h).clamp(MIN_SCALE, MAX_SCALE);
        self.center = [
            (min.x as f64 + max.x as f64) * 0.5,
            (min.y as f64 + max.y as f64) * 0.5,
        ];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera2D {
        let mut c = Camera2D::new([1000.0, 800.0]);
        c.center = [5000.0, 3000.0];
        c
    }

    #[test]
    fn roundtrip_between_world_and_screen() {
        let c = cam();
        for p in [
            Pt::new(0, 0),
            Pt::new(5000, 3000),
            Pt::new(-1200, 8800),
            Pt::new(12_345, 67),
        ] {
            let s = c.world_to_screen(p);
            let back = c.screen_to_world(s);
            assert!((back.x - p.x).abs() <= 1, "{p:?} -> {s:?} -> {back:?}");
            assert!((back.y - p.y).abs() <= 1);
        }
    }

    #[test]
    fn centre_maps_to_viewport_centre() {
        let c = cam();
        let s = c.world_to_screen(Pt::new(5000, 3000));
        assert!((s[0] - 500.0).abs() < 0.01);
        assert!((s[1] - 400.0).abs() < 0.01);
    }

    #[test]
    fn zoom_keeps_the_anchor_fixed() {
        let mut c = cam();
        let anchor = [730.0, 120.0];
        let before = c.screen_to_world_f64(anchor);
        for factor in [1.3, 0.4, 2.5, 0.8] {
            c.zoom_at(anchor, factor);
            let after = c.screen_to_world_f64(anchor);
            assert!((after[0] - before[0]).abs() <= 0.01, "x drift at {factor}");
            assert!((after[1] - before[1]).abs() <= 0.01, "y drift at {factor}");
        }
    }

    #[test]
    fn zoom_is_clamped() {
        let mut c = cam();
        c.zoom_at([0.0, 0.0], 1e9);
        assert!((c.scale - MAX_SCALE).abs() < 1e-12);
        c.zoom_at([0.0, 0.0], 1e-9);
        assert!((c.scale - MIN_SCALE).abs() < 1e-12);
    }

    #[test]
    fn pan_follows_the_pointer() {
        let mut c = cam();
        let p = Pt::new(5000, 3000);
        let before = c.world_to_screen(p);
        c.pan_px(40.0, -25.0);
        let after = c.world_to_screen(p);
        assert!((after[0] - before[0] - 40.0).abs() < 0.01);
        assert!((after[1] - before[1] + 25.0).abs() < 0.01);
    }

    #[test]
    fn fit_frames_the_bounds() {
        let mut c = Camera2D::new([1200.0, 900.0]);
        c.fit(Pt::new(0, 0), Pt::new(10_000, 7_000), 40.0);
        let b = c.world_bounds();
        assert!(b[3] > 7_000.0 && b[2] > 10_000.0, "{b:?}");
        let centre = c.world_to_screen(Pt::new(5_000, 3_500));
        assert!((centre[0] - 600.0).abs() < 1.0);
        assert!((centre[1] - 450.0).abs() < 1.0);
    }
}
