//! Screen-space vertex batches for the plan view.

use fp_core::{EntityId, Pt};
use fp_model::catalog::{material_color, material_kind_color};
use fp_model::{Material, Wall, WallFinish, WallKind};

use crate::camera::Camera2D;

/// Walls never render thinner than this on screen, so a 60 mm partition stays
/// visible at overview zoom.
pub const MIN_WALL_PX: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub [f32; 4]);

impl Color {
    pub const PAPER: Self = Self([0.97, 0.96, 0.94, 1.0]);
    pub const GRID_MINOR: Self = Self([0.88, 0.87, 0.85, 1.0]);
    pub const GRID_MAJOR: Self = Self([0.78, 0.77, 0.75, 1.0]);
    pub const SELECTED: Self = Self([1.0, 0.58, 0.12, 1.0]);
    pub const HOVER: Self = Self([0.18, 0.52, 0.96, 1.0]);

    pub fn with_alpha(self, alpha: f32) -> Self {
        Self([self.0[0], self.0[1], self.0[2], alpha])
    }
}

/// Wall fill by structural kind, echoing the Web plan's palette.
pub fn wall_color(kind: WallKind) -> Color {
    match kind {
        WallKind::Bearing => Color([0.30, 0.30, 0.33, 1.0]),
        WallKind::Exterior => Color([0.42, 0.42, 0.45, 1.0]),
        WallKind::Partition => Color([0.62, 0.61, 0.60, 1.0]),
        WallKind::Low => Color([0.52, 0.60, 0.68, 1.0]),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct QuadVertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub vertices: Vec<QuadVertex>,
}

impl Batch {
    pub fn triangle_count(&self) -> usize {
        self.vertices.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Append one axis-aligned quad as two triangles.
    /// Draw a world-space segment as a screen-space quad of `width_px`.
    pub fn line(&mut self, cam: &Camera2D, a: Pt, b: Pt, width_px: f32, color: Color) {
        let sa = cam.world_to_screen(a);
        let sb = cam.world_to_screen(b);
        let d = [sb[0] - sa[0], sb[1] - sa[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if len < 1e-3 {
            return;
        }
        let n = [-d[1] / len * width_px * 0.5, d[0] / len * width_px * 0.5];
        self.push_quad(
            [
                [sa[0] + n[0], sa[1] + n[1]],
                [sb[0] + n[0], sb[1] + n[1]],
                [sb[0] - n[0], sb[1] - n[1]],
                [sa[0] - n[0], sa[1] - n[1]],
            ],
            color,
        );
    }

    pub fn rect_outline(&mut self, cam: &Camera2D, rect: [f32; 4], width_px: f32, color: Color) {
        let [x0, y0, x1, y1] = rect;
        let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        for i in 0..4 {
            let a = corners[i];
            let b = corners[(i + 1) % 4];
            let sa = cam.screen_to_world(a);
            let sb = cam.screen_to_world(b);
            self.line(cam, sa, sb, width_px, color);
        }
    }

    pub fn push_quad(&mut self, corners: [[f32; 2]; 4], color: Color) {
        let v = |p: [f32; 2]| QuadVertex {
            pos: p,
            color: color.0,
        };
        self.vertices.push(v(corners[0]));
        self.vertices.push(v(corners[1]));
        self.vertices.push(v(corners[2]));
        self.vertices.push(v(corners[0]));
        self.vertices.push(v(corners[2]));
        self.vertices.push(v(corners[3]));
    }
}

/// Plan walls tinted by the finish record each wall points at. Used by the
/// construction sheets so a wall's colour is the same finish shown in A-03.
pub fn build_wall_finish_batch(
    walls: &[Wall],
    materials: &[Material],
    cam: &Camera2D,
    show_bearing: bool,
) -> Batch {
    let mut batch = Batch {
        vertices: Vec::with_capacity(walls.len() * 6),
    };
    for wall in walls {
        let a = cam.world_to_screen(wall.start);
        let b = cam.world_to_screen(wall.end);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if len < 1e-3 {
            continue;
        }
        let n = [-d[1] / len, d[0] / len];
        let half = ((wall.thickness_mm as f32 * cam.scale as f32) * 0.5).max(MIN_WALL_PX * 0.5);
        let rgb = wall
            .finish_id
            .and_then(|id| materials.iter().find(|m| m.id == id))
            .map(material_color)
            .unwrap_or_else(|| match wall.finish {
                WallFinish::Paint => material_kind_color("paint"),
                WallFinish::Wallpaper => material_kind_color("wallpaper"),
                WallFinish::WoodPanel => material_kind_color("wood_panel"),
                WallFinish::Tile => material_kind_color("tile"),
            });
        let color = if !show_bearing && wall.kind == WallKind::Bearing {
            wall_color(WallKind::Partition)
        } else {
            Color([rgb[0], rgb[1], rgb[2], 0.84])
        };
        batch.push_quad(
            [
                [a[0] + n[0] * half, a[1] + n[1] * half],
                [b[0] + n[0] * half, b[1] + n[1] * half],
                [b[0] - n[0] * half, b[1] - n[1] * half],
                [a[0] - n[0] * half, a[1] - n[1] * half],
            ],
            color,
        );
        // Keep the wall readable as a plan element: the finish tint alone can
        // be too close to the paper or floor colours.
        let off = (half / (cam.scale as f32).max(1e-6)).round() as i32;
        for sign in [1.0f32, -1.0] {
            let a_edge = Pt::new(
                wall.start.x + (n[0] * off as f32 * sign) as i32,
                wall.start.y + (n[1] * off as f32 * sign) as i32,
            );
            let b_edge = Pt::new(
                wall.end.x + (n[0] * off as f32 * sign) as i32,
                wall.end.y + (n[1] * off as f32 * sign) as i32,
            );
            batch.line(cam, a_edge, b_edge, 1.0, Color([0.24, 0.23, 0.21, 0.9]));
        }
    }
    batch
}

/// Turn wall centrelines into screen-space quads.
pub fn build_wall_batch(walls: &[Wall], selected: &[EntityId], cam: &Camera2D) -> Batch {
    build_wall_batch_visible(walls, selected, cam, true)
}

/// Same as [`build_wall_batch`], but structural walls can be drawn in the
/// neutral partition colour when the bearing-wall layer is switched off.
pub fn build_wall_batch_visible(
    walls: &[Wall],
    selected: &[EntityId],
    cam: &Camera2D,
    show_bearing: bool,
) -> Batch {
    let mut batch = Batch {
        vertices: Vec::with_capacity(walls.len() * 6),
    };
    for wall in walls {
        let a = cam.world_to_screen(wall.start);
        let b = cam.world_to_screen(wall.end);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if len < 1e-3 {
            continue;
        }
        let n = [-d[1] / len, d[0] / len];
        let half = ((wall.thickness_mm as f32 * cam.scale as f32) * 0.5).max(MIN_WALL_PX * 0.5);
        let color = if selected.contains(&wall.id) {
            Color::SELECTED
        } else if !show_bearing && wall.kind == fp_model::WallKind::Bearing {
            wall_color(fp_model::WallKind::Partition)
        } else {
            wall_color(wall.kind)
        };
        batch.push_quad(
            [
                [a[0] + n[0] * half, a[1] + n[1] * half],
                [b[0] + n[0] * half, b[1] + n[1] * half],
                [b[0] - n[0] * half, b[1] - n[1] * half],
                [a[0] - n[0] * half, a[1] - n[1] * half],
            ],
            color,
        );
    }
    batch
}

#[cfg(test)]
mod tests {
    use fp_core::Pt;
    use fp_model::{Document, Wall};

    use super::*;

    fn wall(thickness: i32) -> Wall {
        Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(1000, 0),
            thickness,
            WallKind::Partition,
        )
    }

    fn area_of(batch: &Batch) -> f32 {
        let mut sum = 0.0;
        let (tris, _) = batch.vertices.as_chunks::<3>();
        for tri in tris {
            let (a, b, c) = (tri[0].pos, tri[1].pos, tri[2].pos);
            sum += ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) * 0.5;
        }
        sum.abs()
    }

    #[test]
    fn wall_quads_use_the_finish_record_colour() {
        let cam = Camera2D::new([1000.0, 800.0]);
        let materials = vec![
            Material {
                id: EntityId(201),
                name: "red".to_string(),
                price_per_m2: 0.0,
                kind: "paint".to_string(),
                color: "#ff0000".to_string(),
                surface: fp_model::FinishSurface::Wall,
                waste_rate: 0.0,
                built_in: true,
            },
            Material {
                id: EntityId(202),
                name: "green".to_string(),
                price_per_m2: 0.0,
                kind: "tile".to_string(),
                color: "#00ff00".to_string(),
                surface: fp_model::FinishSurface::Wall,
                waste_rate: 0.0,
                built_in: true,
            },
        ];
        let mut a = wall(120);
        a.finish_id = Some(EntityId(201));
        let mut b = wall(120);
        b.id = EntityId(2);
        b.start = Pt::new(0, 2000);
        b.end = Pt::new(1000, 2000);
        b.finish_id = Some(EntityId(202));
        let batch = build_wall_finish_batch(&[a, b], &materials, &cam, true);
        assert!(batch
            .vertices
            .iter()
            .any(|v| v.color[0] == 1.0 && v.color[1] == 0.0));
        assert!(batch
            .vertices
            .iter()
            .any(|v| v.color[1] == 1.0 && v.color[0] == 0.0));
    }

    #[test]
    fn one_wall_becomes_one_quad() {
        let mut cam = Camera2D::new([1000.0, 800.0]);
        cam.scale = 0.05; // 1 px = 20 mm
        let b = build_wall_batch(&[wall(100)], &[], &cam);
        assert_eq!(b.vertices.len(), 6);
        assert_eq!(b.triangle_count(), 2);
        // 1000 mm × 100 mm at 0.05 px/mm → 50 px × 5 px = 250 px²
        let area = area_of(&b);
        assert!((area - 250.0).abs() < 1.0, "area {area}");
    }

    #[test]
    fn thin_walls_stay_visible() {
        let mut cam = Camera2D::new([1000.0, 800.0]);
        cam.scale = 0.005; // 1 px = 200 mm
        let b = build_wall_batch(&[wall(60)], &[], &cam);
        let area = area_of(&b);
        let expected_len = 1000.0 * 0.005;
        assert!(area >= expected_len * MIN_WALL_PX, "area {area} too small");
    }

    #[test]
    fn selection_uses_the_highlight_colour() {
        let cam = Camera2D::new([1000.0, 800.0]);
        let b = build_wall_batch(&[wall(100)], &[EntityId(1)], &cam);
        assert_eq!(b.vertices[0].color, Color::SELECTED.0);
    }

    #[test]
    fn demo_plan_renders_in_one_batch() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("fixture");
        let mut cam = Camera2D::new([1600.0, 1000.0]);
        cam.fit(Pt::new(0, 0), Pt::new(12_000, 9_000), 40.0);
        let b = build_wall_batch(&doc.walls, &[], &cam);
        assert_eq!(b.vertices.len(), doc.walls.len() * 6);
        assert!(b.vertices.iter().all(|v| v.pos[0].is_finite()));
    }
}
