//! 2D geometry helpers used by the editor, in millimetres.

pub mod boolean;
pub mod planar;
pub mod triangulate;

pub use boolean::{difference, intersection, union, union_all};
pub use planar::{ROOM_JOIN_TOLERANCE_MM, detect_rooms};
pub use triangulate::{triangles_area_m2, triangulate};

use fp_core::{Mm, Pt};

/// Signed polygon area (shoelace). Positive means counter-clockwise.
pub fn signed_area(points: &[Pt]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut sum = 0i64;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        sum += (a.x as i64) * (b.y as i64) - (b.x as i64) * (a.y as i64);
    }
    sum as f64 / 2.0
}

pub fn area(points: &[Pt]) -> f64 {
    signed_area(points).abs()
}

pub fn perimeter(points: &[Pt], closed: bool) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut total = 0.0;
    for i in 0..points.len() - 1 {
        total += distance(points[i], points[i + 1]);
    }
    if closed {
        total += distance(points[points.len() - 1], points[0]);
    }
    total
}

pub fn distance(a: Pt, b: Pt) -> f64 {
    let dx = (a.x - b.x) as f64;
    let dy = (a.y - b.y) as f64;
    (dx * dx + dy * dy).sqrt()
}

/// Ray casting point-in-polygon test.
pub fn point_in_polygon(p: Pt, polygon: &[Pt]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[j];
        let intersects = ((a.y > p.y) != (b.y > p.y))
            && ((p.x as f64)
                < (b.x - a.x) as f64 * (p.y - a.y) as f64 / (b.y - a.y) as f64 + a.x as f64);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn orientation(a: Pt, b: Pt, c: Pt) -> i64 {
    let value = (b.x as i64 - a.x as i64) * (c.y as i64 - a.y as i64)
        - (b.y as i64 - a.y as i64) * (c.x as i64 - a.x as i64);
    value.signum()
}

fn on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// Inclusive segment intersection (touching endpoints count).
pub fn segments_intersect(a1: Pt, a2: Pt, b1: Pt, b2: Pt) -> bool {
    let o1 = orientation(a1, a2, b1);
    let o2 = orientation(a1, a2, b2);
    let o3 = orientation(b1, b2, a1);
    let o4 = orientation(b1, b2, a2);

    if o1 != o2 && o3 != o4 {
        return true;
    }
    (o1 == 0 && on_segment(a1, a2, b1))
        || (o2 == 0 && on_segment(a1, a2, b2))
        || (o3 == 0 && on_segment(b1, b2, a1))
        || (o4 == 0 && on_segment(b1, b2, a2))
}

pub fn distance_point_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let dx = (b.x - a.x) as f64;
    let dy = (b.y - a.y) as f64;
    let len_sq = dx * dx + dy * dy;
    if len_sq == 0.0 {
        return distance(p, a);
    }
    let t = (((p.x - a.x) as f64) * dx + ((p.y - a.y) as f64) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let proj = Pt::new(
        (a.x as f64 + t * dx).round() as Mm,
        (a.y as f64 + t * dy).round() as Mm,
    );
    distance(p, proj)
}

/// Snap a value to the nearest multiple of `grid` (grid must be positive).
pub fn snap_to_grid(value: Mm, grid: Mm) -> Mm {
    if grid <= 0 {
        return value;
    }
    let half = grid / 2;
    if value >= 0 {
        ((value + half) / grid) * grid
    } else {
        -(((-value + half) / grid) * grid)
    }
}

/// Snap an angle to the nearest step, returning degrees in [0, 360).
pub fn snap_angle_deg(angle: f64, step_deg: f64) -> f64 {
    if step_deg <= 0.0 {
        return normalize_angle_deg(angle);
    }
    let snapped = (angle / step_deg).round() * step_deg;
    normalize_angle_deg(snapped)
}

pub fn normalize_angle_deg(angle: f64) -> f64 {
    let mut a = angle % 360.0;
    if a < 0.0 {
        a += 360.0;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: i32, h: i32) -> Vec<Pt> {
        vec![Pt::new(0, 0), Pt::new(w, 0), Pt::new(w, h), Pt::new(0, h)]
    }

    #[test]
    fn rectangle_area_and_perimeter() {
        let r = rect(4000, 3000);
        assert_eq!(area(&r), 12_000_000.0);
        assert_eq!(perimeter(&r, true), 14_000.0);
    }

    #[test]
    fn point_inside_and_outside() {
        let r = rect(4000, 3000);
        assert!(point_in_polygon(Pt::new(2000, 1500), &r));
        assert!(!point_in_polygon(Pt::new(5000, 1500), &r));
        assert!(!point_in_polygon(Pt::new(-10, 1500), &r));
    }

    #[test]
    fn crossing_segments() {
        assert!(segments_intersect(
            Pt::new(0, 0),
            Pt::new(1000, 1000),
            Pt::new(0, 1000),
            Pt::new(1000, 0)
        ));
        assert!(!segments_intersect(
            Pt::new(0, 0),
            Pt::new(1000, 0),
            Pt::new(0, 500),
            Pt::new(1000, 500)
        ));
    }

    #[test]
    fn point_to_segment_distance() {
        let d = distance_point_segment(Pt::new(500, 300), Pt::new(0, 0), Pt::new(1000, 0));
        assert!((d - 300.0).abs() < 1e-6);
    }

    #[test]
    fn grid_and_angle_snapping() {
        assert_eq!(snap_to_grid(1234, 10), 1230);
        assert_eq!(snap_to_grid(-1234, 10), -1230);
        assert_eq!(snap_angle_deg(37.0, 15.0), 30.0);
        assert_eq!(snap_angle_deg(-7.0, 15.0), 0.0);
    }
}
