//! 3D mesh generation: walls extruded to boxes, rooms flattened to floors,
//! furniture as rotated boxes.
//!
//! Coordinates are metres with Y up, matching the renderer's NDC convention.

use fp_core::Pt;
use fp_model::catalog::{finish_pattern, finish_roughness, material_color};
use fp_model::{
    Furniture, Material, Opening, OpeningKind, RoofKind, Room, Stair, Wall, WallFinish,
};

/// Default surface colours, overridable per entity.
pub const WALL_COLOR: [f32; 3] = [0.80, 0.78, 0.74];
pub const WALL_WALLPAPER: [f32; 3] = [0.74, 0.70, 0.64];
pub const WALL_WOOD: [f32; 3] = [0.52, 0.40, 0.29];
pub const WALL_TILE: [f32; 3] = [0.74, 0.75, 0.73];
type SurfaceStyle = ([f32; 3], [f32; 3], f32);

fn wall_style(finish: WallFinish) -> SurfaceStyle {
    match finish {
        WallFinish::Paint => (WALL_COLOR, [1.0, 0.0, 0.0], 0.0),
        WallFinish::Wallpaper => (WALL_WALLPAPER, [0.88, 0.0, 0.0], 16.0),
        WallFinish::WoodPanel => (WALL_WOOD, [0.58, 0.0, 0.0], 11.0),
        WallFinish::Tile => (WALL_TILE, [0.28, 0.0, 0.0], 6.0),
    }
}

fn material_surface_style(material: &Material) -> SurfaceStyle {
    (
        material_color(material),
        [finish_roughness(&material.kind), 0.0, 0.0],
        finish_pattern(&material.kind),
    )
}

fn wall_surface_style(wall: &Wall, materials: &[Material]) -> SurfaceStyle {
    wall.finish_id
        .and_then(|id| materials.iter().find(|m| m.id == id))
        .map(material_surface_style)
        .unwrap_or_else(|| wall_style(wall.finish))
}
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
    // The old 80 m slab ended inside the overview camera and drew a hard
    // horizon line across the middle of the screen. A kilometre-scale slab
    // pushes that edge far beyond the visible range.
    let h = 1000.0f32;
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

/// Extrude walls, cutting door and window openings. Wall colours, patterns and
/// roughness come from the document's wall finishes; walls without a
/// `finish_id` keep the historical enum style.
pub fn wall_mesh_with_openings(walls: &[Wall], openings: &[Opening]) -> Vec<MeshVertex> {
    wall_mesh_with_finishes(walls, openings, &[])
}

pub fn wall_mesh_with_finishes(
    walls: &[Wall],
    openings: &[Opening],
    materials: &[Material],
) -> Vec<MeshVertex> {
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

        let style = wall_surface_style(wall, materials);
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
                box_segment_styled(&mut out, point_at(cursor), point_at(s), half, 0.0, h, style);
            }
            cursor = cursor.max(e);
        }
        if cursor < len_m as f32 {
            box_segment_styled(
                &mut out,
                point_at(cursor),
                point_at(len_m as f32),
                half,
                0.0,
                h,
                style,
            );
        }

        for (s, e, sill, head) in &holes {
            if *sill > 0.0 {
                box_segment_styled(
                    &mut out,
                    point_at(*s),
                    point_at(*e),
                    half,
                    0.0,
                    *sill,
                    style,
                );
            }
            if *head < h {
                box_segment_styled(&mut out, point_at(*s), point_at(*e), half, *head, h, style);
            }
        }
    }
    out
}

fn box_segment(out: &mut Vec<MeshVertex>, a: [f32; 2], b: [f32; 2], half: f32, y0: f32, y1: f32) {
    box_segment_styled(out, a, b, half, y0, y1, wall_style(WallFinish::Paint));
}

fn box_segment_styled(
    out: &mut Vec<MeshVertex>,
    a: [f32; 2],
    b: [f32; 2],
    half: f32,
    y0: f32,
    y1: f32,
    style: SurfaceStyle,
) {
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
        style,
    );
    quad(
        out,
        p(s2[0], y0, s2[1]),
        p(s3[0], y0, s3[1]),
        p(s3[0], y1, s3[1]),
        p(s2[0], y1, s2[1]),
        [-normal[0], 0.0, -normal[1]],
        style,
    );
    quad(
        out,
        p(s3[0], y0, s3[1]),
        p(s0[0], y0, s0[1]),
        p(s0[0], y1, s0[1]),
        p(s3[0], y1, s3[1]),
        [-dir[0], 0.0, -dir[1]],
        style,
    );
    quad(
        out,
        p(s1[0], y0, s1[1]),
        p(s2[0], y0, s2[1]),
        p(s2[0], y1, s2[1]),
        p(s1[0], y1, s1[1]),
        [dir[0], 0.0, dir[1]],
        style,
    );
    quad(
        out,
        p(s0[0], y1, s0[1]),
        p(s1[0], y1, s1[1]),
        p(s2[0], y1, s2[1]),
        p(s3[0], y1, s3[1]),
        [0.0, 1.0, 0.0],
        style,
    );
    quad(
        out,
        p(s3[0], y0, s3[1]),
        p(s2[0], y0, s2[1]),
        p(s1[0], y0, s1[1]),
        p(s0[0], y0, s0[1]),
        [0.0, -1.0, 0.0],
        style,
    );
}

/// Room polygons triangulated into a floor plane at y = 0.
pub fn floor_mesh(rooms: &[Room], materials: &[Material]) -> Vec<MeshVertex> {
    let mut out = Vec::new();
    for room in rooms {
        let material = room
            .material_id
            .and_then(|id| materials.iter().find(|m| m.id == id));
        let color = material.map(material_color).unwrap_or(FLOOR_FALLBACK);
        let pattern = material.map(|m| finish_pattern(&m.kind)).unwrap_or(0.0);
        let roughness = material.map(|m| finish_roughness(&m.kind)).unwrap_or(0.9);
        for tri in fp_geom::triangulate(&room.polygon) {
            let a = to_m(tri[0]);
            let b = to_m(tri[1]);
            let c = to_m(tri[2]);
            for v in [a, b, c] {
                out.push(MeshVertex::with_pattern(
                    [v[0], 0.0, v[1]],
                    [0.0, 1.0, 0.0],
                    color,
                    [roughness, 0.0, 0.0],
                    pattern,
                ));
            }
        }
    }
    out
}

/// White ceiling over every room, so a first-person view is not open to the sky.
pub const CEILING_COLOR: [f32; 3] = [0.82, 0.81, 0.78];

pub fn ceiling_mesh(rooms: &[Room], height: f32) -> Vec<MeshVertex> {
    ceiling_mesh_with_materials(rooms, height, &[])
}

pub fn ceiling_mesh_with_materials(
    rooms: &[Room],
    height: f32,
    materials: &[Material],
) -> Vec<MeshVertex> {
    let mut out = Vec::new();
    for room in rooms {
        let (color, mat, pattern) = room
            .ceiling_material_id
            .and_then(|id| materials.iter().find(|m| m.id == id))
            .map(material_surface_style)
            .unwrap_or((CEILING_COLOR, [0.95, 0.0, 0.0], 0.0));
        for tri in fp_geom::triangulate(&room.polygon) {
            let a = to_m(tri[0]);
            let b = to_m(tri[1]);
            let c = to_m(tri[2]);
            for v in [a, c, b] {
                out.push(MeshVertex::with_pattern(
                    [v[0], height, v[1]],
                    [0.0, -1.0, 0.0],
                    color,
                    mat,
                    pattern,
                ));
            }
        }
    }
    out
}

/// Flat roof slab over the top storey's rooms. Two-sided so it reads from both
/// the dollhouse view above and the first-person view below.
pub fn roof_mesh(rooms: &[Room], height: f32) -> Vec<MeshVertex> {
    const ROOF_COLOR: [f32; 3] = [0.52, 0.48, 0.43];
    const ROOF_TOP: [f32; 3] = [0.61, 0.57, 0.51];
    const THICKNESS: f32 = 0.12;
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
                    ROOF_COLOR,
                ));
            }
            for v in [a, b, c] {
                out.push(roof_vertex(
                    [v[0], height + THICKNESS, v[1]],
                    [0.0, 1.0, 0.0],
                    ROOF_TOP,
                ));
            }
        }
    }
    out
}

/// Roof over the top storey. Flat roofs are one slab; gable roofs get two
/// pitched planes, gable-end triangles and a fascia band around the eave.
/// The footprint is the bounding rectangle of the top-floor rooms, expanded
/// by the eave overhang.
fn roof_components(rooms: &[Room]) -> Vec<Vec<Pt>> {
    let polys: Vec<Vec<Pt>> = rooms
        .iter()
        .filter(|room| room.polygon.len() >= 3)
        .map(|room| room.polygon.clone())
        .collect();
    let merged = fp_geom::union_all(&polys);
    if merged.len() <= 1 {
        return merged;
    }
    // Interior room polygons stop at the inner wall face. A 150 mm join closes
    // the wall-thickness gaps between rooms, then one boolean pass merges them
    // back into the building envelope.
    let joined: Vec<Vec<Pt>> = merged
        .iter()
        .flat_map(|part| expand_polygon(part, 150.0))
        .collect();
    fp_geom::union_all(&joined)
}

/// Offset a simple polygon outward by `over` millimetres. Each edge grows a
/// thin outward strip; unioning those strips yields square/mitered corners and
/// keeps concave plan outlines (L-shaped apartments) intact.
fn expand_polygon(poly: &[Pt], over: f32) -> Vec<Vec<Pt>> {
    if over <= 0.0 || poly.len() < 3 {
        return vec![poly.to_vec()];
    }
    let ccw = fp_geom::signed_area(poly) > 0.0;
    let mut parts = vec![poly.to_vec()];
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let dx = b.x as f32 - a.x as f32;
        let dy = b.y as f32 - a.y as f32;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1.0 {
            continue;
        }
        let (nx, ny) = if ccw {
            (dy / len, -dx / len)
        } else {
            (-dy / len, dx / len)
        };
        let offset = |p: Pt| {
            Pt::new(
                (p.x as f32 + nx * over).round() as i32,
                (p.y as f32 + ny * over).round() as i32,
            )
        };
        parts.push(vec![a, b, offset(b), offset(a)]);
    }
    fp_geom::union_all(&parts)
}

fn split_polygon(poly: &[Pt], cut_mm: f32, horizontal: bool, upper: bool) -> Vec<Vec<Pt>> {
    let (min_x, max_x, min_y, max_y) = poly.iter().fold(
        (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
        |(x0, x1, y0, y1), p| {
            (
                x0.min(p.x as f32),
                x1.max(p.x as f32),
                y0.min(p.y as f32),
                y1.max(p.y as f32),
            )
        },
    );
    let pad = 2000.0;
    let rect = if horizontal {
        if upper {
            vec![
                Pt::new((min_x - pad) as i32, cut_mm.round() as i32),
                Pt::new((max_x + pad) as i32, cut_mm.round() as i32),
                Pt::new((max_x + pad) as i32, (max_y + pad) as i32),
                Pt::new((min_x - pad) as i32, (max_y + pad) as i32),
            ]
        } else {
            vec![
                Pt::new((min_x - pad) as i32, (min_y - pad) as i32),
                Pt::new((max_x + pad) as i32, (min_y - pad) as i32),
                Pt::new((max_x + pad) as i32, cut_mm.round() as i32),
                Pt::new((min_x - pad) as i32, cut_mm.round() as i32),
            ]
        }
    } else if upper {
        vec![
            Pt::new(cut_mm.round() as i32, (min_y - pad) as i32),
            Pt::new((max_x + pad) as i32, (min_y - pad) as i32),
            Pt::new((max_x + pad) as i32, (max_y + pad) as i32),
            Pt::new(cut_mm.round() as i32, (max_y + pad) as i32),
        ]
    } else {
        vec![
            Pt::new((min_x - pad) as i32, (min_y - pad) as i32),
            Pt::new(cut_mm.round() as i32, (min_y - pad) as i32),
            Pt::new(cut_mm.round() as i32, (max_y + pad) as i32),
            Pt::new((min_x - pad) as i32, (max_y + pad) as i32),
        ]
    };
    fp_geom::intersection(poly, &rect)
}

/// Roof surface following the union of the top-floor room outlines. Gable
/// roofs are clipped at the ridge and triangulated on both slopes, so an
/// L-shaped footprint only receives roof over its actual rooms rather than
/// over the bounding rectangle.
fn roof_vertex(pos: [f32; 3], normal: [f32; 3], color: [f32; 3]) -> MeshVertex {
    let mut v = MeshVertex::new(pos, normal, color);
    // The shingle courses only belong on the top planes; gable ends, fascia
    // and soffit keep the painted/metal treatment.
    if color[0] >= 0.19 && color[1] >= 0.18 {
        v.pattern = 15.0;
    }
    v
}

pub fn roof_mesh_for(
    rooms: &[Room],
    height: f32,
    kind: RoofKind,
    pitch_deg: f32,
    overhang_mm: i32,
) -> Vec<MeshVertex> {
    const TOP: [f32; 3] = [0.20, 0.19, 0.18];
    const GABLE: [f32; 3] = [0.19, 0.18, 0.17];
    const FASCIA: [f32; 3] = [0.12, 0.115, 0.105];
    const SOFFIT: [f32; 3] = [0.10, 0.095, 0.088];
    let components = roof_components(rooms);
    if components.is_empty() {
        return Vec::new();
    }
    let over = overhang_mm.max(0) as f32;
    let pitch = pitch_deg.clamp(10.0, 60.0).to_radians();
    let tan = pitch.tan();
    let thickness = 0.16;
    let low = height - thickness;
    let mut out = Vec::new();

    fn roof_face(
        out: &mut Vec<MeshVertex>,
        p0: [f32; 3],
        p1: [f32; 3],
        p2: [f32; 3],
        p3: [f32; 3],
        color: [f32; 3],
    ) {
        let a = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let b = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let mut n = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 1e-6 {
            n = [n[0] / len, n[1] / len, n[2] / len];
        }
        if n[1] < 0.0 {
            n = [-n[0], -n[1], -n[2]];
        }
        for v in [p0, p1, p2, p0, p2, p3] {
            out.push(roof_vertex(v, n, color));
        }
    }
    fn roof_tri(
        out: &mut Vec<MeshVertex>,
        p0: [f32; 3],
        p1: [f32; 3],
        p2: [f32; 3],
        color: [f32; 3],
    ) {
        let a = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let b = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let mut n = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 1e-6 {
            n = [n[0] / len, n[1] / len, n[2] / len];
        }
        if n[1] < 0.0 {
            n = [-n[0], -n[1], -n[2]];
        }
        for v in [p0, p1, p2] {
            out.push(roof_vertex(v, n, color));
        }
    }

    for component in components {
        let expanded = expand_polygon(&component, over);
        for poly in expanded {
            if poly.len() < 3 {
                continue;
            }
            let (min_x, max_x, min_y, max_y) = poly.iter().fold(
                (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
                |(x0, x1, y0, y1), p| {
                    (
                        x0.min(p.x as f32 / 1000.0),
                        x1.max(p.x as f32 / 1000.0),
                        y0.min(p.y as f32 / 1000.0),
                        y1.max(p.y as f32 / 1000.0),
                    )
                },
            );
            let span_x = (max_x - min_x).max(0.1);
            let span_y = (max_y - min_y).max(0.1);
            let ridge_along_x = span_x >= span_y;
            let rise = if kind == RoofKind::Gable {
                (if ridge_along_x { span_y } else { span_x }) * 0.5 * tan
            } else {
                0.0
            };
            let ridge_mm = if ridge_along_x {
                (min_y + span_y * 0.5) * 1000.0
            } else {
                (min_x + span_x * 0.5) * 1000.0
            };
            let roof_height = |p: &Pt| -> f32 {
                if kind == RoofKind::Flat {
                    return height;
                }
                let x = p.x as f32 / 1000.0;
                let y = p.y as f32 / 1000.0;
                let run = if ridge_along_x {
                    (y - min_y).min(max_y - y)
                } else {
                    (x - min_x).min(max_x - x)
                };
                height + run.max(0.0) * tan
            };

            // Top surface, split at the ridge so each triangle lies on one
            // slope plane rather than interpolating across the ridge.
            let top_parts = if kind == RoofKind::Flat {
                vec![poly.clone()]
            } else if ridge_along_x {
                let south = split_polygon(&poly, ridge_mm, true, false);
                let north = split_polygon(&poly, ridge_mm, true, true);
                let mut parts = south;
                parts.extend(north);
                parts
            } else {
                let west = split_polygon(&poly, ridge_mm, false, false);
                let east = split_polygon(&poly, ridge_mm, false, true);
                let mut parts = west;
                parts.extend(east);
                parts
            };
            for part in top_parts {
                for tri in fp_geom::triangulate(&part) {
                    let p = [
                        [
                            tri[0].x as f32 / 1000.0,
                            roof_height(&tri[0]),
                            tri[0].y as f32 / 1000.0,
                        ],
                        [
                            tri[1].x as f32 / 1000.0,
                            roof_height(&tri[1]),
                            tri[1].y as f32 / 1000.0,
                        ],
                        [
                            tri[2].x as f32 / 1000.0,
                            roof_height(&tri[2]),
                            tri[2].y as f32 / 1000.0,
                        ],
                    ];
                    roof_tri(&mut out, p[0], p[1], p[2], TOP);
                }
            }

            // Flat soffit under the roof, closing the eave band.
            for tri in fp_geom::triangulate(&poly) {
                let p = [
                    [tri[0].x as f32 / 1000.0, low, tri[0].y as f32 / 1000.0],
                    [tri[1].x as f32 / 1000.0, low, tri[1].y as f32 / 1000.0],
                    [tri[2].x as f32 / 1000.0, low, tri[2].y as f32 / 1000.0],
                ];
                roof_tri(&mut out, p[0], p[1], p[2], SOFFIT);
            }

            // Perimeter fascia follows the plan outline, including the
            // re-entrant edges of an L-shaped apartment.
            for i in 0..poly.len() {
                let a = poly[i];
                let b = poly[(i + 1) % poly.len()];
                let ax = a.x as f32 / 1000.0;
                let az = a.y as f32 / 1000.0;
                let bx = b.x as f32 / 1000.0;
                let bz = b.y as f32 / 1000.0;
                let ha = roof_height(&a);
                let hb = roof_height(&b);
                let edge_vertical = (bx - ax).abs() < (bz - az).abs();
                let gable_end = if ridge_along_x {
                    edge_vertical
                        && ((ax <= min_x + 0.001 && bx <= min_x + 0.001)
                            || (ax >= max_x - 0.001 && bx >= max_x - 0.001))
                } else {
                    !edge_vertical
                        && ((az <= min_y + 0.001 && bz <= min_y + 0.001)
                            || (az >= max_y - 0.001 && bz >= max_y - 0.001))
                };
                roof_face(
                    &mut out,
                    [ax, low, az],
                    [bx, low, bz],
                    [bx, hb, bz],
                    [ax, ha, az],
                    if kind == RoofKind::Gable && gable_end {
                        GABLE
                    } else {
                        FASCIA
                    },
                );
            }
            let _ = rise;
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
            let mut v = MeshVertex::new(pos, n, color);
            v.pattern = 11.0;
            out.push(v);
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
    scene.push_opaque(wall_mesh_with_finishes(walls, openings, materials));
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

/// A straight stair run from its plan footprint, rising `rise` metres.
/// Steps are stacked solid boxes, which reads well in both orbit and cutaway.
pub fn stair_mesh(stair: &Stair, rise: f32) -> Vec<MeshVertex> {
    let a = [stair.start.x as f32 / 1000.0, stair.start.y as f32 / 1000.0];
    let b = [stair.end.x as f32 / 1000.0, stair.end.y as f32 / 1000.0];
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 0.2 || rise <= 0.1 {
        return Vec::new();
    }
    let steps = stair.steps.clamp(3, 60);
    let half = stair.width_mm.max(300) as f32 / 2000.0;
    let mut out = Vec::new();
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let t1 = (i + 1) as f32 / steps as f32;
        let p0 = [a[0] + d[0] * t0, a[1] + d[1] * t0];
        let p1 = [a[0] + d[0] * t1, a[1] + d[1] * t1];
        let y0 = rise * i as f32 / steps as f32;
        let y1 = rise * (i + 1) as f32 / steps as f32;
        box_segment(&mut out, p0, p1, half, y0, y1);
    }
    let color = [0.52, 0.42, 0.32];
    for v in &mut out {
        v.color = color;
        v.mat = [0.62, 0.0, 0.0];
        v.pattern = 11.0;
    }
    out
}

fn quad(
    out: &mut Vec<MeshVertex>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
    n: [f32; 3],
    style: SurfaceStyle,
) {
    let (color, mat, pattern) = style;
    for p in [a, b, c, a, c, d] {
        out.push(MeshVertex::with_pattern(p, n, color, mat, pattern));
    }
}

#[cfg(test)]
mod tests {
    use fp_core::{EntityId, Pt};
    use fp_model::{FinishSurface, Material, Opening, OpeningKind, Room, Wall, WallFinish, WallKind};

    use super::*;

    fn room_with_polygon(points: Vec<(i32, i32)>) -> Room {
        Room {
            id: EntityId(10),
            polygon: points.into_iter().map(|(x, y)| Pt::new(x, y)).collect(),
            name: "房间".into(),
            material_id: None,
            counted: true,
            floor: 0,
            ceiling_material_id: None,
        }
    }

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
            floor: 0,
            ceiling_material_id: None,
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
            price: None,
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
            price: None,
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

    fn finish(id: u128, surface: FinishSurface, kind: &str, color: &str) -> Material {
        Material {
            id: EntityId(id),
            name: kind.into(),
            price_per_m2: 0.0,
            kind: kind.into(),
            color: color.into(),
            surface,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
        }
    }

    #[test]
    fn wall_finish_record_overrides_legacy_enum() {
        let mut w = wall();
        w.finish = WallFinish::Tile;
        w.finish_id = Some(EntityId(42));
        let paint = finish(42, FinishSurface::Wall, "paint", "#ff0000");
        let mesh = wall_mesh_with_finishes(&[w.clone()], &[], &[paint]);
        assert!(mesh.iter().all(|v| v.color == [1.0, 0.0, 0.0]));
        assert!(mesh.iter().all(|v| v.pattern == 0.0));
        // Legacy enum still drives walls without a finish id.
        let legacy = wall_mesh(&[w]);
        assert!(legacy.iter().all(|v| v.pattern == 6.0));
    }

    #[test]
    fn one_storey_can_mix_three_wall_finishes() {
        let ids = [1u128, 2, 3];
        let kinds = ["paint", "wood_panel", "tile"];
        let surfaces = [FinishSurface::Wall; 3];
        let materials: Vec<Material> = ids
            .iter()
            .zip(kinds)
            .zip(surfaces)
            .map(|((id, kind), surface)| finish(*id, surface, kind, ""))
            .collect();
        let mut walls = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            let mut w = wall();
            w.id = EntityId(*id);
            w.start = Pt::new(index as i32 * 2000, 0);
            w.end = Pt::new((index as i32 + 1) * 2000, 0);
            w.floor = 0;
            w.finish_id = Some(EntityId(*id));
            walls.push(w);
        }
        let mesh = wall_mesh_with_finishes(&walls, &[], &materials);
        let patterns: Vec<f32> = mesh.iter().map(|v| v.pattern).collect();
        for wanted in [0.0, 11.0, 6.0] {
            assert!(patterns.contains(&wanted), "missing pattern {wanted}");
        }
    }

    #[test]
    fn ceiling_finish_uses_room_record() {
        let mut r = room();
        r.ceiling_material_id = Some(EntityId(77));
        let wood = finish(77, FinishSurface::Ceiling, "wood_panel", "#00ff00");
        let mesh = ceiling_mesh_with_materials(&[r], 2.75, &[wood]);
        assert!(mesh.iter().all(|v| v.color == [0.0, 1.0, 0.0]));
        assert!(mesh.iter().all(|v| v.pattern == 11.0));
        let plain = ceiling_mesh(&[room()], 2.75);
        assert!(plain.iter().all(|v| v.color == CEILING_COLOR));
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
            color: "#c49a6c".into(),
            surface: fp_model::FinishSurface::Floor,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
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
            price: None,
            floor: 0,
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
            price: None,
        };
        let closed = door_leaf_mesh(&wall(), &o, 0.0);
        assert_eq!(closed.len(), 36, "one box, six faces");
        assert!(
            closed.iter().all(|v| (v.pattern - 11.0).abs() < 1e-5),
            "door leaf should carry the timber grain"
        );
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
            price: None,
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
            price: None,
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
            price: None,
        };
        let glass = opening_glass_mesh(&[wall()], &[o]);
        assert_eq!(glass.len(), 6, "one pane per window");
        assert!(glass.iter().all(|v| v.alpha < 0.2));
        assert!(glass.iter().all(|v| (v.pattern - 8.008).abs() < 1e-5));
    }

    #[test]
    fn gable_roof_has_a_ridge_and_eave_overhang() {
        let mesh = roof_mesh_for(&[room()], 3.0, RoofKind::Gable, 25.0, 600);
        assert!(!mesh.is_empty());
        let min_x = mesh.iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        let max_x = mesh.iter().map(|v| v.pos[0]).fold(f32::MIN, f32::max);
        let max_y = mesh.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
        assert!((min_x + 0.6).abs() < 1e-3, "eave west {min_x}");
        assert!((max_x - 4.6).abs() < 1e-3, "eave east {max_x}");
        assert!(max_y > 3.9, "ridge rises above the eave: {max_y}");
    }

    #[test]
    fn gable_roof_follows_an_l_shaped_footprint() {
        let rooms = [
            Room {
                id: EntityId(10),
                polygon: vec![
                    Pt::new(0, 0),
                    Pt::new(6000, 0),
                    Pt::new(6000, 4000),
                    Pt::new(0, 4000),
                ],
                name: "主卧".into(),
                material_id: None,
                counted: true,
                floor: 0,
                ceiling_material_id: None,
            },
            Room {
                id: EntityId(11),
                polygon: vec![
                    Pt::new(0, 4000),
                    Pt::new(3000, 4000),
                    Pt::new(3000, 7000),
                    Pt::new(0, 7000),
                ],
                name: "次卧".into(),
                material_id: None,
                counted: true,
                floor: 0,
                ceiling_material_id: None,
            },
        ];
        let mesh = roof_mesh_for(&rooms, 3.0, RoofKind::Gable, 25.0, 600);
        assert!(!mesh.is_empty());
        // The bbox is 6×7 m, but the north-east quadrant is a void. No roof
        // vertex should sit in the middle of that void.
        assert!(
            mesh.iter().all(|v| !(v.pos[0] > 3.8 && v.pos[2] > 4.8)),
            "roof crosses the L-shaped void"
        );
        let max_x = mesh.iter().map(|v| v.pos[0]).fold(f32::MIN, f32::max);
        let max_z = mesh.iter().map(|v| v.pos[2]).fold(f32::MIN, f32::max);
        assert!((max_x - 6.6).abs() < 1e-3, "eave east {max_x}");
        assert!((max_z - 7.6).abs() < 1e-3, "eave north {max_z}");
    }

    fn top_face_covers(points: &[[f32; 2]], mesh: &[MeshVertex], px: f32, pz: f32) -> bool {
        fn inside(a: [f32; 2], b: [f32; 2], c: [f32; 2], p: [f32; 2]) -> bool {
            let cross = |u: [f32; 2], v: [f32; 2], w: [f32; 2]| {
                (v[0] - u[0]) * (w[1] - u[1]) - (v[1] - u[1]) * (w[0] - u[0])
            };
            let ab = cross(a, b, p);
            let bc = cross(b, c, p);
            let ca = cross(c, a, p);
            (ab >= -1e-4 && bc >= -1e-4 && ca >= -1e-4) || (ab <= 1e-4 && bc <= 1e-4 && ca <= 1e-4)
        }
        for chunk in points.chunks(3) {
            if chunk.len() == 3 && inside(chunk[0], chunk[1], chunk[2], [px, pz]) {
                return true;
            }
        }
        let _ = mesh;
        false
    }

    #[test]
    fn roof_top_planes_use_the_shingle_pattern() {
        let rooms = [room_with_polygon(vec![
            (0, 0),
            (6000, 0),
            (6000, 4000),
            (0, 4000),
        ])];
        let mesh = roof_mesh_for(&rooms, 3.0, RoofKind::Gable, 25.0, 600);
        let tops: Vec<&MeshVertex> = mesh
            .iter()
            .filter(|v| v.color[0] >= 0.19 && v.color[1] >= 0.18)
            .collect();
        assert!(!tops.is_empty(), "roof top vertices present");
        assert!(
            tops.iter().all(|v| (v.pattern - 15.0).abs() < 1e-5),
            "top planes should carry the shingle pattern"
        );
        let fascia: Vec<&MeshVertex> = mesh.iter().filter(|v| v.color[0] < 0.19).collect();
        assert!(!fascia.is_empty(), "fascia vertices present");
        assert!(
            fascia.iter().all(|v| v.pattern < 0.5),
            "fascia should stay painted, not shingled"
        );
    }

    #[test]
    fn roof_components_close_a_wall_thickness_gap() {
        let rooms = vec![
            room_with_polygon(vec![(0, 0), (4000, 0), (4000, 4000), (0, 4000)]),
            room_with_polygon(vec![(4120, 0), (8000, 0), (8000, 4000), (4120, 4000)]),
        ];
        let parts = roof_components(&rooms);
        assert_eq!(
            parts.len(),
            1,
            "a 120 mm wall gap must read as one building"
        );
        let (min_x, max_x, min_y, max_y) = parts[0].iter().fold(
            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
            |(x0, x1, y0, y1), p| {
                (
                    x0.min(p.x as f32),
                    x1.max(p.x as f32),
                    y0.min(p.y as f32),
                    y1.max(p.y as f32),
                )
            },
        );
        assert!(min_x <= 0.0 && max_x >= 8000.0 && min_y <= 0.0 && max_y >= 4000.0);
    }

    #[test]
    fn roof_components_keep_far_apart_rooms_separate() {
        let rooms = vec![
            room_with_polygon(vec![(0, 0), (4000, 0), (4000, 4000), (0, 4000)]),
            room_with_polygon(vec![(6000, 0), (10000, 0), (10000, 4000), (6000, 4000)]),
        ];
        let parts = roof_components(&rooms);
        assert_eq!(parts.len(), 2, "rooms 2 m apart are separate buildings");
    }

    #[test]
    fn roof_top_surface_covers_l_arm_and_skips_void() {
        let rooms = [
            Room {
                id: EntityId(10),
                polygon: vec![
                    Pt::new(0, 0),
                    Pt::new(6000, 0),
                    Pt::new(6000, 4000),
                    Pt::new(0, 4000),
                ],
                name: "主卧".into(),
                material_id: None,
                counted: true,
                floor: 0,
                ceiling_material_id: None,
            },
            Room {
                id: EntityId(11),
                polygon: vec![
                    Pt::new(0, 4000),
                    Pt::new(3000, 4000),
                    Pt::new(3000, 7000),
                    Pt::new(0, 7000),
                ],
                name: "次卧".into(),
                material_id: None,
                counted: true,
                floor: 0,
                ceiling_material_id: None,
            },
        ];
        let mesh = roof_mesh_for(&rooms, 3.0, RoofKind::Gable, 25.0, 0);
        let top: Vec<[f32; 2]> = mesh
            .iter()
            .filter(|v| (v.pattern - 15.0).abs() < 1e-5 && v.normal[1] > 0.1)
            .map(|v| [v.pos[0], v.pos[2]])
            .collect();
        assert!(top_face_covers(&top, &mesh, 1.5, 5.5), "wing not covered");
        assert!(top_face_covers(&top, &mesh, 5.0, 2.0), "main not covered");
        assert!(!top_face_covers(&top, &mesh, 5.0, 5.5), "void covered");
    }

    #[test]
    fn flat_roof_stays_at_eave_height() {
        let mesh = roof_mesh_for(&[room()], 3.0, RoofKind::Flat, 45.0, 300);
        let max_y = mesh.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
        assert!((max_y - 3.0).abs() < 1e-3, "flat roof height {max_y}");
        let min_x = mesh.iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert!((min_x + 0.3).abs() < 1e-3, "flat eave {min_x}");
    }

    #[test]
    fn normals_point_up_for_floors() {
        for v in floor_mesh(&[room()], &[]) {
            assert_eq!(v.normal, [0.0, 1.0, 0.0]);
        }
    }
}

#[cfg(test)]
mod finish_tests {
    use super::*;
    use fp_core::{EntityId, Pt};
    use fp_model::WallKind;

    #[test]
    fn wall_finish_changes_the_surface_pattern_and_colour() {
        let mut wall = Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(2000, 0),
            240,
            WallKind::Exterior,
        );
        for (finish, colour, pattern) in [
            (WallFinish::Paint, WALL_COLOR, 0.0),
            (WallFinish::Wallpaper, WALL_WALLPAPER, 16.0),
            (WallFinish::WoodPanel, WALL_WOOD, 11.0),
            (WallFinish::Tile, WALL_TILE, 6.0),
        ] {
            wall.finish = finish;
            let mesh = wall_mesh(&[wall.clone()]);
            assert!(!mesh.is_empty());
            assert!(mesh.iter().all(|v| v.pattern == pattern));
            assert!(mesh.iter().all(|v| v.color == colour));
        }
    }
}
