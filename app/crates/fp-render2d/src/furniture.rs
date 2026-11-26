//! Furniture footprints: rotated rectangles with an outline and facing tick.

use fp_core::{EntityId, Pt};
use fp_model::{Furniture, catalog::hex_color};

use crate::batch::{Batch, Color, QuadVertex};
use crate::camera::Camera2D;

/// World-space corners of the footprint, counter-clockwise.
pub fn corners(f: &Furniture) -> [Pt; 4] {
    let (sin, cos) = f.rotation_deg.to_radians().sin_cos();
    let hw = f.width_mm as f64 / 2.0;
    let hd = f.depth_mm as f64 / 2.0;
    [(-hw, -hd), (hw, -hd), (hw, hd), (-hw, hd)].map(|(x, y)| {
        Pt::new(
            (f.center.x as f64 + x * cos - y * sin).round() as i32,
            (f.center.y as f64 + x * sin + y * cos).round() as i32,
        )
    })
}

/// Point-in-rotated-rectangle test.
pub fn hit(f: &Furniture, p: Pt) -> bool {
    let (sin, cos) = f.rotation_deg.to_radians().sin_cos();
    let dx = (p.x - f.center.x) as f64;
    let dy = (p.y - f.center.y) as f64;
    let lx = dx * cos + dy * sin;
    let ly = -dx * sin + dy * cos;
    lx.abs() <= f.width_mm as f64 / 2.0 && ly.abs() <= f.depth_mm as f64 / 2.0
}

pub fn build_furniture_batch(items: &[Furniture], cam: &Camera2D, selected: &[EntityId]) -> Batch {
    let mut batch = Batch::default();
    for f in items {
        let c = corners(f);
        let rgb = hex_color(&f.color);
        let base = Color([rgb[0], rgb[1], rgb[2], 1.0]);
        let is_selected = selected.contains(&f.id);
        let fill = if is_selected {
            base.with_alpha(0.92)
        } else {
            base.with_alpha(0.62)
        };
        let screen: Vec<[f32; 2]> = c.iter().map(|q| cam.world_to_screen(*q)).collect();
        for tri in [[0usize, 1, 2], [0, 2, 3]] {
            for i in tri {
                batch.vertices.push(QuadVertex {
                    pos: screen[i],
                    color: fill.0,
                });
            }
        }
        let outline = if is_selected {
            Color::SELECTED
        } else {
            Color([0.22, 0.22, 0.26, 0.85])
        };
        for i in 0..4 {
            batch.line(cam, c[i], c[(i + 1) % 4], 1.5, outline);
        }
        let front = Pt::new((c[2].x + c[3].x) / 2, (c[2].y + c[3].y) / 2);
        batch.line(cam, f.center, front, 1.5, outline);
    }
    batch
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sofa() -> Furniture {
        Furniture {
            id: EntityId(9),
            asset_id: "sofa".into(),
            name: "沙发".into(),
            center: Pt::new(1000, 2000),
            width_mm: 2200,
            depth_mm: 900,
            rotation_deg: 0.0,
            color: "#8b6f55".into(),
            room_id: None,
            price: None,
            floor: 0,
        }
    }

    #[test]
    fn corners_respect_rotation() {
        let a = corners(&sofa());
        assert_eq!(a[0].x, 1000 - 1100);
        assert_eq!(a[0].y, 2000 - 450);
        let mut r = sofa();
        r.rotation_deg = 90.0;
        let b = corners(&r);
        assert_eq!(b[0].x, 1000 + 450);
        assert_eq!(b[0].y, 2000 - 1100);
    }

    #[test]
    fn hit_inside_and_outside() {
        let f = sofa();
        assert!(hit(&f, Pt::new(1000, 2000)));
        assert!(!hit(&f, Pt::new(1000, 2600)));
        let mut r = f.clone();
        r.rotation_deg = 90.0;
        assert!(hit(&r, Pt::new(1000, 2600)), "depth axis becomes length");
        assert!(
            !hit(&r, Pt::new(2200, 2000)),
            "1200mm > 2200/2 on the short side"
        );
    }

    #[test]
    fn each_item_is_two_triangles_plus_outline() {
        let cam = Camera2D::new([1000.0, 800.0]);
        let batch = build_furniture_batch(&[sofa()], &cam, &[]);
        assert!(batch.vertices.len() >= 6);
    }
}
