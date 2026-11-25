//! Polygon boolean operations, backed by `geo` (i_overlay).
//!
//! Results are outer rings only; holes are not expected for room-scale
//! geometry, and callers that need them can extend this module later.

use fp_core::Pt;
use geo::{BooleanOps, Coord, LineString, MultiPolygon, Polygon};

fn to_geo(points: &[Pt]) -> Option<Polygon<f64>> {
    if points.len() < 3 {
        return None;
    }
    let coords: Vec<Coord<f64>> = points
        .iter()
        .map(|p| Coord {
            x: p.x as f64,
            y: p.y as f64,
        })
        .collect();
    Some(Polygon::new(LineString::new(coords), Vec::new()))
}

fn from_multi(multi: &MultiPolygon<f64>) -> Vec<Vec<Pt>> {
    multi
        .0
        .iter()
        .map(|poly| {
            poly.exterior()
                .0
                .iter()
                .map(|c| Pt::new(c.x.round() as i32, c.y.round() as i32))
                .collect()
        })
        .collect()
}

pub fn union(a: &[Pt], b: &[Pt]) -> Vec<Vec<Pt>> {
    match (to_geo(a), to_geo(b)) {
        (Some(pa), Some(pb)) => from_multi(&pa.union(&pb)),
        (Some(pa), None) => from_multi(&MultiPolygon(vec![pa])),
        (None, Some(pb)) => from_multi(&MultiPolygon(vec![pb])),
        (None, None) => Vec::new(),
    }
}

pub fn intersection(a: &[Pt], b: &[Pt]) -> Vec<Vec<Pt>> {
    match (to_geo(a), to_geo(b)) {
        (Some(pa), Some(pb)) => from_multi(&pa.intersection(&pb)),
        _ => Vec::new(),
    }
}

pub fn difference(a: &[Pt], b: &[Pt]) -> Vec<Vec<Pt>> {
    match (to_geo(a), to_geo(b)) {
        (Some(pa), Some(pb)) => from_multi(&pa.difference(&pb)),
        (Some(pa), None) => from_multi(&MultiPolygon(vec![pa])),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<Pt> {
        vec![
            Pt::new(x0, y0),
            Pt::new(x1, y0),
            Pt::new(x1, y1),
            Pt::new(x0, y1),
        ]
    }

    fn area_m2(polys: &[Vec<Pt>]) -> f64 {
        polys
            .iter()
            .map(|poly| {
                let mut sum = 0i64;
                for i in 0..poly.len() {
                    let a = poly[i];
                    let b = poly[(i + 1) % poly.len()];
                    sum += a.x as i64 * b.y as i64 - b.x as i64 * a.y as i64;
                }
                (sum as f64 / 2.0).abs() / 1_000_000.0
            })
            .sum()
    }

    #[test]
    fn union_of_overlapping_rectangles() {
        let a = rect(0, 0, 4000, 4000);
        let b = rect(2000, 0, 6000, 4000);
        let out = union(&a, &b);
        assert_eq!(out.len(), 1);
        assert!((area_m2(&out) - 24.0).abs() < 0.01);
    }

    #[test]
    fn intersection_of_overlapping_rectangles() {
        let a = rect(0, 0, 4000, 4000);
        let b = rect(2000, 0, 6000, 4000);
        let out = intersection(&a, &b);
        assert_eq!(out.len(), 1);
        assert!((area_m2(&out) - 8.0).abs() < 0.01);
    }

    #[test]
    fn difference_carves_a_notch() {
        let a = rect(0, 0, 4000, 4000);
        let b = rect(2000, 2000, 6000, 6000);
        let out = difference(&a, &b);
        assert!((area_m2(&out) - 12.0).abs() < 0.01);
    }

    #[test]
    fn empty_input_is_handled() {
        assert!(union(&[], &[]).is_empty());
        assert!(intersection(&rect(0, 0, 1000, 1000), &[]).is_empty());
        assert_eq!(union(&rect(0, 0, 1000, 1000), &[]).len(), 1);
    }
}
