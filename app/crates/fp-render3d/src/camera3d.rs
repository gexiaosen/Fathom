//! Orbit camera for the 3D plan view.

use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    /// Rotation around the up axis, radians.
    pub yaw: f32,
    /// Elevation above the plan, radians (clamped to avoid the poles).
    pub pitch: f32,
    pub fov_y: f32,
}

pub const MIN_DISTANCE: f32 = 2.0;
pub const MAX_DISTANCE: f32 = 80.0;

impl OrbitCamera {
    pub fn new(target: Vec3) -> Self {
        Self {
            target,
            distance: 14.0,
            yaw: 0.55,
            pitch: 0.62,
            fov_y: 45f32.to_radians(),
        }
    }

    pub fn eye(&self) -> Vec3 {
        let cp = self.pitch.cos();
        self.target
            + Vec3::new(
                self.distance * cp * self.yaw.sin(),
                self.distance * self.pitch.sin(),
                self.distance * cp * self.yaw.cos(),
            )
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye(), self.target, Vec3::Y);
        let proj = Mat4::perspective_rh(self.fov_y, aspect.max(0.1), 0.08, 1500.0);
        proj * view
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.006).clamp(-1.35, 1.35);
    }

    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance * (delta * 0.0015).exp()).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    pub fn fit(&mut self, min: Vec3, max: Vec3) {
        self.target = (min + max) * 0.5;
        let size = (max - min).length();
        self.distance = (size * 1.25).clamp(4.0, MAX_DISTANCE);
    }
}

/// First-person camera: the eye sits at `pos`, `yaw` turns around +Y (0 faces +Z).
#[derive(Clone, Copy, Debug)]
pub struct WalkCamera {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
}

pub const EYE_HEIGHT: f32 = 1.62;

impl WalkCamera {
    pub fn new(pos: Vec3, yaw: f32) -> Self {
        Self {
            pos,
            yaw,
            pitch: 0.0,
            fov_y: 68f32.to_radians(),
        }
    }

    /// Unit vector the camera looks along.
    pub fn dir(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(self.yaw.sin() * cp, self.pitch.sin(), self.yaw.cos() * cp)
    }

    /// Planar forward and right vectors, used for WASD movement. Looking down
    /// +Z with +Y up puts screen-right at -X, so `right` is forward × up.
    pub fn basis(&self) -> (Vec3, Vec3) {
        let forward = Vec3::new(self.yaw.sin(), 0.0, self.yaw.cos());
        let right = Vec3::new(-self.yaw.cos(), 0.0, self.yaw.sin());
        (forward, right)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.pos, self.pos + self.dir(), Vec3::Y);
        let proj = Mat4::perspective_rh(self.fov_y, aspect.max(0.1), 0.08, 1500.0);
        proj * view
    }

    /// Mouse look, matching the web and touch build: dragging grabs the scene
    /// and pulls it, so dragging right turns the view left and dragging down
    /// looks up. This is deliberately the inverse of a first-person shooter.
    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.0035;
        self.pitch = (self.pitch + dy * 0.003).clamp(-1.35, 1.35);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> OrbitCamera {
        OrbitCamera::new(Vec3::new(5.0, 0.0, 4.0))
    }

    #[test]
    fn eye_sits_on_the_orbit_sphere() {
        let c = cam();
        let d = (c.eye() - c.target).length();
        assert!((d - c.distance).abs() < 1e-4);
    }

    #[test]
    fn target_projects_to_the_centre() {
        let c = cam();
        let vp = c.view_proj(16.0 / 9.0);
        let clip = vp * c.target.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1e-4, "{ndc:?}");
        assert!(ndc.y.abs() < 1e-4, "{ndc:?}");
        assert!(ndc.z > 0.0 && ndc.z < 1.0, "{ndc:?}");
    }

    #[test]
    fn orbit_clamps_the_pitch() {
        let mut c = cam();
        for _ in 0..200 {
            c.orbit(0.0, 10.0);
        }
        assert!((c.pitch - 1.35).abs() < 1e-6);
        for _ in 0..400 {
            c.orbit(0.0, -10.0);
        }
        assert!((c.pitch + 1.35).abs() < 1e-6);
    }

    #[test]
    fn zoom_stays_within_limits() {
        let mut c = cam();
        for _ in 0..100 {
            c.zoom(-100.0);
        }
        assert!((c.distance - MIN_DISTANCE).abs() < 1e-4);
        for _ in 0..100 {
            c.zoom(100.0);
        }
        assert!((c.distance - MAX_DISTANCE).abs() < 1e-4);
    }

    #[test]
    fn fit_frames_the_bounds() {
        let mut c = cam();
        c.fit(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 2.8, 8.0));
        assert!((c.target - Vec3::new(5.0, 1.4, 4.0)).length() < 1e-5);
        assert!(c.distance > 8.0 && c.distance < MAX_DISTANCE);
    }
    #[test]
    fn walk_camera_starts_at_eye_height() {
        let c = WalkCamera::new(Vec3::new(1.0, EYE_HEIGHT, 2.0), 0.0);
        assert!((c.pos.y - EYE_HEIGHT).abs() < 1e-6);
    }

    #[test]
    fn walk_forward_follows_yaw() {
        let c = WalkCamera::new(Vec3::ZERO, 0.0);
        assert!((c.dir() - Vec3::Z).length() < 1e-5);
        let (f, r) = c.basis();
        assert!((f - Vec3::Z).length() < 1e-5);
        assert!((r + Vec3::X).length() < 1e-5);
    }

    #[test]
    fn walk_drag_pulls_the_scene_instead_of_turning_like_an_fps() {
        let mut c = WalkCamera::new(Vec3::ZERO, 0.0);
        c.look(120.0, 0.0);
        assert!(c.dir().x > 0.05, "dragging right looks left");
        c.look(0.0, 300.0);
        assert!(c.pitch > 0.0, "dragging down looks up");
    }

    #[test]
    fn walk_projection_keeps_the_eye_in_front() {
        let c = WalkCamera::new(Vec3::new(0.0, EYE_HEIGHT, 0.0), 0.0);
        let vp = c.view_proj(1.6);
        assert!(vp.is_finite());
        let near = vp * Vec3::new(0.0, EYE_HEIGHT, 1.0).extend(1.0);
        assert!(near.w > 0.0);
    }
}
