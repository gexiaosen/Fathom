//! Polygon triangulation (ear clipping) for 2D fills and 3D floors.

use fp_core::Pt;

/// Triangulate a simple polygon given in millimetres.
///
/// Returns triangles in input-space; an empty vector means the polygon was
/// degenerate or self-intersecting enough that ear clipping failed.
pub fn triangulate(polygon: &[Pt]) -> Vec<[Pt; 3]> {
    if polygon.len() < 3 {
        return Vec::new();
    }
    let coords: Vec<f64> = polygon
        .iter()
        .flat_map(|p| [p.x as f64, p.y as f64])
        .collect();
    match earcutr::earcut(&coords, &[], 2) {
        Ok(indices) if indices.len() >= 3 => {
            let (tris, _) = indices.as_chunks::<3>();
            tris.iter()
                .filter_map(|tri| {
                    Some([
                        *polygon.get(tri[0])?,
                        *polygon.get(tri[1])?,
                        *polygon.get(tri[2])?,
                    ])
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Total area of a triangle list, in m².
pub fn triangles_area_m2(triangles: &[[Pt; 3]]) -> f64 {
    triangles
        .iter()
        .map(|[a, b, c]| {
            let area = ((b.x - a.x) as f64 * (c.y - a.y) as f64
                - (b.y - a.y) as f64 * (c.x - a.x) as f64)
                .abs()
                / 2.0;
            area / 1_000_000.0
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_gives_two_triangles() {
        let poly = [
            Pt::new(0, 0),
            Pt::new(4000, 0),
            Pt::new(4000, 3000),
            Pt::new(0, 3000),
        ];
        let tris = triangulate(&poly);
        assert_eq!(tris.len(), 2);
        assert!((triangles_area_m2(&tris) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn concave_polygon_is_triangulated() {
        let poly = [
            Pt::new(0, 0),
            Pt::new(6000, 0),
            Pt::new(6000, 3000),
            Pt::new(3000, 3000),
            Pt::new(3000, 6000),
            Pt::new(0, 6000),
        ];
        let tris = triangulate(&poly);
        assert_eq!(tris.len(), 4);
        assert!((triangles_area_m2(&tris) - 27.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_polygon_returns_empty() {
        assert!(triangulate(&[Pt::new(0, 0), Pt::new(1000, 0)]).is_empty());
    }
}
