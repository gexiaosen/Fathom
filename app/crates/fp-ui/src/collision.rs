//! Physical boundaries for furniture: a footprint may touch a wall, but it
//! may never pass through one.
//!
//! Walls are treated as thick rectangles and furniture as a rotated rectangle.
//! Both are convex quads, so a separating-axis test is enough. A small
//! tolerance lets a piece sit exactly flush with a wall surface without being
//! reported as a collision.

use fp_core::{EntityId, Mm, Pt};
use fp_model::{Furniture, Wall};
use fp_render2d::corners;

/// Penetration deeper than this counts as a collision. Exact flush contact is
/// allowed, which matters because wall snapping deliberately puts the piece
/// edge on the wall surface.
pub const WALL_CLEARANCE_MM: f64 = 2.5;

/// Sampling step used for swept moves. This is much smaller than any real
/// furniture depth, so a fast pointer can never tunnel through a wall.
const SWEEP_STEP_MM: f64 = 25.0;

/// Corners of a wall's physical footprint, in plan millimetres.
pub fn wall_corners(wall: &Wall) -> [Pt; 4] {
    let dx = (wall.end.x - wall.start.x) as f64;
    let dy = (wall.end.y - wall.start.y) as f64;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        let c = wall.start;
        return [c, c, c, c];
    }
    let nx = -dy / len;
    let ny = dx / len;
    let half = (wall.thickness_mm.max(1) as f64) * 0.5;
    let ox = (nx * half).round() as i32;
    let oy = (ny * half).round() as i32;
    [
        Pt::new(wall.start.x + ox, wall.start.y + oy),
        Pt::new(wall.end.x + ox, wall.end.y + oy),
        Pt::new(wall.end.x - ox, wall.end.y - oy),
        Pt::new(wall.start.x - ox, wall.start.y - oy),
    ]
}

fn projection(poly: &[Pt], axis: (f64, f64)) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for p in poly {
        let d = p.x as f64 * axis.0 + p.y as f64 * axis.1;
        min = min.min(d);
        max = max.max(d);
    }
    (min, max)
}

fn separating_axis(a: &[Pt], b: &[Pt], axis: (f64, f64), tolerance: f64) -> bool {
    let (amin, amax) = projection(a, axis);
    let (bmin, bmax) = projection(b, axis);
    // Touching or within the tolerance is not penetration.
    amax <= bmin + tolerance || bmax <= amin + tolerance
}

/// True when two convex quads overlap by more than `WALL_CLEARANCE_MM`.
pub fn quads_penetrate(a: &[Pt], b: &[Pt]) -> bool {
    let axes = |poly: &[Pt]| {
        let mut out = Vec::with_capacity(poly.len());
        for i in 0..poly.len() {
            let p = poly[i];
            let q = poly[(i + 1) % poly.len()];
            let dx = (q.x - p.x) as f64;
            let dy = (q.y - p.y) as f64;
            let len = (dx * dx + dy * dy).sqrt();
            if len > 1e-9 {
                out.push((-dy / len, dx / len));
            }
        }
        out
    };
    for axis in axes(a).into_iter().chain(axes(b)) {
        if separating_axis(a, b, axis, WALL_CLEARANCE_MM) {
            return false;
        }
    }
    true
}

/// True when this furniture footprint cuts into the wall's physical body.
pub fn furniture_hits_wall(item: &Furniture, wall: &Wall) -> bool {
    quads_penetrate(&corners(item), &wall_corners(wall))
}

/// First wall the footprint penetrates, if any.
pub fn furniture_blocking_wall(item: &Furniture, walls: &[Wall]) -> Option<EntityId> {
    walls
        .iter()
        .find(|wall| furniture_hits_wall(item, wall))
        .map(|wall| wall.id)
}

/// True when two furniture footprints overlap by more than the flush tolerance.
///
/// Pieces are allowed to sit flush against each other (2.5 mm grace), exactly
/// like they are allowed to sit flush against a wall, but one piece may never
/// be dropped on top of another.
pub fn furniture_hits_furniture(a: &Furniture, b: &Furniture) -> bool {
    quads_penetrate(&corners(a), &corners(b))
}

/// First other piece whose footprint overlaps `item`, skipping `ignore`
/// (the ids that move together with `item`).
pub fn furniture_blocking_other(
    item: &Furniture,
    others: &[Furniture],
    ignore: &[EntityId],
) -> Option<EntityId> {
    others
        .iter()
        .filter(|other| other.id != item.id && !ignore.contains(&other.id))
        .find(|other| furniture_hits_furniture(item, other))
        .map(|other| other.id)
}

/// Sweep a move against every other piece. Besides the end pose, intermediate
/// poses are sampled so a fast pointer cannot drag one sofa straight across
/// another. Pieces already overlapping at the start are skipped: a legacy plan
/// may have stacked items, and those must still be separable.
pub fn furniture_path_blocked(
    item: &Furniture,
    from: Pt,
    to: Pt,
    others: &[Furniture],
    ignore: &[EntityId],
) -> Option<EntityId> {
    let mut start = item.clone();
    start.center = from;
    let already_overlapping: Vec<EntityId> = others
        .iter()
        .filter(|other| other.id != item.id && !ignore.contains(&other.id))
        .filter(|other| furniture_hits_furniture(&start, other))
        .map(|other| other.id)
        .collect();

    let dx = (to.x - from.x) as f64;
    let dy = (to.y - from.y) as f64;
    let distance = (dx * dx + dy * dy).sqrt();
    let steps = ((distance / SWEEP_STEP_MM).ceil() as usize).clamp(1, 256);
    let mut probe = item.clone();
    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        probe.center = Pt::new(
            (from.x as f64 + dx * t).round() as Mm,
            (from.y as f64 + dy * t).round() as Mm,
        );
        for other in others {
            if other.id == item.id
                || ignore.contains(&other.id)
                || already_overlapping.contains(&other.id)
            {
                continue;
            }
            if furniture_hits_furniture(&probe, other) {
                return Some(other.id);
            }
        }
    }
    None
}

/// How deep this wall is embedded in the footprint, in millimetres beyond the
/// flush tolerance. `0.0` means free or exactly touching. The wall is a finite
/// segment: a piece beyond its end is outside even if it lies on the same
/// infinite line, which the older depth test wrongly treated as blocking.
fn wall_depth(item: &Furniture, wall: &Wall) -> f64 {
    let dx = (wall.end.x - wall.start.x) as f64;
    let dy = (wall.end.y - wall.start.y) as f64;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return 0.0;
    }
    let ux = dx / len;
    let uy = dy / len;
    let nx = -uy;
    let ny = ux;
    let corners = corners(item);
    // SAT against the actual finite wall body first. The distance formula
    // below measures the wall's infinite centre line, which reports a "deep"
    // overlap for a piece merely lined up beyond the wall's end.
    if !quads_penetrate(&corners, &wall_corners(wall)) {
        return 0.0;
    }
    let along =
        corners.map(|p| (p.x - wall.start.x) as f64 * ux + (p.y - wall.start.y) as f64 * uy);
    let amin = along.iter().cloned().fold(f64::INFINITY, f64::min);
    let amax = along.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if amax <= WALL_CLEARANCE_MM || amin >= len - WALL_CLEARANCE_MM {
        return 0.0;
    }

    let half_wall = wall.thickness_mm.max(1) as f64 * 0.5;
    let dist = ((item.center.x - wall.start.x) as f64 * nx
        + (item.center.y - wall.start.y) as f64 * ny)
        .abs();
    let (pmin, pmax) = projection(&corners, (nx, ny));
    let half_item = (pmax - pmin) * 0.5;
    (half_wall + half_item - dist - WALL_CLEARANCE_MM).max(0.0)
}

fn bounds_of(item: &Furniture) -> (f64, f64, f64, f64) {
    let c = corners(item);
    let mut minx = f64::INFINITY;
    let mut miny = f64::INFINITY;
    let mut maxx = f64::NEG_INFINITY;
    let mut maxy = f64::NEG_INFINITY;
    for p in c {
        minx = minx.min(p.x as f64);
        miny = miny.min(p.y as f64);
        maxx = maxx.max(p.x as f64);
        maxy = maxy.max(p.y as f64);
    }
    (minx, miny, maxx, maxy)
}

/// Signed distance from the piece centre to the wall's centre line. Used to
/// notice that a piece which started embedded in a wall is being dragged clean
/// through to the other side; merely reducing penetration is not enough.
fn signed_line_distance(item: &Furniture, wall: &Wall) -> f64 {
    let dx = (wall.end.x - wall.start.x) as f64;
    let dy = (wall.end.y - wall.start.y) as f64;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return 0.0;
    }
    let nx = -dy / len;
    let ny = dx / len;
    (item.center.x - wall.start.x) as f64 * nx + (item.center.y - wall.start.y) as f64 * ny
}

fn boxes_overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

/// Sweep a candidate move and reject it if any intermediate footprint goes
/// deeper into a wall than it already was. A piece that is already slightly
/// embedded (legacy plans predate collision) can still be pulled out or slid
/// along the wall, but it can never be pushed further in. Safe movement also
/// cannot tunnel through a wall in one fast drag.
///
/// Walls whose bounding boxes are nowhere near the swept footprint are skipped
/// first, so dragging in open space costs almost nothing.
pub fn blocked_along_move(item: &Furniture, from: Pt, to: Pt, walls: &[Wall]) -> Option<EntityId> {
    let mut start = item.clone();
    start.center = from;
    let mut end = item.clone();
    end.center = to;

    let a = bounds_of(&start);
    let b = bounds_of(&end);
    let swept = (
        a.0.min(b.0) - WALL_CLEARANCE_MM,
        a.1.min(b.1) - WALL_CLEARANCE_MM,
        a.2.max(b.2) + WALL_CLEARANCE_MM,
        a.3.max(b.3) + WALL_CLEARANCE_MM,
    );

    let relevant: Vec<&Wall> = walls
        .iter()
        .filter(|wall| {
            let c = wall_corners(wall);
            let mut minx = f64::INFINITY;
            let mut miny = f64::INFINITY;
            let mut maxx = f64::NEG_INFINITY;
            let mut maxy = f64::NEG_INFINITY;
            for p in c {
                minx = minx.min(p.x as f64);
                miny = miny.min(p.y as f64);
                maxx = maxx.max(p.x as f64);
                maxy = maxy.max(p.y as f64);
            }
            boxes_overlap(swept, (minx, miny, maxx, maxy))
        })
        .collect();
    if relevant.is_empty() {
        return None;
    }

    let start_depths: Vec<f64> = relevant.iter().map(|w| wall_depth(&start, w)).collect();
    let start_sides: Vec<f64> = relevant
        .iter()
        .map(|w| signed_line_distance(&start, w))
        .collect();
    let dx = (to.x - from.x) as f64;
    let dy = (to.y - from.y) as f64;
    let distance = (dx * dx + dy * dy).sqrt();
    let steps = ((distance / SWEEP_STEP_MM).ceil() as usize).clamp(1, 256);
    let mut probe = item.clone();
    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        probe.center = Pt::new(
            (from.x as f64 + dx * t).round() as Mm,
            (from.y as f64 + dy * t).round() as Mm,
        );
        for (i, wall) in relevant.iter().enumerate() {
            if wall_depth(&probe, wall) > start_depths[i] + 0.5 {
                return Some(wall.id);
            }
            // A piece that started inside a wall may be pulled out, but it may
            // never be dragged through the wall and out on the far side.
            if start_depths[i] > 0.5
                && start_sides[i].abs() > 0.5
                && wall_depth(&probe, wall) > 0.0
                && signed_line_distance(&probe, wall).signum() != start_sides[i].signum()
            {
                return Some(wall.id);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use fp_model::WallKind;

    use super::*;

    fn wall() -> Wall {
        Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            240,
            WallKind::Exterior,
        )
    }

    fn sofa(center_y: i32, rotation: f64) -> Furniture {
        Furniture {
            id: EntityId(9),
            asset_id: "sofa_two".into(),
            name: "双人沙发".into(),
            center: Pt::new(2000, center_y),
            width_mm: 1600,
            depth_mm: 900,
            rotation_deg: rotation,
            color: "#8B6F55".into(),
            room_id: None,
            price: None,
            floor: 0,
        }
    }

    #[test]
    fn a_sofa_flush_against_a_wall_is_not_a_collision() {
        // Half depth 450 + half wall 120: the edge lands exactly on the wall.
        assert!(!furniture_hits_wall(&sofa(570, 0.0), &wall()));
    }

    #[test]
    fn a_sofa_inside_the_wall_is_a_collision() {
        assert!(furniture_hits_wall(&sofa(500, 0.0), &wall()));
    }

    #[test]
    fn a_rotated_sofa_still_cannot_enter_the_wall() {
        // Rotated 90° the short side is half depth? width and depth swap.
        assert!(furniture_hits_wall(&sofa(400, 90.0), &wall()));
        assert!(!furniture_hits_wall(&sofa(920, 90.0), &wall()));
    }

    #[test]
    fn a_swept_move_cannot_tunnel_through_a_wall() {
        let item = sofa(600, 0.0);
        let blocked = blocked_along_move(&item, Pt::new(2000, 600), Pt::new(2000, -600), &[wall()]);
        assert_eq!(blocked, Some(EntityId(1)));
    }

    #[test]
    fn moving_parallel_outside_the_wall_is_allowed() {
        let item = sofa(600, 0.0);
        let blocked = blocked_along_move(&item, Pt::new(2000, 600), Pt::new(3000, 600), &[wall()]);
        assert_eq!(blocked, None);
    }

    #[test]
    fn a_slightly_embedded_piece_can_be_pulled_out() {
        // 20 mm into the wall body: moving away must be allowed.
        let item = sofa(550, 0.0);
        let blocked = blocked_along_move(&item, Pt::new(2000, 550), Pt::new(2000, 900), &[wall()]);
        assert_eq!(blocked, None);
    }

    #[test]
    fn pushing_a_piece_deeper_into_the_wall_is_blocked() {
        let item = sofa(550, 0.0);
        let blocked = blocked_along_move(&item, Pt::new(2000, 550), Pt::new(2000, 400), &[wall()]);
        assert_eq!(blocked, Some(EntityId(1)));
    }

    #[test]
    fn a_piece_past_the_wall_end_is_not_blocked_by_the_wall_line() {
        // The sofa sits on the same centre line as the wall but 200 mm past its
        // end. The old infinite-line depth test called this a collision.
        let mut item = sofa(570, 0.0);
        item.center.x = 5000;
        let blocked = blocked_along_move(&item, Pt::new(5000, 570), Pt::new(5000, 900), &[wall()]);
        assert_eq!(blocked, None);
    }

    #[test]
    fn a_piece_slightly_past_the_wall_end_can_still_be_pulled_further_out() {
        let mut item = sofa(570, 0.0);
        item.center.x = 4200;
        let blocked = blocked_along_move(&item, Pt::new(4200, 570), Pt::new(4200, 900), &[wall()]);
        assert_eq!(blocked, None);
    }

    #[test]
    fn sliding_an_embedded_piece_along_the_wall_is_allowed() {
        let item = sofa(550, 0.0);
        let blocked = blocked_along_move(&item, Pt::new(1500, 550), Pt::new(3000, 550), &[wall()]);
        assert_eq!(blocked, None);
    }

    fn bearing_wall() -> Wall {
        Wall::new(
            EntityId(2),
            Pt::new(0, 0),
            Pt::new(1000, 0),
            240,
            WallKind::Bearing,
        )
    }

    #[test]
    fn a_bearing_wall_blocks_a_swept_move() {
        let item = sofa(600, 0.0);
        let blocked = blocked_along_move(
            &item,
            Pt::new(500, 600),
            Pt::new(500, -600),
            &[bearing_wall()],
        );
        assert_eq!(blocked, Some(EntityId(2)));
    }

    #[test]
    fn an_embedded_piece_cannot_be_dragged_out_the_far_side() {
        // The sofa starts inside the wall, close to its +Y face. Pulling it
        // out the near side must be allowed, but continuing out the far side
        // is still a wall crossing.
        let item = sofa(50, 0.0);
        let out = blocked_along_move(
            &item,
            Pt::new(500, 50),
            Pt::new(500, 900),
            &[bearing_wall()],
        );
        assert_eq!(out, None);
        let through = blocked_along_move(
            &item,
            Pt::new(500, 50),
            Pt::new(500, -900),
            &[bearing_wall()],
        );
        assert_eq!(through, Some(EntityId(2)));
    }

    #[test]
    fn two_pieces_cannot_overlap() {
        let a = sofa(600, 0.0);
        let mut b = sofa(600, 0.0);
        b.id = EntityId(10);
        b.center = Pt::new(2100, 600);
        assert!(furniture_hits_furniture(&a, &b));
        assert_eq!(
            furniture_blocking_other(&a, std::slice::from_ref(&b), &[]),
            Some(EntityId(10))
        );
    }

    #[test]
    fn flush_pieces_are_not_overlaps() {
        let a = sofa(600, 0.0);
        let mut b = sofa(600, 0.0);
        b.id = EntityId(10);
        // a spans x 1200..2800; b starts at 2800.
        b.center = Pt::new(4400, 600);
        assert!(!furniture_hits_furniture(&a, &b));
    }

    #[test]
    fn a_fast_move_cannot_pass_through_another_piece() {
        let a = sofa(600, 0.0);
        let mut b = sofa(600, 0.0);
        b.id = EntityId(10);
        b.center = Pt::new(3600, 600);
        let blocked = furniture_path_blocked(&a, Pt::new(2000, 600), Pt::new(4000, 600), &[b], &[]);
        assert_eq!(blocked, Some(EntityId(10)));
    }

    #[test]
    fn group_members_do_not_block_each_other() {
        let a = sofa(600, 0.0);
        let mut b = sofa(600, 0.0);
        b.id = EntityId(10);
        b.center = Pt::new(2100, 600);
        let blocked = furniture_path_blocked(
            &a,
            Pt::new(2000, 600),
            Pt::new(2600, 600),
            &[b],
            &[EntityId(10)],
        );
        assert_eq!(blocked, None);
    }
}
