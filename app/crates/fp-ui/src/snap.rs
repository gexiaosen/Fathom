//! Furniture snapping: keep a piece flush against nearby walls.

use fp_core::{Mm, Pt};
use fp_model::{Furniture, Wall};

/// Gap under which a piece jumps flush to the wall surface.
pub const FURNITURE_SNAP_MM: Mm = 24;

/// Nudge a candidate centre so that the closest wall side sits flush.
///
/// The half extent facing the wall is the rectangle's projection onto the wall
/// normal, so rotation is handled without special-casing 90° steps.
pub fn snap_furniture(item: &Furniture, walls: &[Wall], candidate: Pt) -> Pt {
    let (sin, cos) = item.rotation_deg.to_radians().sin_cos();
    let hw = item.width_mm as f64 / 2.0;
    let hd = item.depth_mm as f64 / 2.0;
    let mut best: Option<(f64, Pt)> = None;
    for wall in walls {
        let (tx, ty) = (
            (wall.end.x - wall.start.x) as f64,
            (wall.end.y - wall.start.y) as f64,
        );
        let len = (tx * tx + ty * ty).sqrt();
        if len < 1e-6 {
            continue;
        }
        let (dx, dy) = (tx / len, ty / len);
        let (nx, ny) = (-dy, dx);
        let cx = (candidate.x - wall.start.x) as f64;
        let cy = (candidate.y - wall.start.y) as f64;
        let dist = cx * nx + cy * ny;
        let raw_t = cx * dx + cy * dy;
        // A wall is a finite segment. Snapping to its clamped endpoint would
        // yank a piece that is already past the corner sideways onto the wall.
        if raw_t < 0.0 || raw_t > len {
            continue;
        }
        let t = raw_t;
        let half = (hw * (nx * cos + ny * sin)).abs() + (hd * (ny * cos - nx * sin)).abs();
        let gap = dist.abs() - half - wall.thickness_mm as f64 / 2.0;
        if gap.abs() > FURNITURE_SNAP_MM as f64 {
            continue;
        }
        let signed = half + wall.thickness_mm as f64 / 2.0;
        let target = if dist >= 0.0 { signed } else { -signed };
        let px = wall.start.x as f64 + dx * t + nx * target;
        let py = wall.start.y as f64 + dy * t + ny * target;
        let score = gap.abs();
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, Pt::new(px.round() as i32, py.round() as i32)));
        }
    }
    best.map(|(_, p)| p).unwrap_or(candidate)
}

/// Axis-aligned bounds of a rotated footprint in plan millimetres.
pub fn furniture_bounds(item: &Furniture) -> (Pt, Pt) {
    let (sin, cos) = item.rotation_deg.to_radians().sin_cos();
    let hx = (item.width_mm as f64 / 2.0 * cos).abs() + (item.depth_mm as f64 / 2.0 * sin).abs();
    let hy = (item.width_mm as f64 / 2.0 * sin).abs() + (item.depth_mm as f64 / 2.0 * cos).abs();
    (
        Pt::new(
            (item.center.x as f64 - hx).round() as i32,
            (item.center.y as f64 - hy).round() as i32,
        ),
        Pt::new(
            (item.center.x as f64 + hx).round() as i32,
            (item.center.y as f64 + hy).round() as i32,
        ),
    )
}

/// Snap a candidate centre to the edges or centres of other furniture.
///
/// X and Y corrections are chosen independently, so a piece can line up with
/// one neighbour horizontally and another vertically in a single move.
pub fn snap_furniture_to_others(item: &Furniture, others: &[Furniture], candidate: Pt) -> Pt {
    let mut probe = item.clone();
    probe.center = candidate;
    let (min, max) = furniture_bounds(&probe);
    let center = Pt::new((min.x + max.x) / 2, (min.y + max.y) / 2);
    let mut best_x: Option<(f64, i32)> = None;
    let mut best_y: Option<(f64, i32)> = None;
    for other in others {
        if other.id == item.id {
            continue;
        }
        let (omin, omax) = furniture_bounds(other);
        let oc = Pt::new((omin.x + omax.x) / 2, (omin.y + omax.y) / 2);
        for edge in [min.x, center.x, max.x] {
            for other_edge in [omin.x, oc.x, omax.x] {
                let d = other_edge - edge;
                if d.abs() <= FURNITURE_SNAP_MM
                    && best_x.is_none_or(|(bd, _)| (d as f64).abs() < bd)
                {
                    best_x = Some((d as f64, d));
                }
            }
        }
        for edge in [min.y, center.y, max.y] {
            for other_edge in [omin.y, oc.y, omax.y] {
                let d = other_edge - edge;
                if d.abs() <= FURNITURE_SNAP_MM
                    && best_y.is_none_or(|(bd, _)| (d as f64).abs() < bd)
                {
                    best_y = Some((d as f64, d));
                }
            }
        }
    }
    Pt::new(
        candidate.x + best_x.map(|(_, d)| d).unwrap_or(0),
        candidate.y + best_y.map(|(_, d)| d).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use fp_core::EntityId;
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

    fn sofa(rot: f64) -> Furniture {
        Furniture {
            id: EntityId(9),
            asset_id: "sofa".into(),
            name: "沙发".into(),
            center: Pt::new(2000, 600),
            width_mm: 2200,
            depth_mm: 900,
            rotation_deg: rot,
            color: "#8b6f55".into(),
            room_id: None,
        }
    }

    #[test]
    fn near_wall_becomes_flush() {
        let f = sofa(0.0);
        let p = snap_furniture(&f, &[wall()], Pt::new(2000, 580));
        assert_eq!(p.y, 450 + 120, "half depth + half wall thickness");
        assert_eq!(p.x, 2000);
    }

    #[test]
    fn rotation_uses_the_short_side() {
        let f = sofa(90.0);
        let p = snap_furniture(&f, &[wall()], Pt::new(2000, 1240));
        assert_eq!(p.y, 1100 + 120);
    }

    #[test]
    fn far_away_is_untouched() {
        let f = sofa(0.0);
        let candidate = Pt::new(2000, 3000);
        assert_eq!(snap_furniture(&f, &[wall()], candidate), candidate);
    }

    #[test]
    fn furniture_edges_and_centres_snap() {
        let mut moving = sofa(0.0);
        moving.id = EntityId(9);
        let mut neighbour = sofa(0.0);
        neighbour.id = EntityId(10);
        neighbour.center = Pt::new(3200, 0);
        let candidate = Pt::new(1010, 3000);
        let p = snap_furniture_to_others(&moving, &[neighbour], candidate);
        assert_eq!(p.x, 1000, "right edge 2190 snaps to neighbour left 2100");
        assert_eq!(p.y, 3000, "vertical position untouched");
    }

    #[test]
    fn bounds_respect_rotation() {
        let mut f = sofa(0.0);
        f.rotation_deg = 90.0;
        let (min, max) = furniture_bounds(&f);
        assert_eq!(max.x - min.x, 900);
        assert_eq!(max.y - min.y, 2200);
    }
}
