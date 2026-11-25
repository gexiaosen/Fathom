//! Acceptance tests for the Web-imported demo apartment.

use fp_model::{Document, SCHEMA_VERSION};

const FIXTURE: &str = include_str!("../../../assets/samples/demo-apartment.json");

fn load() -> Document {
    Document::from_json(FIXTURE).expect("demo fixture must load")
}

#[test]
fn fixture_has_expected_entities() {
    let doc = load();
    assert_eq!(doc.schema_version, SCHEMA_VERSION);
    assert_eq!(doc.walls.len(), 48, "wall centrelines from the Web plan");
    assert_eq!(doc.rooms.len(), 13, "rooms from the Web plan");
    assert_eq!(doc.materials.len(), 8, "materials from MATS");
    assert_eq!(doc.openings.len(), 7, "demo doors and windows");
    assert!(
        doc.openings
            .iter()
            .all(|o| doc.walls.iter().any(|w| w.id == o.wall_id)),
        "every opening hangs on an existing wall"
    );
}

#[test]
fn fixture_roundtrips_exactly() {
    let doc = load();
    let json = doc.to_json().expect("serialize");
    let back = Document::from_json(&json).expect("deserialize");
    assert_eq!(doc, back);
}

#[test]
fn counted_area_matches_web_plan() {
    let doc = load();
    let counted: f64 = doc
        .rooms
        .iter()
        .filter(|room| room.counted)
        .map(|room| fp_geom::area(&room.polygon))
        .sum::<f64>()
        / 1_000_000.0;
    assert!(
        (counted - 87.2).abs() < 0.5,
        "counted area {counted:.1} m2 should match the Web plan"
    );
}

#[test]
fn every_room_triangulates_to_its_own_area() {
    let doc = load();
    for room in &doc.rooms {
        let triangles = fp_geom::triangulate(&room.polygon);
        assert!(
            !triangles.is_empty(),
            "room {} failed to triangulate",
            room.name
        );
        let poly_area = fp_geom::area(&room.polygon) / 1_000_000.0;
        let tri_area = fp_geom::triangulate::triangles_area_m2(&triangles);
        assert!(
            (poly_area - tri_area).abs() < 1e-6,
            "room {}: polygon {poly_area:.6} vs triangles {tri_area:.6}",
            room.name
        );
    }
}

#[test]
fn walls_are_inside_the_plan_bounds() {
    let doc = load();
    let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
    let (mut max_x, mut max_y) = (i32::MIN, i32::MIN);
    for wall in &doc.walls {
        for p in [wall.start, wall.end] {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
    }
    assert!(max_x - min_x > 10_000, "plan should be wider than 10 m");
    assert!(max_y - min_y > 10_000, "plan should be taller than 10 m");
    assert!(-500 <= min_x && max_x <= 12_500, "bounds {min_x}..{max_x}");
}
