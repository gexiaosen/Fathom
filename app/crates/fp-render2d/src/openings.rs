//! 2D plan symbols for doors and windows.

use fp_core::{EntityId, Pt};
use fp_model::{Opening, OpeningKind, Wall};

use crate::batch::Color;
use crate::{Batch, Camera2D};

pub const DOOR_COLOR: Color = Color([0.82, 0.52, 0.12, 1.0]);
pub const WINDOW_COLOR: Color = Color([0.22, 0.52, 0.90, 1.0]);
const SYMBOL_WIDTH_PX: f32 = 2.4;

/// Door leaf + swing arc, or a double glazing line for windows.
pub fn build_opening_batch(
    walls: &[Wall],
    openings: &[Opening],
    cam: &Camera2D,
    selected: &[EntityId],
) -> Batch {
    let mut batch = Batch::default();
    for opening in openings {
        let Some(wall) = walls.iter().find(|w| w.id == opening.wall_id) else {
            continue;
        };
        let len = ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
        if len < 1.0 {
            continue;
        }
        let t0 = (opening.offset_mm as f64 / len).clamp(0.0, 1.0) as f32;
        let t1 = ((opening.offset_mm + opening.width_mm) as f64 / len).clamp(0.0, 1.0) as f32;
        if t1 <= t0 {
            continue;
        }
        let a = lerp(wall.start, wall.end, t0);
        let b = lerp(wall.start, wall.end, t1);
        let dir = [((b.x - a.x) as f32), ((b.y - a.y) as f32)];
        let dl = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
        if dl < 1.0 {
            continue;
        }
        let unit = [dir[0] / dl, dir[1] / dl];
        let normal = [-unit[1], unit[0]];
        let color = if selected.contains(&opening.id) {
            Color::SELECTED
        } else {
            match opening.kind {
                OpeningKind::Door | OpeningKind::Sliding => DOOR_COLOR,
                OpeningKind::Window | OpeningKind::Bay => WINDOW_COLOR,
            }
        };

        match opening.kind {
            OpeningKind::Door | OpeningKind::Sliding => {
                // Hinge at one jamb; leaf opens perpendicular to the wall.
                let leaf_len = opening.width_mm as f32;
                let side = if opening.swing_deg >= 0.0 { 1.0 } else { -1.0 };
                let hinge = if opening.hinge_right { b } else { a };
                let jamb = if opening.hinge_right { a } else { b };
                let hinge_dir = [normal[0] * side, normal[1] * side];
                let leaf_end = Pt::new(
                    hinge.x + (hinge_dir[0] * leaf_len) as i32,
                    hinge.y + (hinge_dir[1] * leaf_len) as i32,
                );
                batch.line(cam, hinge, leaf_end, SYMBOL_WIDTH_PX, color);

                // Swing arc from the leaf back to the other jamb.
                let start_angle = hinge_dir[1].atan2(hinge_dir[0]);
                let end_angle = ((jamb.y - hinge.y) as f32).atan2((jamb.x - hinge.x) as f32);
                let mut delta = end_angle - start_angle;
                while delta > std::f32::consts::PI {
                    delta -= std::f32::consts::TAU;
                }
                while delta < -std::f32::consts::PI {
                    delta += std::f32::consts::TAU;
                }
                let steps = 10;
                let mut prev = leaf_end;
                for i in 1..=steps {
                    let angle = start_angle + delta * (i as f32 / steps as f32);
                    let p = Pt::new(
                        hinge.x + (angle.cos() * leaf_len) as i32,
                        hinge.y + (angle.sin() * leaf_len) as i32,
                    );
                    batch.line(cam, prev, p, SYMBOL_WIDTH_PX * 0.6, color);
                    prev = p;
                }
            }
            OpeningKind::Window | OpeningKind::Bay => {
                let offset = wall.thickness_mm as f32 * 0.28;
                let a1 = Pt::new(
                    a.x + (normal[0] * offset) as i32,
                    a.y + (normal[1] * offset) as i32,
                );
                let b1 = Pt::new(
                    b.x + (normal[0] * offset) as i32,
                    b.y + (normal[1] * offset) as i32,
                );
                let a2 = Pt::new(
                    a.x - (normal[0] * offset) as i32,
                    a.y - (normal[1] * offset) as i32,
                );
                let b2 = Pt::new(
                    b.x - (normal[0] * offset) as i32,
                    b.y - (normal[1] * offset) as i32,
                );
                batch.line(cam, a1, b1, SYMBOL_WIDTH_PX, color);
                batch.line(cam, a2, b2, SYMBOL_WIDTH_PX, color);
            }
        }
    }
    batch
}

fn lerp(a: Pt, b: Pt, t: f32) -> Pt {
    Pt::new(
        a.x + ((b.x - a.x) as f32 * t) as i32,
        a.y + ((b.y - a.y) as f32 * t) as i32,
    )
}

#[cfg(test)]
mod tests {
    use fp_core::{EntityId, Pt};
    use fp_model::{OpeningKind, WallKind};

    use super::*;

    fn wall() -> Wall {
        Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            200,
            WallKind::Partition,
        )
    }

    fn opening(id: u128, kind: OpeningKind, offset: i32, width: i32) -> Opening {
        Opening {
            id: EntityId(id),
            wall_id: EntityId(1),
            kind,
            offset_mm: offset,
            width_mm: width,
            height_mm: if kind == OpeningKind::Door {
                2100
            } else {
                1500
            },
            sill_mm: if kind == OpeningKind::Door { 0 } else { 900 },
            swing_deg: 90.0,
            hinge_right: false,
            price: None,
        }
    }

    #[test]
    fn door_draws_leaf_and_arc() {
        let cam = Camera2D::new([1000.0, 800.0]);
        let b = build_opening_batch(
            &[wall()],
            &[opening(7, OpeningKind::Door, 500, 900)],
            &cam,
            &[],
        );
        assert!(
            b.triangle_count() >= 11,
            "leaf + arc: {}",
            b.triangle_count()
        );
    }

    #[test]
    fn window_draws_two_lines() {
        let cam = Camera2D::new([1000.0, 800.0]);
        let b = build_opening_batch(
            &[wall()],
            &[opening(8, OpeningKind::Window, 1000, 1200)],
            &cam,
            &[],
        );
        assert_eq!(b.triangle_count(), 4, "two quads");
        assert_eq!(b.vertices[0].color, WINDOW_COLOR.0);
    }

    #[test]
    fn opening_on_missing_wall_is_skipped() {
        let cam = Camera2D::new([1000.0, 800.0]);
        let mut o = opening(9, OpeningKind::Door, 100, 800);
        o.wall_id = EntityId(99);
        let b = build_opening_batch(&[wall()], &[o], &cam, &[]);
        assert!(b.is_empty());
    }
}
