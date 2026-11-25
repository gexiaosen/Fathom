//! 3D mesh generation: walls extruded to boxes, rooms flattened to floors,
//! furniture as rotated boxes.
//!
//! Coordinates are metres with Y up, matching the renderer's NDC convention.

use fp_core::Pt;
use fp_model::catalog::material_kind_color;
use fp_model::{Furniture, Material, Opening, OpeningKind, Room, Wall};

/// Default surface colours, overridable per entity.
pub const WALL_COLOR: [f32; 3] = [0.88, 0.86, 0.83];
pub const FLOOR_FALLBACK: [f32; 3] = [0.76, 0.73, 0.69];
pub const FURNITURE_FALLBACK: [f32; 3] = [0.62, 0.58, 0.54];

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    /// x = roughness, y = metalness, z = emissive strength.
    pub mat: [f32; 3],
    /// 1.0 = opaque; anything lower is drawn in the glass pass.
    pub alpha: f32,
    /// Procedural floor pattern: 0 plain, 1 walnut, 2 oak, 3 marble,
    /// 4 carpet, 5 terrazzo, 6 tile, 7 stone.
    pub pattern: f32,
}

impl MeshVertex {
    fn new(pos: [f32; 3], normal: [f32; 3], color: [f32; 3]) -> Self {
        Self {
            pos,
            normal,
            color,
            mat: [1.0, 0.0, 0.0],
            alpha: 1.0,
            pattern: 0.0,
        }
    }

    pub fn with_mat(pos: [f32; 3], normal: [f32; 3], color: [f32; 3], mat: [f32; 3]) -> Self {
        Self {
            pos,
            normal,
            color,
            mat,
            alpha: 1.0,
            pattern: 0.0,
        }
    }

    pub fn with_alpha(
        pos: [f32; 3],
        normal: [f32; 3],
        color: [f32; 3],
        mat: [f32; 3],
        alpha: f32,
    ) -> Self {
        Self {
            pos,
            normal,
            color,
            mat,
            alpha,
            pattern: 0.0,
        }
    }

    pub fn with_pattern(
        pos: [f32; 3],
        normal: [f32; 3],
        color: [f32; 3],
        mat: [f32; 3],
        pattern: f32,
    ) -> Self {
        Self {
            pos,
            normal,
            color,
            mat,
            alpha: 1.0,
            pattern,
        }
    }
}

/// A mesh split into an opaque prefix and a glass tail. The renderer draws
/// the prefix with depth writes, then the tail with alpha blending.
#[derive(Default, Clone, Debug)]
pub struct MeshParts {
    pub vertices: Vec<MeshVertex>,
    pub opaque: usize,
}

impl MeshParts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_opaque(&mut self, part: Vec<MeshVertex>) {
        self.vertices.extend(part);
        self.opaque = self.vertices.len();
    }

    pub fn push_mixed(&mut self, part: MeshParts) {
        self.vertices.extend(part.vertices);
        self.opaque += part.opaque;
    }

    pub fn push_glass(&mut self, part: Vec<MeshVertex>) {
        self.vertices.extend(part);
    }
}

/// A large neutral studio ground under the model. A mid-dark grey keeps the
/// building and its shadows readable instead of washing the whole view into
/// one warm beige field.
pub fn ground_mesh() -> Vec<MeshVertex> {
    let h = 80.0f32;
    let y = -0.02f32;
    let color = [0.07, 0.074, 0.08];
    let mat = [0.92, 0.0, 0.0];
    let a = [-h, y, -h];
    let b = [h, y, -h];
    let c = [h, y, h];
    let d = [-h, y, h];
    let mut out = Vec::new();
    for v in [a, b, c, a, c, d] {
        out.push(MeshVertex::with_pattern(
            v,
            [0.0, 1.0, 0.0],
            color,
            mat,
            7.0,
        ));
    }
    out
}

/// Extrude every wall into a closed box (no openings).
pub fn wall_mesh(walls: &[Wall]) -> Vec<MeshVertex> {
    wall_mesh_with_openings(walls, &[])
}

/// Extrude walls, cutting door and window openings.
///
/// A wall is split into full-height solid runs between holes; each hole keeps
/// the wall below its sill and above its head, so doors leave a header and
/// windows leave a lintel plus a sill wall.
pub fn wall_mesh_with_openings(walls: &[Wall], openings: &[Opening]) -> Vec<MeshVertex> {
    let mut out = Vec::new();
    for wall in walls {
        let len_m =
            ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64) / 1000.0;
        if len_m < 1e-4 {
            continue;
        }
        let h = wall.height_mm as f32 / 1000.0;
        let half = wall.thickness_mm as f32 / 2000.0;
        let a = [wall.start.x as f32 / 1000.0, wall.start.y as f32 / 1000.0];
        let dir = [
            (wall.end.x - wall.start.x) as f32 / 1000.0 / len_m as f32,
            (wall.end.y - wall.start.y) as f32 / 1000.0 / len_m as f32,
        ];
        let point_at = |t: f32| [a[0] + dir[0] * t, a[1] + dir[1] * t];

        let mut holes: Vec<(f32, f32, f32, f32)> = openings
            .iter()
            .filter(|o| o.wall_id == wall.id)
            .map(|o| {
                (
                    o.offset_mm as f32 / 1000.0,
                    (o.offset_mm + o.width_mm) as f32 / 1000.0,
                    o.sill_mm as f32 / 1000.0,
                    (o.sill_mm + o.height_mm) as f32 / 1000.0,
                )
            })
            .filter(|(s, e, _, _)| e > s)
            .collect();
        holes.sort_by(|x, y| x.0.total_cmp(&y.0));

        let mut cursor = 0.0f32;
        for (s, e, _, _) in &holes {
            let s = s.clamp(0.0, len_m as f32);
            let e = e.clamp(0.0, len_m as f32);
            if s > cursor {
                box_segment(&mut out, point_at(cursor), point_at(s), half, 0.0, h);
            }
            cursor = cursor.max(e);
        }
        if cursor < len_m as f32 {
            box_segment(
                &mut out,
                point_at(cursor),
                point_at(len_m as f32),
                half,
                0.0,
                h,
            );
        }

        for (s, e, sill, head) in &holes {
            if *sill > 0.0 {
                box_segment(&mut out, point_at(*s), point_at(*e), half, 0.0, *sill);
            }
            if *head < h {
                box_segment(&mut out, point_at(*s), point_at(*e), half, *head, h);
            }
        }
    }
    out
}

fn box_segment(out: &mut Vec<MeshVertex>, a: [f32; 2], b: [f32; 2], half: f32, y0: f32, y1: f32) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 1e-6 || y1 <= y0 {
        return;
    }
    let dir = [d[0] / len, d[1] / len];
    let normal = [-dir[1], dir[0]];

    let s0 = [a[0] + normal[0] * half, a[1] + normal[1] * half];
    let s1 = [b[0] + normal[0] * half, b[1] + normal[1] * half];
    let s2 = [b[0] - normal[0] * half, b[1] - normal[1] * half];
    let s3 = [a[0] - normal[0] * half, a[1] - normal[1] * half];

    let p = |x: f32, y: f32, z: f32| [x, y, z];
    quad(
        out,
        p(s0[0], y0, s0[1]),
        p(s1[0], y0, s1[1]),
        p(s1[0], y1, s1[1]),
        p(s0[0], y1, s0[1]),
        [normal[0], 0.0, normal[1]],
    );
    quad(
        out,
        p(s2[0], y0, s2[1]),
        p(s3[0], y0, s3[1]),
        p(s3[0], y1, s3[1]),
        p(s2[0], y1, s2[1]),
        [-normal[0], 0.0, -normal[1]],
    );
    quad(
        out,
        p(s3[0], y0, s3[1]),
        p(s0[0], y0, s0[1]),
        p(s0[0], y1, s0[1]),
        p(s3[0], y1, s3[1]),
        [-dir[0], 0.0, -dir[1]],
    );
    quad(
        out,
        p(s1[0], y0, s1[1]),
        p(s2[0], y0, s2[1]),
        p(s2[0], y1, s2[1]),
        p(s1[0], y1, s1[1]),
        [dir[0], 0.0, dir[1]],
    );
    quad(
        out,
        p(s0[0], y1, s0[1]),
        p(s1[0], y1, s1[1]),
        p(s2[0], y1, s2[1]),
        p(s3[0], y1, s3[1]),
        [0.0, 1.0, 0.0],
    );
    quad(
        out,
        p(s3[0], y0, s3[1]),
        p(s2[0], y0, s2[1]),
        p(s1[0], y0, s1[1]),
        p(s0[0], y0, s0[1]),
        [0.0, -1.0, 0.0],
    );
}

/// Room polygons triangulated into a floor plane at y = 0.
/// Pattern id understood by the 3D shader (see `floor_pattern` in render3d).
fn floor_pattern(kind: &str) -> f32 {
    let k = kind.to_ascii_lowercase();
    if k.contains("walnut") {
        1.0
    } else if k.contains("wood") || k.contains("oak") {
        2.0
    } else if k.contains("marble") {
        3.0
    } else if k.contains("carpet") {
        4.0
    } else if k.contains("terrazzo") {
        5.0
    } else if k.contains("tile") || k.contains("ceramic") || k.contains("antislip") {
        6.0
    } else if k.contains("stone") {
        7.0
    } else {
        0.0
    }
}

pub fn floor_mesh(rooms: &[Room], materials: &[Material]) -> Vec<MeshVertex> {
    let mut out = Vec::new();
    for room in rooms {
        let material = room
            .material_id
            .and_then(|id| materials.iter().find(|m| m.id == id));
        let color = material
            .map(|m| material_kind_color(&m.kind))
            .unwrap_or(FLOOR_FALLBACK);
        let pattern = material.map(|m| floor_pattern(&m.kind)).unwrap_or(0.0);
        for tri in fp_geom::triangulate(&room.polygon) {
            let a = to_m(tri[0]);
            let b = to_m(tri[1]);
            let c = to_m(tri[2]);
            for v in [a, b, c] {
                out.push(MeshVertex::with_pattern(
                    [v[0], 0.0, v[1]],
                    [0.0, 1.0, 0.0],
                    color,
                    [0.9, 0.0, 0.0],
                    pattern,
                ));
            }
        }
    }
    out
}

/// White ceiling over every room, so a first-person view is not open to the sky.
pub const CEILING_COLOR: [f32; 3] = [0.96, 0.95, 0.92];

pub fn ceiling_mesh(rooms: &[Room], height: f32) -> Vec<MeshVertex> {
    let mut out = Vec::new();
    for room in rooms {
        for tri in fp_geom::triangulate(&room.polygon) {
            let a = to_m(tri[0]);
            let b = to_m(tri[1]);
            let c = to_m(tri[2]);
            for v in [a, c, b] {
                out.push(MeshVertex::new(
                    [v[0], height, v[1]],
                    [0.0, -1.0, 0.0],
                    CEILING_COLOR,
                ));
            }
        }
    }
    out
}

/// Furniture footprints extruded to boxes, honouring rotation and catalogue height.
pub fn furniture_mesh(items: &[Furniture]) -> Vec<MeshVertex> {
    super::furniture::furniture_mesh(items)
}

/// Furniture mesh already split into opaque + glass ranges.
pub fn furniture_mesh_parts(items: &[Furniture]) -> MeshParts {
    super::furniture::furniture_mesh_parts(items)
}

/// A door leaf hinged at the opening's start edge, swung `open_deg`
/// towards the wall normal (0 = flush with the wall).
pub fn door_leaf_mesh(wall: &Wall, opening: &Opening, open_deg: f32) -> Vec<MeshVertex> {
    let len = ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
    if len < 1.0 || opening.width_mm <= 0 || opening.height_mm <= 0 {
        return Vec::new();
    }
    let dir = [
        (wall.end.x - wall.start.x) as f32 / len as f32,
        (wall.end.y - wall.start.y) as f32 / len as f32,
    ];
    let a = [wall.start.x as f32 / 1000.0, wall.start.y as f32 / 1000.0];
    let t_hinge = if opening.hinge_right {
        (opening.offset_mm + opening.width_mm) as f32 / 1000.0
    } else {
        opening.offset_mm as f32 / 1000.0
    };
    let hinge = [a[0] + dir[0] * t_hinge, a[1] + dir[1] * t_hinge];
    let n = [-dir[1], dir[0]];
    let (sin, cos) = open_deg.to_radians().sin_cos();
    let leaf = if opening.hinge_right { -1.0 } else { 1.0 };
    let u = [
        (dir[0] * cos + n[0] * sin) * leaf,
        (dir[1] * cos + n[1] * sin) * leaf,
    ];
    let m = [-u[1], u[0]];
    let w = opening.width_mm as f32 / 1000.0;
    let y0 = opening.sill_mm as f32 / 1000.0;
    let y1 = y0 + opening.height_mm as f32 / 1000.0;
    let half = 0.022;
    let color = [0.70, 0.56, 0.40];

    let c = |du: f32, dm: f32, y: f32| {
        [
            hinge[0] + u[0] * du + m[0] * dm,
            y,
            hinge[1] + u[1] * du + m[1] * dm,
        ]
    };
    let mut out = Vec::new();
    let mut face = |a: [f32; 3], b: [f32; 3], c2: [f32; 3], d: [f32; 3], n: [f32; 3]| {
        for pos in [a, b, c2, a, c2, d] {
            out.push(MeshVertex::new(pos, n, color));
        }
    };
    face(
        c(0.0, half, y0),
        c(w, half, y0),
        c(w, half, y1),
        c(0.0, half, y1),
        [m[0], 0.0, m[1]],
    );
    face(
        c(w, -half, y0),
        c(0.0, -half, y0),
        c(0.0, -half, y1),
        c(w, -half, y1),
        [-m[0], 0.0, -m[1]],
    );
    face(
        c(0.0, -half, y0),
        c(0.0, half, y0),
        c(0.0, half, y1),
        c(0.0, -half, y1),
        [-u[0], 0.0, -u[1]],
    );
    face(
        c(w, half, y0),
        c(w, -half, y0),
        c(w, -half, y1),
        c(w, half, y1),
        [u[0], 0.0, u[1]],
    );
    face(
        c(0.0, -half, y1),
        c(w, -half, y1),
        c(w, half, y1),
        c(0.0, half, y1),
        [0.0, 1.0, 0.0],
    );
    face(
        c(0.0, half, y0),
        c(w, half, y0),
        c(w, -half, y0),
        c(0.0, -half, y0),
        [0.0, -1.0, 0.0],
    );
    out
}

/// Walls + floors in one buffer.
pub fn scene_mesh(walls: &[Wall], rooms: &[Room]) -> Vec<MeshVertex> {
    scene_mesh_full(walls, &[], rooms, &[], &[]).vertices
}

/// Walls (with openings cut) + floors.
pub fn scene_mesh_with_openings(
    walls: &[Wall],
    openings: &[Opening],
    rooms: &[Room],
) -> Vec<MeshVertex> {
    scene_mesh_full(walls, openings, rooms, &[], &[]).vertices
}

/// Everything the 3D view draws.
pub fn scene_mesh_full(
    walls: &[Wall],
    openings: &[Opening],
    rooms: &[Room],
    materials: &[Material],
    furniture: &[Furniture],
) -> MeshParts {
    let mut scene = MeshParts::new();
    scene.push_opaque(ground_mesh());
    scene.push_opaque(wall_mesh_with_openings(walls, openings));
    scene.push_opaque(floor_mesh(rooms, materials));
    scene.push_mixed(furniture_mesh_parts(furniture));
    scene.push_glass(opening_glass_mesh(walls, openings));
    scene
}

/// Thin glass panes across every window-like opening.
pub fn opening_glass_mesh(walls: &[Wall], openings: &[Opening]) -> Vec<MeshVertex> {
    let color = [0.62, 0.76, 0.80];
    let mat = [0.04, 0.0, 0.0];
    let alpha = 0.16;
    let mut out = Vec::new();
    for opening in openings {
        if opening.kind == OpeningKind::Door {
            continue;
        }
        let Some(wall) = walls.iter().find(|w| w.id == opening.wall_id) else {
            continue;
        };
        let len =
            ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64) / 1000.0;
        if len < 1e-4 {
            continue;
        }
        let dir = [
            (wall.end.x - wall.start.x) as f32 / 1000.0 / len as f32,
            (wall.end.y - wall.start.y) as f32 / 1000.0 / len as f32,
        ];
        let a = [wall.start.x as f32 / 1000.0, wall.start.y as f32 / 1000.0];
        let s = opening.offset_mm as f32 / 1000.0;
        let e = (opening.offset_mm + opening.width_mm) as f32 / 1000.0;
        let y0 = opening.sill_mm as f32 / 1000.0;
        let y1 = (opening.sill_mm + opening.height_mm) as f32 / 1000.0;
        let p = |t: f32, y: f32| [a[0] + dir[0] * t, y, a[1] + dir[1] * t];
        let n = [-dir[1], 0.0, dir[0]];
        let a0 = p(s, y0);
        let a1 = p(e, y0);
        let b1 = p(e, y1);
        let b0 = p(s, y1);
        // One pane, not two coincident quads: the shader flips the normal by
        // `front_facing`, so both sides stay lit and refracted correctly.
        for v in [a0, a1, b1, a0, b1, b0] {
            let mut vert = MeshVertex::with_alpha(v, n, color, mat, alpha);
            vert.pattern = 8.008;
            out.push(vert);
        }
    }
    out
}

fn to_m(p: Pt) -> [f32; 2] {
    [p.x as f32 / 1000.0, p.y as f32 / 1000.0]
}

fn quad(
    out: &mut Vec<MeshVertex>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
    n: [f32; 3],
) {
    let color = WALL_COLOR;
    out.push(MeshVertex::new(a, n, color));
    out.push(MeshVertex::new(b, n, color));
    out.push(MeshVertex::new(c, n, color));
    out.push(MeshVertex::new(a, n, color));
    out.push(MeshVertex::new(c, n, color));
    out.push(MeshVertex::new(d, n, color));
}

#[cfg(test)]
mod tests {
    use fp_core::{EntityId, Pt};
    use fp_model::{Opening, OpeningKind, Room, Wall, WallKind};

    use super::*;

    fn wall() -> Wall {
        Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(2000, 0),
            240,
            WallKind::Exterior,
        )
    }

    fn room() -> Room {
        Room {
            id: EntityId(2),
            polygon: vec![
                Pt::new(0, 0),
                Pt::new(4000, 0),
                Pt::new(4000, 3000),
                Pt::new(0, 3000),
            ],
            name: "客厅".into(),
            material_id: None,
            counted: true,
        }
    }

    #[test]
    fn wall_box_has_twelve_triangles() {
        let mesh = wall_mesh(&[wall()]);
        assert_eq!(mesh.len(), 36);
        let xs: Vec<f32> = mesh.iter().map(|v| v.pos[0]).collect();
        assert!(xs.iter().cloned().fold(f32::MIN, f32::max) >= 2.0);
        let zs: Vec<f32> = mesh.iter().map(|v| v.pos[2]).collect();
        assert!(
            (zs.iter().cloned().fold(f32::MAX, f32::min) + 0.12).abs() < 1e-5,
            "{:?}",
            zs.iter().cloned().fold(f32::MAX, f32::min)
        );
        assert!(mesh.iter().all(|v| v.color == WALL_COLOR));
    }

    #[test]
    fn door_cuts_a_hole_and_keeps_a_header() {
        let o = Opening {
            id: EntityId(10),
            wall_id: EntityId(1),
            kind: OpeningKind::Door,
            offset_mm: 500,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 90.0,
            hinge_right: false,
        };
        let mesh = wall_mesh_with_openings(&[wall()], &[o]);
        assert_eq!(mesh.len(), 3 * 36, "two runs + header");
    }

    #[test]
    fn window_keeps_sill_and_lintel() {
        let o = Opening {
            id: EntityId(11),
            wall_id: EntityId(1),
            kind: OpeningKind::Window,
            offset_mm: 500,
            width_mm: 1200,
            height_mm: 1500,
            sill_mm: 900,
            swing_deg: 0.0,
            hinge_right: false,
        };
        let mesh = wall_mesh_with_openings(&[wall()], &[o]);
        assert_eq!(mesh.len(), 4 * 36, "two runs + sill + lintel");
    }

    #[test]
    fn floor_triangles_cover_the_room() {
        let mesh = floor_mesh(&[room()], &[]);
        assert_eq!(mesh.len() % 3, 0);
        let area: f32 = mesh
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let (a, b, c) = (t[0].pos, t[1].pos, t[2].pos);
                ((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs() * 0.5
            })
            .sum();
        assert!((area - 12.0).abs() < 0.01, "area {area}");
    }

    #[test]
    fn floor_colour_follows_material() {
        let mut r = room();
        r.material_id = Some(EntityId(2001));
        let mats = vec![Material {
            id: EntityId(2001),
            name: "橡木".into(),
            price_per_m2: 0.0,
            kind: "wood".into(),
        }];
        let mesh = floor_mesh(&[r.clone()], &mats);
        assert!(mesh.iter().all(|v| v.color != FLOOR_FALLBACK));
        let plain = floor_mesh(&[room()], &[]);
        assert!(plain.iter().all(|v| v.color == FLOOR_FALLBACK));
    }

    #[test]
    fn furniture_box_is_rotated_and_capped_at_catalogue_height() {
        let f = Furniture {
            id: EntityId(9),
            asset_id: "sofa".into(),
            name: "沙发".into(),
            center: Pt::new(1000, 1000),
            width_mm: 2200,
            depth_mm: 900,
            rotation_deg: 0.0,
            color: "#8b6f55".into(),
            room_id: None,
        };
        let mesh = furniture_mesh(std::slice::from_ref(&f));
        assert!(
            mesh.len() > 1000,
            "sofa is built from many solids, got {}",
            mesh.len()
        );
        let top = mesh.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
        assert!(
            (0.75..0.9).contains(&top),
            "sofa height in range, got {top}"
        );
        assert!(mesh.iter().all(|v| v.color != FLOOR_FALLBACK));
    }

    #[test]
    fn door_leaf_swings_around_the_hinge() {
        let o = Opening {
            id: EntityId(3),
            wall_id: EntityId(1),
            kind: OpeningKind::Door,
            offset_mm: 500,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 90.0,
            hinge_right: false,
        };
        let closed = door_leaf_mesh(&wall(), &o, 0.0);
        assert_eq!(closed.len(), 36, "one box, six faces");
        let closed_z = closed.iter().map(|v| v.pos[2]).fold(f32::MIN, f32::max);
        assert!(
            closed_z.abs() < 0.05,
            "closed leaf sits in the wall: {closed_z}"
        );
        let open = door_leaf_mesh(&wall(), &o, 78.0);
        let spread = open.iter().map(|v| v.pos[2].abs()).fold(f32::MIN, f32::max);
        assert!(spread > 0.5, "swung leaf sticks out: {spread}");
        let top = open.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
        assert!((top - 2.1).abs() < 1e-4, "leaf is 2.1 m tall, got {top}");
    }

    #[test]
    fn negative_door_swing_flips_the_side() {
        let o = Opening {
            id: EntityId(3),
            wall_id: EntityId(1),
            kind: OpeningKind::Door,
            offset_mm: 500,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: -90.0,
            hinge_right: false,
        };
        let forward = door_leaf_mesh(&wall(), &o, 78.0);
        let reverse = door_leaf_mesh(&wall(), &o, -78.0);
        let f_max = forward.iter().map(|v| v.pos[2]).fold(f32::MIN, f32::max);
        let r_min = reverse.iter().map(|v| v.pos[2]).fold(f32::MAX, f32::min);
        assert!(f_max > 0.5, "forward leaf is on +z: {f_max}");
        assert!(r_min < -0.5, "reverse leaf is on -z: {r_min}");
    }

    #[test]
    fn right_hinge_swings_from_the_other_jamb() {
        let mut o = Opening {
            id: EntityId(3),
            wall_id: EntityId(1),
            kind: OpeningKind::Door,
            offset_mm: 500,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 90.0,
            hinge_right: true,
        };
        let open = door_leaf_mesh(&wall(), &o, 78.0);
        let min_x = open.iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert!(min_x > 1.1, "right hinge stays near x=1.4m: {min_x}");
        o.hinge_right = false;
        let left = door_leaf_mesh(&wall(), &o, 78.0);
        let max_x = left.iter().map(|v| v.pos[0]).fold(f32::MIN, f32::max);
        assert!(max_x < 0.8, "left hinge stays near x=0.5m: {max_x}");
    }

    #[test]
    fn window_glass_carries_optical_marker() {
        let o = Opening {
            id: EntityId(12),
            wall_id: EntityId(1),
            kind: OpeningKind::Window,
            offset_mm: 500,
            width_mm: 1200,
            height_mm: 1500,
            sill_mm: 900,
            swing_deg: 0.0,
            hinge_right: false,
        };
        let glass = opening_glass_mesh(&[wall()], &[o]);
        assert_eq!(glass.len(), 6, "one pane per window");
        assert!(glass.iter().all(|v| v.alpha < 0.2));
        assert!(glass.iter().all(|v| (v.pattern - 8.008).abs() < 1e-5));
    }

    #[test]
    fn normals_point_up_for_floors() {
        for v in floor_mesh(&[room()], &[]) {
            assert_eq!(v.normal, [0.0, 1.0, 0.0]);
        }
    }
}
