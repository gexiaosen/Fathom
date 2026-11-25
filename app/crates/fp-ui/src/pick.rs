//! Pointer picking in screen space.

use fp_core::EntityId;
use fp_model::Wall;
use fp_render2d::Camera2D;

/// How far (in pixels) a click may land from a wall centreline.
pub const PICK_TOLERANCE_PX: f32 = 6.0;

pub fn distance_point_segment_px(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len_sq = d[0] * d[0] + d[1] * d[1];
    if len_sq <= 1e-9 {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len_sq).clamp(0.0, 1.0);
    let q = [a[0] + d[0] * t, a[1] + d[1] * t];
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
}

/// Nearest wall under the pointer, if any is within tolerance.
pub fn hit_wall(
    walls: &[Wall],
    cam: &Camera2D,
    pointer: [f32; 2],
    tolerance_px: f32,
) -> Option<EntityId> {
    let mut best: Option<(f32, EntityId)> = None;
    for wall in walls {
        let a = cam.world_to_screen(wall.start);
        let b = cam.world_to_screen(wall.end);
        let d = distance_point_segment_px(pointer, a, b);
        // A wall is a thick band, not a hairline. Include half its drawn
        // thickness so clicking the filled body selects it too, not only the
        // invisible centreline.
        let half_thickness_px = (wall.thickness_mm.max(0) as f64 * cam.scale * 0.5) as f32;
        let hit = d <= tolerance_px + half_thickness_px;
        if hit && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, wall.id));
        }
    }
    best.map(|(_, id)| id)
}

/// Walls whose centreline touches the screen rectangle `[min_x, min_y, max_x, max_y]`.
pub fn walls_in_rect(walls: &[Wall], cam: &Camera2D, rect: [f32; 4]) -> Vec<EntityId> {
    let [min_x, min_y, max_x, max_y] = rect;
    walls
        .iter()
        .filter(|wall| {
            let a = cam.world_to_screen(wall.start);
            let b = cam.world_to_screen(wall.end);
            point_in_rect(a, min_x, min_y, max_x, max_y)
                || point_in_rect(b, min_x, min_y, max_x, max_y)
                || segment_in_rect(a, b, min_x, min_y, max_x, max_y)
        })
        .map(|wall| wall.id)
        .collect()
}

fn point_in_rect(p: [f32; 2], min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> bool {
    p[0] >= min_x && p[0] <= max_x && p[1] >= min_y && p[1] <= max_y
}

fn segment_in_rect(
    a: [f32; 2],
    b: [f32; 2],
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) -> bool {
    let corners = [
        ([min_x, min_y], [max_x, min_y]),
        ([max_x, min_y], [max_x, max_y]),
        ([max_x, max_y], [min_x, max_y]),
        ([min_x, max_y], [min_x, min_y]),
    ];
    corners.iter().any(|(c, d)| segments_cross(a, b, *c, *d))
}

fn segments_cross(p1: [f32; 2], p2: [f32; 2], q1: [f32; 2], q2: [f32; 2]) -> bool {
    let orient = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let o1 = orient(p1, p2, q1);
    let o2 = orient(p1, p2, q2);
    let o3 = orient(q1, q2, p1);
    let o4 = orient(q1, q2, p2);
    o1 * o2 < 0.0 && o3 * o4 < 0.0
}

#[cfg(test)]
mod tests {
    use fp_core::Pt;
    use fp_model::{Wall, WallKind};

    use super::*;

    fn wall(id: u128, x0: i32, y0: i32, x1: i32, y1: i32) -> Wall {
        Wall::new(
            EntityId(id),
            Pt::new(x0, y0),
            Pt::new(x1, y1),
            100,
            WallKind::Partition,
        )
    }

    fn cam() -> Camera2D {
        let mut c = Camera2D::new([1000.0, 800.0]);
        c.center = [5000.0, 4000.0];
        c.scale = 0.05;
        c
    }

    #[test]
    fn click_on_wall_hits_it() {
        let c = cam();
        let walls = vec![wall(1, 4000, 4000, 6000, 4000)];
        let mid = c.world_to_screen(Pt::new(5000, 4000));
        assert_eq!(
            hit_wall(&walls, &c, mid, PICK_TOLERANCE_PX),
            Some(EntityId(1))
        );
    }

    #[test]
    fn click_far_away_hits_nothing() {
        let c = cam();
        let walls = vec![wall(1, 4000, 4000, 6000, 4000)];
        let far = c.world_to_screen(Pt::new(5000, 6000));
        assert_eq!(hit_wall(&walls, &c, far, PICK_TOLERANCE_PX), None);
    }

    #[test]
    fn click_on_wall_body_not_only_centreline_hits_it() {
        let c = cam();
        let thick = Wall::new(
            EntityId(7),
            Pt::new(4000, 4000),
            Pt::new(6000, 4000),
            400,
            WallKind::Partition,
        );
        // 400 mm at 0.05 px/mm is a 20 px band (half is 10 px). Clicking
        // 9 px off the centre line is still inside the visible wall, but
        // outside a fixed 6 px centreline tolerance.
        let body = c.world_to_screen(Pt::new(5000, 4180));
        assert_eq!(
            hit_wall(&[thick], &c, body, PICK_TOLERANCE_PX),
            Some(EntityId(7))
        );
    }

    #[test]
    fn nearest_wall_wins() {
        let c = cam();
        let walls = vec![
            wall(1, 4000, 4100, 6000, 4100),
            wall(2, 4000, 4000, 6000, 4000),
        ];
        let p = c.world_to_screen(Pt::new(5000, 4000));
        assert_eq!(
            hit_wall(&walls, &c, p, PICK_TOLERANCE_PX),
            Some(EntityId(2))
        );
    }

    #[test]
    fn rect_catches_crossing_walls() {
        let c = cam();
        let walls = vec![
            wall(1, 4000, 4000, 6000, 4000), // inside
            wall(2, 3000, 4000, 8000, 4000), // crosses the box
            wall(3, 4000, 7000, 6000, 7000), // outside
        ];
        let a = c.world_to_screen(Pt::new(4500, 3500));
        let b = c.world_to_screen(Pt::new(5500, 4500));
        let rect = [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[0].max(b[0]),
            a[1].max(b[1]),
        ];
        let picked = walls_in_rect(&walls, &c, rect);
        assert_eq!(picked.len(), 2);
        assert!(picked.contains(&EntityId(1)) && picked.contains(&EntityId(2)));
    }
}
