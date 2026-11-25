//! Planar graph traversal: recover closed rooms from wall centrelines.
//!
//! Walls are first split at every intersection (including T-junctions where an
//! endpoint lands in the middle of another wall), then merged into a planar
//! graph. Walking each half-edge's clockwise predecessor yields the minimal
//! faces; faces with positive signed area are rooms, the negative one is the
//! outside.

use std::collections::{HashMap, HashSet};

use fp_core::{Mm, Pt};

const EPS: f64 = 1e-6;

/// Detect rooms bounded by wall segments.
///
/// - `tolerance_mm`: vertices closer than this are merged (default 10 mm).
/// - `min_area_m2`: faces smaller than this are discarded (default 0.3 m²).
pub fn detect_rooms(segments: &[(Pt, Pt)], tolerance_mm: Mm, min_area_m2: f64) -> Vec<Vec<Pt>> {
    let split = split_at_intersections(segments);
    if split.is_empty() {
        return Vec::new();
    }

    let tol = tolerance_mm.max(1) as f64;
    let key = |p: Pt| {
        (
            (p.x as f64 / tol).round() as i64,
            (p.y as f64 / tol).round() as i64,
        )
    };

    let mut verts: Vec<Pt> = Vec::new();
    let mut vert_index: HashMap<(i64, i64), usize> = HashMap::new();
    let mut edges: Vec<(usize, usize)> = Vec::new();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();

    for (a, b) in &split {
        let ka = key(*a);
        let kb = key(*b);
        if ka == kb {
            continue;
        }
        let ia = *vert_index.entry(ka).or_insert_with(|| {
            verts.push(*a);
            verts.len() - 1
        });
        let ib = *vert_index.entry(kb).or_insert_with(|| {
            verts.push(*b);
            verts.len() - 1
        });
        let pair = (ia.min(ib), ia.max(ib));
        if seen.insert(pair) {
            edges.push(pair);
        }
    }

    // Half-edges: two per undirected edge.
    #[derive(Clone, Copy)]
    struct HalfEdge {
        from: usize,
        to: usize,
        twin: usize,
    }
    let mut half: Vec<HalfEdge> = Vec::with_capacity(edges.len() * 2);
    for (u, v) in edges {
        let i = half.len();
        half.push(HalfEdge {
            from: u,
            to: v,
            twin: i + 1,
        });
        half.push(HalfEdge {
            from: v,
            to: u,
            twin: i,
        });
    }

    let angle = |h: &HalfEdge| {
        let a = verts[h.from];
        let b = verts[h.to];
        ((b.y - a.y) as f64).atan2((b.x - a.x) as f64)
    };

    let mut order: Vec<Vec<usize>> = vec![Vec::new(); verts.len()];
    for (i, h) in half.iter().enumerate() {
        order[h.from].push(i);
    }
    for list in &mut order {
        list.sort_by(|&i, &j| angle(&half[i]).partial_cmp(&angle(&half[j])).unwrap());
    }

    let mut used = vec![false; half.len()];
    let mut rooms: Vec<Vec<Pt>> = Vec::new();
    let mut room_keys: HashSet<Vec<usize>> = HashSet::new();

    for start in 0..half.len() {
        if used[start] {
            continue;
        }
        let mut h = start;
        let mut walk: Vec<usize> = Vec::new();
        let guard = half.len() * 2 + 4;
        loop {
            if used[h] && h != start {
                break;
            }
            used[h] = true;
            walk.push(half[h].from);

            let twin = half[h].twin;
            let list = &order[half[h].to];
            let Some(pos) = list.iter().position(|&e| e == twin) else {
                break;
            };
            let next = list[(pos + list.len() - 1) % list.len()];
            h = next;
            if h == start || walk.len() > guard {
                break;
            }
        }

        if walk.len() < 3 {
            continue;
        }

        let poly: Vec<Pt> = walk.iter().map(|&v| verts[v]).collect();
        let area = signed_area_f64(&poly);
        if area <= min_area_m2 * 1_000_000.0 {
            continue;
        }

        let mut ids = walk.clone();
        if let Some(min) = ids
            .iter()
            .enumerate()
            .min_by_key(|(_, v)| **v)
            .map(|(i, _)| i)
        {
            ids.rotate_left(min);
        }
        if room_keys.insert(ids) {
            rooms.push(poly);
        }
    }

    rooms
}

fn signed_area_f64(points: &[Pt]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        sum += a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
    }
    sum / 2.0
}

fn cross(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    ax * by - ay * bx
}

fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt::new(
        (a.x as f64 + (b.x - a.x) as f64 * t).round() as Mm,
        (a.y as f64 + (b.y - a.y) as f64 * t).round() as Mm,
    )
}

fn split_at_intersections(segments: &[(Pt, Pt)]) -> Vec<(Pt, Pt)> {
    let mut out = Vec::new();
    for (i, (a, b)) in segments.iter().enumerate() {
        let mut ts = vec![0.0f64, 1.0];
        for (j, (c, d)) in segments.iter().enumerate() {
            if i == j {
                continue;
            }
            for t in intersection_params(*a, *b, *c, *d) {
                if t > EPS && t < 1.0 - EPS {
                    ts.push(t);
                }
            }
        }
        ts.sort_by(|x, y| x.partial_cmp(y).unwrap());
        ts.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
        for w in ts.windows(2) {
            let p = lerp(*a, *b, w[0]);
            let q = lerp(*a, *b, w[1]);
            if (p.x - q.x).abs() >= 1 || (p.y - q.y).abs() >= 1 {
                out.push((p, q));
            }
        }
    }
    out
}

fn intersection_params(a: Pt, b: Pt, c: Pt, d: Pt) -> Vec<f64> {
    let ax = a.x as f64;
    let ay = a.y as f64;
    let rx = (b.x - a.x) as f64;
    let ry = (b.y - a.y) as f64;
    let sx = (d.x - c.x) as f64;
    let sy = (d.y - c.y) as f64;
    let qpx = c.x as f64 - ax;
    let qpy = c.y as f64 - ay;
    let denom = cross(rx, ry, sx, sy);

    if denom.abs() < EPS {
        // Parallel: handle collinear overlap so duplicate walls do not create
        // extra edges.
        if cross(qpx, qpy, rx, ry).abs() < EPS {
            let len2 = rx * rx + ry * ry;
            if len2 < EPS {
                return Vec::new();
            }
            let tc = (qpx * rx + qpy * ry) / len2;
            let td = ((d.x as f64 - ax) * rx + (d.y as f64 - ay) * ry) / len2;
            let lo = tc.min(td).clamp(0.0, 1.0);
            let hi = tc.max(td).clamp(0.0, 1.0);
            if hi - lo > EPS {
                return vec![lo, hi];
            }
        }
        return Vec::new();
    }

    let t = cross(qpx, qpy, sx, sy) / denom;
    let u = cross(qpx, qpy, rx, ry) / denom;
    if (-EPS..=1.0 + EPS).contains(&t) && (-EPS..=1.0 + EPS).contains(&u) {
        vec![t.clamp(0.0, 1.0)]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(Pt, Pt)> {
        let p = [
            Pt::new(x0, y0),
            Pt::new(x1, y0),
            Pt::new(x1, y1),
            Pt::new(x0, y1),
        ];
        (0..4).map(|i| (p[i], p[(i + 1) % 4])).collect()
    }

    #[test]
    fn single_rectangle_is_one_room() {
        let rooms = detect_rooms(&rect(0, 0, 4000, 3000), 10, 0.3);
        assert_eq!(rooms.len(), 1);
        assert!((signed_area_f64(&rooms[0]).abs() - 12_000_000.0).abs() < 1.0);
    }

    #[test]
    fn shared_wall_splits_two_rooms() {
        let mut segs = rect(0, 0, 8000, 3000);
        segs.push((Pt::new(4000, 0), Pt::new(4000, 3000)));
        let rooms = detect_rooms(&segs, 10, 0.3);
        assert_eq!(rooms.len(), 2);
        for room in &rooms {
            assert!((signed_area_f64(room).abs() - 12_000_000.0).abs() < 1.0);
        }
    }

    #[test]
    fn t_junction_wall_is_split() {
        // Outer rectangle plus a stub that only touches the bottom wall.
        let mut segs = rect(0, 0, 6000, 4000);
        segs.push((Pt::new(3000, 2000), Pt::new(3000, 4000)));
        let rooms = detect_rooms(&segs, 10, 0.3);
        // The stub does not reach the top, so no closed split: still one room.
        assert_eq!(rooms.len(), 1);
        assert!((signed_area_f64(&rooms[0]).abs() - 24_000_000.0).abs() < 1.0);
    }

    #[test]
    fn l_shaped_room() {
        let pts = [
            Pt::new(0, 0),
            Pt::new(6000, 0),
            Pt::new(6000, 3000),
            Pt::new(3000, 3000),
            Pt::new(3000, 6000),
            Pt::new(0, 6000),
        ];
        let segs: Vec<(Pt, Pt)> = (0..pts.len())
            .map(|i| (pts[i], pts[(i + 1) % pts.len()]))
            .collect();
        let rooms = detect_rooms(&segs, 10, 0.3);
        assert_eq!(rooms.len(), 1);
        // L area = 6000*3000 + 3000*3000 = 27 m2
        assert!((signed_area_f64(&rooms[0]).abs() - 27_000_000.0).abs() < 1.0);
    }

    #[test]
    fn open_polyline_has_no_rooms() {
        let segs = vec![
            (Pt::new(0, 0), Pt::new(4000, 0)),
            (Pt::new(4000, 0), Pt::new(4000, 3000)),
            (Pt::new(4000, 3000), Pt::new(0, 3000)),
        ];
        assert!(detect_rooms(&segs, 10, 0.3).is_empty());
    }
}
