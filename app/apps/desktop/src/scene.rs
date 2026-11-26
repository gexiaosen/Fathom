//! Editor state: document, history, camera and every pointer/keyboard gesture.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use fp_core::{EntityId, Mm, Pt};
use fp_geom::{ROOM_JOIN_TOLERANCE_MM, detect_rooms, distance_point_segment, point_in_polygon, snap_to_grid};
use fp_history::{Command, History};
use fp_model::catalog::{CATALOG, FurnitureCategory};
use fp_model::{
    Document, FinishSurface, Furniture, Level, Material, Measure, Opening, OpeningKind,
    QuoteSettings, RoofKind, RoofSettings, Room, Stair, Wall, WallFinish, WallKind,
};
use fp_render2d::camera::{MAX_SCALE, MIN_SCALE};
use fp_render2d::{
    Batch, Camera2D, Color, QuadVertex, build_grid, build_opening_batch, build_room_fills,
    build_wall_batch, build_wall_batch_visible, build_wall_finish_batch,
};
use fp_render2d::{build_furniture_batch, hit as furniture_hit};

/// Point-in-rotated-rectangle test for a straight stair, in plan millimetres.
fn point_in_stair(stair: &Stair, p: Pt, pad_mm: f32) -> bool {
    let dx = (stair.end.x - stair.start.x) as f32;
    let dy = (stair.end.y - stair.start.y) as f32;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return false;
    }
    let ux = dx / len;
    let uy = dy / len;
    let along = (p.x - stair.start.x) as f32 * ux + (p.y - stair.start.y) as f32 * uy;
    let across = -(p.x - stair.start.x) as f32 * uy + (p.y - stair.start.y) as f32 * ux;
    along >= -pad_mm
        && along <= len + pad_mm
        && across.abs() <= stair.width_mm as f32 * 0.5 + pad_mm
}
use fp_render3d::{
    EYE_HEIGHT, MAX_DISTANCE, MeshVertex, OrbitCamera, WalkCamera, ceiling_mesh_with_materials,
    floor_mesh, wall_mesh_with_finishes,
};
use fp_ui::{
    DEFAULT_GRID_MM, DragSession, Tool, blocked_along_move, clamp_offset, constrained_end,
    furniture_blocking_other, furniture_blocking_wall, furniture_bounds, furniture_path_blocked,
    hit_wall, is_valid_wall, opening_width, wall_offset_at, walls_in_rect,
};
use glam::{Vec3, Vec4};

use crate::underlay::{UnderlayDraw, UnderlayImage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Plan,
    Three,
    Walk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorAction {
    CycleWallKind,
    WallThickness(i32),
    WallHeight(i32),
    WallSetThickness(i32),
    WallSetHeight(i32),
    WallSetLength(i32),
    SetWallKind(WallKind),
    RemoveWall,
    OpeningWidth(i32),
    OpeningHeight(i32),
    OpeningSill(i32),
    OpeningSetWidth(i32),
    OpeningSetHeight(i32),
    OpeningSetSill(i32),
    OpeningSetPrice(i32),
    RemoveOpening,
    CycleMaterial,
    SetMaterial(EntityId),
    RenameRoom,
    FurnitureRotate,
    FurnitureRotateFine(i32),
    FurnitureWidth(i32),
    FurnitureDepth(i32),
    FurnitureSetWidth(i32),
    FurnitureSetDepth(i32),
    FurnitureSetX(i32),
    FurnitureSetY(i32),
    FurnitureSetPrice(i32),
    FurnitureColor,
    FurnitureSetColor(&'static str),
    FurnitureBringFront,
    FurnitureSendBack,
    FurnitureAlignLeft,
    FurnitureAlignCenterX,
    FurnitureAlignRight,
    FurnitureAlignTop,
    FurnitureAlignCenterY,
    FurnitureAlignBottom,
    RemoveFurniture,
    /// Reverse the side a door swings towards (interior / exterior).
    OpeningFlipSwing,
    /// Move the hinge from the left jamb to the right jamb or vice versa.
    OpeningFlipHinge,
    /// Change the selected wall's finishing material.
    SetWallFinish(WallFinish),
    /// Choose a finish record from the wall finish library.
    SetWallFinishMaterial(EntityId),
    /// Apply one finish to every selected wall.
    ApplyWallFinishToSelection(EntityId),
    /// Copy the selected wall's finish to every wall on its floor.
    ApplyWallFinishToFloor,
    /// Choose a finish record for the selected room's ceiling.
    SetCeilingMaterial(EntityId),
    StairSetWidth(i32),
    StairSetSteps(u32),
    StairReverse,
    RemoveStair,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorEdit {
    FurnitureWidth,
    FurnitureDepth,
    FurniturePrice,
    FurnitureX,
    FurnitureY,
    WallLength,
    WallThickness,
    WallHeight,
    OpeningWidth,
    OpeningHeight,
    OpeningSill,
    OpeningPrice,
    StairWidth,
    StairSteps,
}

impl InspectorEdit {
    pub fn current(self, view: &InspectorView) -> Option<&InspectorField> {
        view.fields.iter().find(|f| f.edit == Some(self))
    }
}

#[derive(Clone, Debug)]
pub struct InspectorField {
    pub group: String,
    pub label: String,
    pub value: String,
    pub unit: String,
    pub edit: Option<InspectorEdit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorKind {
    Furniture,
    Wall,
    OpeningDoor,
    OpeningWindow,
    Room,
    Stair,
    Multi,
}

pub struct InspectorView {
    pub title: String,
    pub subtitle: String,
    pub kind: InspectorKind,
    pub fields: Vec<InspectorField>,
    pub rotation: Option<f64>,
    /// (human name, hex, active) for furniture fabric colours.
    pub swatches: Vec<(String, &'static str, bool)>,
    pub lines: Vec<String>,
    pub actions: Vec<(&'static str, InspectorAction)>,
    /// Direct flooring choices for a selected room: (name, hex, id, active).
    /// Empty for other objects.
    pub material_options: Vec<(String, String, EntityId, bool)>,
    /// Wall finish choices from the finish library.
    pub wall_finish_options: Vec<(String, String, EntityId, bool)>,
    /// Ceiling finish choices for a selected room.
    pub ceiling_options: Vec<(String, String, EntityId, bool)>,
    /// Current wall construction kind, for the inspector's type dropdown.
    pub wall_kind: Option<WallKind>,
    /// Current wall finish, for the inspector's finish chips.
    pub wall_finish: Option<WallFinish>,
}

/// Ground-material quote, including the 5% cutting allowance used by the web
/// original. Kept in one place so the status bar, room inspector and CSV agree.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QuoteTotals {
    pub net_area: f64,
    pub billed_area: f64,
    pub material_cost: f64,
    pub waste_cost: f64,
}

fn wall_kind_name(kind: WallKind) -> &'static str {
    match kind {
        WallKind::Bearing => "承重墙",
        WallKind::Exterior => "外墙",
        WallKind::Partition => "隔墙",
        WallKind::Low => "矮墙",
    }
}

const HIGHLIGHT_3D: [f32; 3] = [0.96, 0.58, 0.16];

/// Screen-space radius for wall endpoint / centreline snapping. At the default
/// plan zoom this is roughly 200 mm, enough to catch a corner drawn one wall
/// thickness away without pulling unrelated walls together.
const WALL_SNAP_PX: f32 = 10.0;

/// Fabric swatches shown in the furniture inspector. These are neutral,
/// architectural tones from the design demo rather than an arbitrary rainbow.
pub const FABRIC_SWATCHES: [(&str, &str); 5] = [
    ("雾蓝", "#b8c3d2"),
    ("燕麦", "#c9b9a4"),
    ("鼠尾草", "#9eaaa0"),
    ("铅灰", "#8f9299"),
    ("深海", "#6f7d92"),
];

pub const CEILING_HEIGHT: f32 = 2.75;

fn paint(mesh: &mut [MeshVertex], color: [f32; 3]) {
    for vertex in mesh {
        vertex.color = color;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick3d {
    Wall(EntityId),
    Furniture(EntityId),
    Room(EntityId),
}

/// Slab test of a ray against an oriented box (yaw only).
fn ray_box(origin: Vec3, dir: Vec3, centre: Vec3, half: Vec3, angle: f32) -> Option<f32> {
    let (sin, cos) = angle.sin_cos();
    let rel = origin - centre;
    let o = Vec3::new(cos * rel.x + sin * rel.z, rel.y, -sin * rel.x + cos * rel.z);
    let d = Vec3::new(cos * dir.x + sin * dir.z, dir.y, -sin * dir.x + cos * dir.z);
    let mut t_min = f32::NEG_INFINITY;
    let mut t_max = f32::INFINITY;
    for axis in 0..3 {
        let oi = o[axis];
        let di = d[axis];
        let hi = half[axis];
        if di.abs() < 1e-6 {
            if oi < -hi || oi > hi {
                return None;
            }
        } else {
            let t1 = (-hi - oi) / di;
            let t2 = (hi - oi) / di;
            let (lo, up) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
            t_min = t_min.max(lo);
            t_max = t_max.min(up);
            if t_min > t_max {
                return None;
            }
        }
    }
    if t_max < 0.0 {
        None
    } else {
        Some(t_min.max(0.0))
    }
}

/// Nearest hit along a ray through the extruded scene (metres).
pub fn pick_scene(doc: &Document, origin: Vec3, dir: Vec3) -> Option<Pick3d> {
    pick_scene_at(doc, origin, dir, 0, 0.0)
}

/// Storey-aware pick. `elevation` is the storey's floor height in metres.
pub fn pick_scene_at(
    doc: &Document,
    origin: Vec3,
    dir: Vec3,
    floor: u32,
    elevation: f32,
) -> Option<Pick3d> {
    let mut best: Option<(f32, Pick3d)> = None;
    let mut consider = |t: f32, hit: Pick3d| {
        if best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, hit));
        }
    };
    for item in doc.furniture.iter().filter(|f| f.floor == floor) {
        let height = CATALOG
            .iter()
            .find(|s| s.asset_id == item.asset_id)
            .map(|s| s.height_mm as f32 / 1000.0)
            .unwrap_or(0.75);
        let centre = Vec3::new(
            item.center.x as f32 / 1000.0,
            height * 0.5 + elevation,
            item.center.y as f32 / 1000.0,
        );
        let half = Vec3::new(
            item.width_mm as f32 / 2000.0,
            height * 0.5,
            item.depth_mm as f32 / 2000.0,
        );
        if let Some(t) = ray_box(
            origin,
            dir,
            centre,
            half,
            (item.rotation_deg as f32).to_radians(),
        ) {
            consider(t, Pick3d::Furniture(item.id));
        }
    }
    for wall in doc.walls.iter().filter(|w| w.floor == floor) {
        let dx = (wall.end.x - wall.start.x) as f32;
        let dz = (wall.end.y - wall.start.y) as f32;
        let len = (dx * dx + dz * dz).sqrt();
        if len < 1.0 {
            continue;
        }
        let centre = Vec3::new(
            (wall.start.x + wall.end.x) as f32 / 2000.0,
            wall.height_mm as f32 / 2000.0 + elevation,
            (wall.start.y + wall.end.y) as f32 / 2000.0,
        );
        let half = Vec3::new(
            len / 2000.0,
            wall.height_mm as f32 / 2000.0,
            wall.thickness_mm as f32 / 2000.0,
        );
        if let Some(t) = ray_box(origin, dir, centre, half, dz.atan2(dx)) {
            consider(t, Pick3d::Wall(wall.id));
        }
    }
    if dir.y.abs() > 1e-6 {
        let t = (elevation - origin.y) / dir.y;
        if t >= 0.0 {
            let point = origin + dir * t;
            let plan_point = Pt::new(
                (point.x * 1000.0).round() as i32,
                (point.z * 1000.0).round() as i32,
            );
            for room in doc.rooms.iter().filter(|r| r.floor == floor) {
                if point_in_polygon(plan_point, &room.polygon) {
                    consider(t, Pick3d::Room(room.id));
                }
            }
        }
    }
    best.map(|(_, hit)| hit)
}

pub struct PlanLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub bold: bool,
}

/// Cloned entities waiting to be pasted.
#[derive(Clone, Default)]
struct Clipboard {
    walls: Vec<Wall>,
    furniture: Vec<Furniture>,
    openings: Vec<Opening>,
}

impl Clipboard {
    fn is_empty(&self) -> bool {
        self.walls.is_empty() && self.furniture.is_empty() && self.openings.is_empty()
    }
}

/** Wall length and unit direction along the wall. */
fn wall_axis(w: &Wall) -> (f64, f64, f64) {
    let dx = (w.end.x - w.start.x) as f64;
    let dy = (w.end.y - w.start.y) as f64;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        (1.0, 1.0, 0.0)
    } else {
        (len, dx / len, dy / len)
    }
}

#[derive(Clone, Copy, Debug)]
struct OpeningDrag {
    id: EntityId,
    before: i32,
    /// Distance between the pointer and the opening start when the drag began.
    grab: f64,
}

#[derive(Clone)]
struct FurnitureDrag {
    anchor: EntityId,
    start_pointer: [f32; 2],
    /// Pixels-to-mm factor captured at pointer-down. The drag is applied as a
    /// world delta, so a viewport resize or a camera repaint mid-drag cannot
    /// teleport the piece.
    scale_at_down: f64,
    started: bool,
    before: Vec<(EntityId, Pt)>,
}

/// A freshly placed piece the pointer can still nudge before release.
#[derive(Clone, Copy)]
struct PlaceDrag {
    id: EntityId,
    before: Pt,
    start_pointer: [f32; 2],
    scale_at_down: f64,
}

/// Dragging the little handle above a selected piece to spin it.
#[derive(Clone, Copy)]
struct RotateDrag {
    id: EntityId,
    before: f64,
}

/// A finished two-point measurement, kept until Esc clears the tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub from: Pt,
    pub to: Pt,
}

impl Measurement {
    pub fn length_mm(&self) -> f64 {
        ((self.to.x - self.from.x) as f64).hypot((self.to.y - self.from.y) as f64)
    }
}

/// Toggleable drawing layers, mirroring the Web version's five switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    RoomNames,
    Furniture,
    Grid,
    Dimensions,
    Bearing,
    Underlay,
}

/// Canned 3D viewpoints, matching the Web version's top / bird / oblique trio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset3d {
    Top,
    Bird,
    Oblique,
}

/// A furniture item being dragged in the 3D view (ground-plane constraint).
#[derive(Clone, Copy, Debug)]
struct Drag3d {
    id: EntityId,
    before: Pt,
    /// Offset from the furniture centre to where the pointer landed, metres.
    grab: [f32; 2],
}

pub struct SceneState {
    pub doc: Document,
    pub history: History,
    pub cam: Camera2D,
    pub tool: Tool,
    pub selection: Vec<EntityId>,
    pub selected_room: Option<EntityId>,
    pub catalog_index: usize,
    /// The library can be open without an item being armed: clicking the rail
    /// button must not silently place the first sofa.
    pub furniture_armed: bool,
    pub catalog_filter: FurnitureCategory,
    pub measure_from: Option<Pt>,
    pub measurements: Vec<Measurement>,
    pub shift_down: bool,
    /// Held space turns a left-button drag into a hand-pan (Photoshop/Figma
    /// convention) instead of selecting or drawing on the sheet.
    pub space_down: bool,
    pub lang: crate::i18n::Lang,
    pub cutaway: bool,
    /// Draw the flat roof over the top storey in the 3D view.
    pub show_roof: bool,
    /// Doors swung open in the walk view, by raw entity id.
    pub doors_open: Vec<u128>,
    pub night: bool,
    /// Sun sweep, 0 = morning, 0.5 = noon, 1 = evening.
    pub sun: f32,
    pub selected_furniture: Option<EntityId>,
    /// All selected furniture; `selected_furniture` is the primary item.
    pub furniture_selection: Vec<EntityId>,
    rotate_drag: Option<RotateDrag>,
    pub selected_opening: Option<EntityId>,
    pub selected_stair: Option<EntityId>,
    furniture_drag: Option<FurnitureDrag>,
    /// Set while a just-placed piece can still be dragged before pointer-up.
    placing: Option<PlaceDrag>,
    opening_drag: Option<OpeningDrag>,
    pub hover: Option<EntityId>,
    pub drag: Option<DragSession>,
    pub pan: Option<[f32; 2]>,
    pub draft: Option<Pt>,
    /// True between press and release of a wall stroke.
    wall_pressed: bool,
    pub box_select: Option<[f32; 2]>,
    pub pointer: [f32; 2],
    /// True while the pointer is above a floating panel or rail. The furniture
    /// ghost is only drawn over the canvas; otherwise clicking the "家具" tool
    /// button would leave a default sofa icon under the button.
    pub pointer_over_ui: bool,
    pub view_size: [f32; 2],
    /// Active storey (zero-based) shown and edited in the plan.
    pub active_level: usize,
    /// Modal owns the window: clear the plan surface to transparent so the
    /// GPUI modal above it is not covered by the last opaque frame.
    pub modal_clear: bool,
    pub saved_path: PathBuf,
    pub note: Option<String>,
    pub view: ViewMode,
    pub cam3: OrbitCamera,
    pub walk: WalkCamera,
    clipboard: Option<Clipboard>,
    paste_count: u32,
    walk_keys: [bool; 4],
    /// On-screen D-pad state, kept separate from the keyboard so releasing a
    /// mouse button never cancels a held W key.
    walk_stick: [bool; 4],
    walk_fast: bool,
    walk_look: Option<[f32; 2]>,
    orbit_drag: Option<[f32; 2]>,
    orbit_start: Option<[f32; 2]>,
    furniture_drag_3d: Option<Drag3d>,

    dirty: bool,
    revision: u64,
    saved_revision: u64,
    /// Geometry epoch, separate from the save-dirty revision. Camera moves
    /// must not invalidate the extruded mesh; cut-away must.
    mesh_revision: u64,
    mesh_cache: Option<(u64, bool, bool, std::sync::Arc<fp_render3d::MeshParts>)>,
    underlay_image: Option<Arc<UnderlayImage>>,
}

impl SceneState {
    pub fn new(viewport: [f32; 2]) -> Self {
        let saved_path = user_plan_path();
        let doc = fs::read_to_string(&saved_path)
            .ok()
            .and_then(|text| Document::from_json(&text).ok())
            .unwrap_or_else(load_demo);
        if saved_path.is_file() {
            crate::recent::remember(&saved_path, &doc.meta.name);
        }
        Self::from_doc(doc, viewport, saved_path)
    }

    pub fn from_doc(mut doc: Document, viewport: [f32; 2], saved_path: PathBuf) -> Self {
        crate::templates::ensure_materials(&mut doc);
        let mut cam = Camera2D::new(viewport);
        let (min, max) = plan_bounds(&doc);
        cam.fit(min, max, 80.0);
        let walk = walk_camera(&doc);
        let mut cam3 = OrbitCamera::new(Vec3::ZERO);
        cam3.fit(
            Vec3::new(min.x as f32 / 1000.0, 0.0, min.y as f32 / 1000.0),
            Vec3::new(max.x as f32 / 1000.0, 3.0, max.y as f32 / 1000.0),
        );
        let measurements = doc
            .measures
            .iter()
            .map(|m| Measurement { from: m.a, to: m.b })
            .collect();
        let underlay_image = doc.underlay.as_ref().and_then(|u| {
            UnderlayImage::load(std::path::Path::new(&u.path))
                .ok()
                .map(Arc::new)
        });
        Self {
            doc,
            history: History::new(200),
            cam,
            tool: Tool::Select,
            selection: Vec::new(),
            selected_room: None,
            catalog_index: 0,
            furniture_armed: false,
            catalog_filter: FurnitureCategory::Living,
            measure_from: None,
            measurements,
            shift_down: false,
            space_down: false,
            lang: crate::i18n::load_lang(),
            cutaway: true,
            show_roof: false,
            doors_open: Vec::new(),
            night: false,
            sun: 0.5,
            selected_furniture: None,
            furniture_selection: Vec::new(),
            rotate_drag: None,
            selected_opening: None,
            selected_stair: None,
            furniture_drag: None,
            placing: None,
            opening_drag: None,
            hover: None,
            drag: None,
            pan: None,
            draft: None,
            wall_pressed: false,
            box_select: None,
            pointer: [0.0, 0.0],
            pointer_over_ui: true,
            view_size: viewport,
            active_level: 0,
            modal_clear: false,
            saved_path,
            note: None,
            view: match std::env::var("FP_START_VIEW").as_deref() {
                Ok("3d") => ViewMode::Three,
                Ok("walk") => ViewMode::Walk,
                _ => ViewMode::Plan,
            },
            cam3,
            walk,
            clipboard: None,
            paste_count: 0,
            walk_keys: [false; 4],
            walk_stick: [false; 4],
            walk_fast: false,
            walk_look: None,
            orbit_drag: None,
            orbit_start: None,
            furniture_drag_3d: None,
            dirty: true,
            revision: 1,
            saved_revision: 0,
            mesh_revision: 1,
            mesh_cache: None,
            underlay_image,
        }
    }

    // ------------------------------------------------------------- geometry

    /// Static scene geometry for any camera; PNG export reuses this.
    /// Zero-based active storey, clamped to the document's level range.
    pub fn floor_index(&self) -> usize {
        self.active_level
            .min(self.doc.level_count().saturating_sub(1))
    }

    pub fn floor(&self) -> u32 {
        self.floor_index() as u32
    }

    pub fn floor_name(&self) -> String {
        self.doc.level_name(self.floor_index())
    }

    pub fn walls_for_floor(&self, floor: u32) -> Vec<Wall> {
        self.doc
            .walls
            .iter()
            .filter(|w| w.floor == floor)
            .cloned()
            .collect()
    }

    pub fn rooms_for_floor(&self, floor: u32) -> Vec<Room> {
        self.doc
            .rooms
            .iter()
            .filter(|r| r.floor == floor)
            .cloned()
            .collect()
    }

    pub fn stairs_for_floor(&self, floor: u32) -> Vec<Stair> {
        self.doc
            .stairs
            .iter()
            .filter(|s| s.floor == floor)
            .cloned()
            .collect()
    }

    pub fn furniture_for_floor(&self, floor: u32) -> Vec<Furniture> {
        self.doc
            .furniture
            .iter()
            .filter(|f| f.floor == floor)
            .cloned()
            .collect()
    }

    pub fn openings_for_floor(&self, floor: u32) -> Vec<Opening> {
        self.doc
            .openings
            .iter()
            .filter(|o| {
                self.doc
                    .walls
                    .iter()
                    .any(|w| w.id == o.wall_id && w.floor == floor)
            })
            .cloned()
            .collect()
    }

    /// Switch the storey shown in 2D/3D editing. Transient selections from the
    /// old storey must not leak into the new one.
    /// Add a storey above the current one. `copy_walls` clones the current
    /// storey's wall shell (and its openings) so duplexes start from a usable
    /// outline instead of a blank sheet.
    pub fn add_level(&mut self, copy_walls: bool) {
        let before = self.doc.clone();
        let source = self.floor();
        let index = self.doc.level_count();
        let mut after = before.clone();
        after.levels.push(Level {
            name: format!("{}F", index + 1),
            height_mm: 2800,
        });
        let floor = index as u32;
        if copy_walls {
            let mut raw = after.next_id();
            let mut remap = std::collections::HashMap::new();
            let source_walls: Vec<Wall> = after
                .walls
                .iter()
                .filter(|w| w.floor == source)
                .cloned()
                .collect();
            for mut wall in source_walls {
                let id = EntityId(raw);
                raw += 1;
                remap.insert(wall.id, id);
                wall.id = id;
                wall.floor = floor;
                after.walls.push(wall);
            }
            let source_openings: Vec<Opening> = after
                .openings
                .iter()
                .filter(|o| remap.contains_key(&o.wall_id))
                .cloned()
                .collect();
            for mut opening in source_openings {
                opening.id = EntityId(raw);
                raw += 1;
                opening.wall_id = remap[&opening.wall_id];
                after.openings.push(opening);
            }
            recompute_rooms(&mut after);
        }
        self.history.execute(
            Box::new(LevelSnapshot {
                before,
                after,
                label: "add level",
            }),
            &mut self.doc,
        );
        self.active_level = index;
        self.note = Some(if copy_walls {
            format!("已新增 {}F · 复制本层墙体和门窗", index + 1)
        } else {
            format!("已新增 {}F · 空白楼层", index + 1)
        });
        let elevation = self.doc.level_elevation_mm(index) as f32 / 1000.0;
        self.cam3.target.y = elevation + 1.2;
        self.walk.pos.y = elevation + EYE_HEIGHT;
        self.selection.clear();
        self.selected_room = None;
        self.selected_furniture = None;
        self.furniture_selection.clear();
        self.sync_measurements();
        self.touch();
    }

    /// Remove the current storey and shift every storey above it down.
    pub fn remove_active_level(&mut self) {
        if self.doc.level_count() <= 1 {
            self.note = Some("至少保留一层".into());
            return;
        }
        let index = self.floor_index() as u32;
        let before = self.doc.clone();
        let mut after = before.clone();
        let removed_walls: Vec<EntityId> = after
            .walls
            .iter()
            .filter(|w| w.floor == index)
            .map(|w| w.id)
            .collect();
        after.walls.retain(|w| w.floor != index);
        after
            .openings
            .retain(|o| !removed_walls.contains(&o.wall_id));
        after.rooms.retain(|r| r.floor != index);
        after.furniture.retain(|f| f.floor != index);
        after.stairs.retain(|s| s.floor != index);
        after.measures.retain(|m| m.floor != index);
        for wall in &mut after.walls {
            if wall.floor > index {
                wall.floor -= 1;
            }
        }
        for room in &mut after.rooms {
            if room.floor > index {
                room.floor -= 1;
            }
        }
        for item in &mut after.furniture {
            if item.floor > index {
                item.floor -= 1;
            }
        }
        for stair in &mut after.stairs {
            if stair.floor > index {
                stair.floor -= 1;
            }
        }
        for measure in &mut after.measures {
            if measure.floor > index {
                measure.floor -= 1;
            }
        }
        if let Some(underlay) = &mut after.underlay {
            if underlay.floor > index {
                underlay.floor -= 1;
            } else if underlay.floor == index {
                after.underlay = None;
            }
        }
        after.levels.remove(index as usize);
        for (i, level) in after.levels.iter_mut().enumerate() {
            let old_index = if i < index as usize { i } else { i + 1 };
            let auto_name = format!("{}F", old_index + 1);
            if level.name == auto_name {
                level.name = format!("{}F", i + 1);
            }
        }
        self.history.execute(
            Box::new(LevelSnapshot {
                before,
                after,
                label: "remove level",
            }),
            &mut self.doc,
        );
        self.active_level = index.saturating_sub(1) as usize;
        let elevation = self.doc.level_elevation_mm(self.active_level) as f32 / 1000.0;
        self.cam3.target.y = elevation + 1.2;
        self.walk.pos.y = elevation + EYE_HEIGHT;
        self.selection.clear();
        self.selected_room = None;
        self.selected_opening = None;
        self.selected_stair = None;
        self.selected_furniture = None;
        self.furniture_selection.clear();
        self.sync_measurements();
        self.touch();
    }

    pub fn set_level(&mut self, level: usize) {
        let level = level.min(self.doc.level_count().saturating_sub(1));
        if self.active_level == level {
            return;
        }
        self.active_level = level;
        let elevation = self.doc.level_elevation_mm(level) as f32 / 1000.0;
        self.cam3.target.y = elevation + 1.2;
        self.walk.pos.y = elevation + EYE_HEIGHT;
        self.selection.clear();
        self.selected_room = None;
        self.selected_opening = None;
        self.selected_stair = None;
        self.selected_furniture = None;
        self.furniture_selection.clear();
        self.hover = None;
        self.drag = None;
        self.draft = None;
        self.placing = None;
        self.box_select = None;
        self.sync_measurements();
        self.touch();
    }

    pub fn render_batch(
        &self,
        cam: &Camera2D,
        selection: &[EntityId],
        furniture_selection: &[EntityId],
        selected_room: Option<EntityId>,
    ) -> Batch {
        self.render_batch_with(
            self.floor(),
            true,
            true,
            true,
            false,
            cam,
            selection,
            furniture_selection,
            selected_room,
        )
    }

    /// Render one storey without the interactive pointer ghosts or grid. Used
    /// by the construction sheet exporter, which draws every floor from the
    /// same plan.
    pub fn render_batch_on(
        &self,
        floor: u32,
        cam: &Camera2D,
        selection: &[EntityId],
        furniture_selection: &[EntityId],
        selected_room: Option<EntityId>,
    ) -> Batch {
        self.render_batch_with(
            floor,
            false,
            false,
            true,
            true,
            cam,
            selection,
            furniture_selection,
            selected_room,
        )
    }

    /// Ground-finish drawing: walls, rooms and openings stay, furniture goes.
    pub fn render_batch_finish_on(
        &self,
        floor: u32,
        cam: &Camera2D,
        selected_room: Option<EntityId>,
    ) -> Batch {
        self.render_batch_with(
            floor,
            false,
            false,
            false,
            true,
            cam,
            &[],
            &[],
            selected_room,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_batch_with(
        &self,
        floor: u32,
        show_grid: bool,
        show_measurements: bool,
        show_furniture: bool,
        show_wall_finish: bool,
        cam: &Camera2D,
        selection: &[EntityId],
        furniture_selection: &[EntityId],
        selected_room: Option<EntityId>,
    ) -> Batch {
        let mut batch = Batch::default();
        if show_grid && self.doc.settings.show_grid {
            let grid = build_grid(cam, 8.0);
            for l in &grid.minor {
                push_thick_line(&mut batch, cam, l[0], l[1], 1.0, Color::GRID_MINOR);
            }
            for l in &grid.major {
                push_thick_line(&mut batch, cam, l[0], l[1], 1.0, Color::GRID_MAJOR);
            }
        }
        let floor_rooms = self.rooms_for_floor(floor);
        let floor_walls = self.walls_for_floor(floor);
        let floor_openings = self.openings_for_floor(floor);
        let fills = build_room_fills(&floor_rooms, &self.doc.materials, cam, selected_room);
        batch.vertices.extend(fills.vertices);
        let walls = if show_wall_finish {
            build_wall_finish_batch(
                &floor_walls,
                &self.doc.materials,
                cam,
                self.doc.settings.show_bearing,
            )
        } else {
            build_wall_batch_visible(&floor_walls, selection, cam, self.doc.settings.show_bearing)
        };
        batch.vertices.extend(walls.vertices);
        let openings = build_opening_batch(&floor_walls, &floor_openings, cam, &[]);
        batch.vertices.extend(openings.vertices);
        if show_furniture && self.doc.settings.show_furniture {
            let floor_furniture = self.furniture_for_floor(floor);
            let furniture = build_furniture_batch(&floor_furniture, cam, furniture_selection);
            batch.vertices.extend(furniture.vertices);
            if floor == self.floor()
                && let Some(item) = self.selected_furniture_item()
            {
                {
                    let handle = self.furniture_handle(item);
                    let ink = Color([0.96, 0.58, 0.16, 1.0]);
                    push_screen_line(
                        &mut batch,
                        [handle[0] - 5.0, handle[1]],
                        [handle[0] + 5.0, handle[1]],
                        2.0,
                        ink,
                    );
                    push_screen_line(
                        &mut batch,
                        [handle[0], handle[1] - 5.0],
                        [handle[0], handle[1] + 5.0],
                        2.0,
                        ink,
                    );
                }
            }
        }
        for stair in self.stairs_for_floor(floor) {
            let stair_ink = if self.selected_stair == Some(stair.id) {
                Color([0.96, 0.58, 0.16, 1.0])
            } else {
                Color([0.55, 0.42, 0.20, 0.9])
            };
            let dx = (stair.end.x - stair.start.x) as f32;
            let dy = (stair.end.y - stair.start.y) as f32;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1.0 {
                continue;
            }
            let ux = dx / len;
            let uy = dy / len;
            let nx = -uy * stair.width_mm as f32 * 0.5;
            let ny = ux * stair.width_mm as f32 * 0.5;
            let a = Pt::new(
                (stair.start.x as f32 + nx) as i32,
                (stair.start.y as f32 + ny) as i32,
            );
            let b = Pt::new(
                (stair.start.x as f32 - nx) as i32,
                (stair.start.y as f32 - ny) as i32,
            );
            let c = Pt::new(
                (stair.end.x as f32 - nx) as i32,
                (stair.end.y as f32 - ny) as i32,
            );
            let d = Pt::new(
                (stair.end.x as f32 + nx) as i32,
                (stair.end.y as f32 + ny) as i32,
            );
            for (p, q) in [(a, b), (b, c), (c, d), (d, a)] {
                push_thick_line(&mut batch, cam, p, q, 1.8, stair_ink);
            }
            let steps = stair.steps.clamp(3, 40) as f32;
            for i in 1..steps as i32 {
                let t = i as f32 / steps;
                let p = Pt::new(
                    (stair.start.x as f32 + dx * t + nx) as i32,
                    (stair.start.y as f32 + dy * t + ny) as i32,
                );
                let q = Pt::new(
                    (stair.start.x as f32 + dx * t - nx) as i32,
                    (stair.start.y as f32 + dy * t - ny) as i32,
                );
                push_thick_line(&mut batch, cam, p, q, 1.2, stair_ink.with_alpha(0.55));
            }
            let mid = Pt::new(
                (stair.start.x + stair.end.x) / 2,
                (stair.start.y + stair.end.y) / 2,
            );
            push_thick_line(&mut batch, cam, stair.start, mid, 2.4, stair_ink);
        }
        let ink = Color([0.10, 0.62, 0.58, 1.0]);
        for measurement in self
            .doc
            .measures
            .iter()
            .filter(|m| m.floor == floor && show_measurements)
        {
            push_dashed_line(&mut batch, cam, measurement.a, measurement.b, 1.8, ink);
            push_world_cross(&mut batch, cam, measurement.a, ink);
            push_world_cross(&mut batch, cam, measurement.b, ink);
        }
        if self.tool == Tool::Measure
            && let Some(from) = self.measure_from
        {
            let to = constrained_end(
                from,
                self.snap_measure(self.pointer_world()),
                self.shift_down,
                1,
            );
            let ghost = ink.with_alpha(0.75);
            push_dashed_line(&mut batch, cam, from, to, 1.8, ghost);
            push_world_cross(&mut batch, cam, from, ink);
            push_world_cross(&mut batch, cam, to, ghost);
        }

        batch
    }

    pub fn build_batch(&self) -> Batch {
        let mut batch = self.render_batch(
            &self.cam,
            &self.selection,
            &self.furniture_selection,
            self.selected_room,
        );

        // Furniture follows the cursor before the click, so placement is
        // WYSIWYG instead of "drop first, drag later". Never draw that ghost
        // while the pointer sits on the rails / library, where it would look
        // like a stray default sofa appearing under the button.
        if self.tool == Tool::Furniture
            && self.placing.is_none()
            && !self.pointer_over_ui
            && self.pointer_in_canvas()
        {
            let spec = &CATALOG[self.catalog_index % CATALOG.len()];
            let world = self.pointer_world();
            let center = Pt::new(
                snap_to_grid(world.x, DEFAULT_GRID_MM),
                snap_to_grid(world.y, DEFAULT_GRID_MM),
            );
            let ghost = Furniture::from_spec(EntityId(0), spec, center, 0.0);
            let mut ghost_batch =
                build_furniture_batch(std::slice::from_ref(&ghost), &self.cam, &[]);
            for v in &mut ghost_batch.vertices {
                v.color[3] *= 0.55;
            }
            batch.vertices.extend(ghost_batch.vertices);
        }

        let hover_target = self
            .hover
            .filter(|h| !self.selection.contains(h))
            .and_then(|h| {
                self.doc
                    .walls
                    .iter()
                    .find(|w| w.id == h && w.floor == self.floor())
            });
        if let Some(w) = hover_target {
            let ghost = build_wall_batch(std::slice::from_ref(w), &[], &self.cam);
            for v in ghost.vertices {
                let mut c = v.color;
                if self.tool == Tool::Demolish {
                    if w.kind == WallKind::Bearing {
                        c = [0.86, 0.24, 0.20, 0.92];
                    } else {
                        c = [0.98, 0.62, 0.18, 0.92];
                    }
                } else {
                    c[0] = 0.2;
                    c[1] = 0.55;
                    c[2] = 0.95;
                }
                batch.vertices.push(QuadVertex {
                    pos: v.pos,
                    color: c,
                });
            }
        }

        if let Some(start) = self.draft {
            let end = constrained_end(start, self.pointer_world(), true, DEFAULT_GRID_MM);
            push_thick_line(&mut batch, &self.cam, start, end, 3.0, Color::HOVER);
        }

        if let Some(anchor) = self.box_select {
            let rect = [
                anchor[0].min(self.pointer[0]),
                anchor[1].min(self.pointer[1]),
                anchor[0].max(self.pointer[0]),
                anchor[1].max(self.pointer[1]),
            ];
            batch.push_quad(
                [
                    [rect[0], rect[1]],
                    [rect[2], rect[1]],
                    [rect[2], rect[3]],
                    [rect[0], rect[3]],
                ],
                Color::HOVER.with_alpha(0.18),
            );
            let outline = Color::HOVER.with_alpha(0.85);
            let w = 1.0;
            push_rect_outline(&mut batch, rect, w, outline);
        }

        batch
    }

    /// Screen-space textured quad for the imported plan image, if visible.
    pub fn underlay_draw(&self) -> Option<UnderlayDraw> {
        if self.view != ViewMode::Plan || !self.doc.settings.show_underlay {
            return None;
        }
        let underlay = self.doc.underlay.as_ref()?;
        if underlay.floor != self.floor() {
            return None;
        }
        let image = self.underlay_image.as_ref()?.clone();
        let a = self
            .cam
            .world_to_screen(Pt::new(underlay.x_mm, underlay.y_mm));
        let b = self.cam.world_to_screen(Pt::new(
            underlay.x_mm + underlay.width_mm,
            underlay.y_mm + underlay.height_mm,
        ));
        Some(UnderlayDraw {
            image,
            rect: [a[0], a[1], b[0], b[1]],
            opacity: underlay.opacity.clamp(0.05, 0.9),
        })
    }

    fn pointer_world(&self) -> Pt {
        self.cam.screen_to_world(self.pointer)
    }

    /// Loose canvas bounds, used only to keep cursor ghosts off the chrome.
    fn pointer_in_canvas(&self) -> bool {
        let [w, h] = self.view_size;
        self.pointer[0] >= 96.0
            && self.pointer[0] <= w - 116.0
            && self.pointer[1] >= 48.0
            && self.pointer[1] <= h - 66.0
    }

    pub fn set_pointer_over_ui(&mut self, over: bool) {
        self.pointer_over_ui = over;
    }

    /// Toggle the cut-away height. This changes the extruded mesh, so it must
    /// bump the revision; otherwise the cached mesh and the renderer epoch stay
    /// put and the walls only update after the camera is dragged.
    pub fn set_roof(&mut self, on: bool) {
        if self.show_roof != on {
            self.show_roof = on;
            self.mesh_revision += 1;
        }
    }

    pub fn roof_settings(&self) -> RoofSettings {
        self.doc.roof.normalized()
    }

    fn commit_roof(&mut self, after: RoofSettings) {
        let before = self.doc.roof.normalized();
        let after = after.normalized();
        if before == after {
            return;
        }
        let mut doc = self.doc.clone();
        doc.roof = after;
        self.history.execute(
            Box::new(RoofSnapshot {
                before: self.doc.clone(),
                after: doc,
                label: "roof",
            }),
            &mut self.doc,
        );
        self.note = Some(match after.kind {
            RoofKind::Flat => format!("平屋顶 · 檐口 {} mm", after.overhang_mm),
            RoofKind::Gable => {
                format!(
                    "坡屋顶 {:.0}° · 檐口 {} mm",
                    after.pitch_deg, after.overhang_mm
                )
            }
        });
        self.touch();
    }

    pub fn set_roof_kind(&mut self, kind: RoofKind) {
        let mut after = self.roof_settings();
        after.kind = kind;
        self.commit_roof(after);
    }

    pub fn cycle_roof_pitch(&mut self) {
        const PITCHES: [f32; 4] = [15.0, 25.0, 35.0, 45.0];
        let mut after = self.roof_settings();
        after.pitch_deg = PITCHES
            .iter()
            .copied()
            .find(|p| *p > after.pitch_deg + 0.1)
            .unwrap_or(PITCHES[0]);
        self.commit_roof(after);
    }

    pub fn cycle_roof_overhang(&mut self) {
        const EAVES: [i32; 4] = [300, 600, 900, 1_200];
        let mut after = self.roof_settings();
        after.overhang_mm = EAVES
            .iter()
            .copied()
            .find(|m| *m > after.overhang_mm)
            .unwrap_or(EAVES[0]);
        self.commit_roof(after);
    }

    pub fn set_cutaway(&mut self, on: bool) {
        if self.cutaway != on {
            self.cutaway = on;
            self.mesh_revision += 1;
        }
    }

    /// Toggle night lighting. The mesh geometry does not change; the renderer
    /// keys its opaque pass on the light/ambient values, so no mesh rebuild is
    /// needed and the document is not marked dirty.
    pub fn set_night(&mut self, on: bool) {
        if self.night != on {
            self.night = on;
        }
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.revision += 1;
        self.mesh_revision += 1;
    }

    /// Nearest existing wall endpoint or centreline projection. Drawing uses
    /// this so a corner lands exactly on the previous wall instead of one grid
    /// cell inside it, which is what made rooms fail to close.
    fn wall_snap_candidate(&self, raw: Pt) -> Option<Pt> {
        let walls = self.walls_for_floor(self.floor());
        // Endpoint capture is deliberately at least 160 mm: a corner drawn
        // with a half-wall-thickness miss must still weld. Projection below
        // stays tighter so parallel walls do not collapse onto each other.
        let tolerance = (WALL_SNAP_PX as f64 / self.cam.scale.max(1e-6)).clamp(160.0, 800.0);
        // Projection is tighter than endpoint capture: a wall drawn parallel
        // 15 cm away should stay parallel, not jump onto its neighbour.
        let projection_tolerance = (WALL_SNAP_PX as f64 * 0.6 / self.cam.scale.max(1e-6))
            .clamp(60.0, 500.0);
        let mut best: Option<(f64, Pt)> = None;

        for wall in &walls {
            for candidate in [wall.start, wall.end] {
                let distance =
                    ((raw.x - candidate.x) as f64).hypot((raw.y - candidate.y) as f64);
                if distance <= tolerance
                    && best.is_none_or(|(best_distance, _)| distance < best_distance)
                {
                    best = Some((distance, candidate));
                }
            }
        }

        // A corner endpoint wins even when the pointer also lies exactly on
        // the wall's centreline: that is the difference between "extend the
        // wall by 35 mm" and "start at the existing corner".
        if let Some((_, endpoint)) = best {
            return Some(endpoint);
        }

        for wall in &walls {
            let dx = (wall.end.x - wall.start.x) as f64;
            let dy = (wall.end.y - wall.start.y) as f64;
            let len_sq = dx * dx + dy * dy;
            if len_sq < 1.0 {
                continue;
            }
            let t = (((raw.x - wall.start.x) as f64) * dx
                + ((raw.y - wall.start.y) as f64) * dy)
                / len_sq;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let projection = Pt::new(
                (wall.start.x as f64 + dx * t).round() as Mm,
                (wall.start.y as f64 + dy * t).round() as Mm,
            );
            let distance =
                ((raw.x - projection.x) as f64).hypot((raw.y - projection.y) as f64);
            if distance <= projection_tolerance
                && best.is_none_or(|(best_distance, _)| distance < best_distance)
            {
                best = Some((distance, projection));
            }
        }

        best.map(|(_, point)| point)
    }

    fn snap_wall_start(&self, raw: Pt) -> Pt {
        self.wall_snap_candidate(raw).unwrap_or_else(|| {
            Pt::new(
                snap_to_grid(raw.x, DEFAULT_GRID_MM),
                snap_to_grid(raw.y, DEFAULT_GRID_MM),
            )
        })
    }

    /// Keep the orthogonal constraint, then let a nearby joint win over the
    /// projected grid point. Accepting the joint is what makes corners exact.
    fn snap_wall_end(&self, start: Pt, raw: Pt) -> Pt {
        self.wall_snap_candidate(raw)
            .unwrap_or_else(|| constrained_end(start, raw, true, DEFAULT_GRID_MM))
    }

    /// Commit one wall segment. Both click-click and press-drag use this path.
    fn commit_wall(&mut self, start: Pt, end: Pt) {
        if !is_valid_wall(start, end) {
            return;
        }
        let id = EntityId(self.doc.next_id());
        let mut wall = Wall::new(
            id,
            start,
            end,
            default_thickness(&self.doc),
            WallKind::Partition,
        );
        wall.floor = self.floor();
        self.history.execute(
            Box::new(AddWall {
                wall,
                before_rooms: None,
            }),
            &mut self.doc,
        );
        self.selection = vec![id];
        let len = ((end.x - start.x) as f64).hypot((end.y - start.y) as f64);
        self.note = Some(format!("墙 {:.2} m", len / 1000.0));
    }

    fn commit_stair(&mut self, start: Pt, end: Pt) {
        let len = ((end.x - start.x) as f64).hypot((end.y - start.y) as f64);
        if len < 500.0 {
            self.note = Some("楼梯长度至少 0.5 m".into());
            return;
        }
        if self.floor_index() + 1 >= self.doc.level_count() {
            self.note = Some("顶层不能再放楼梯，请先新增楼层".into());
            return;
        }
        let steps = ((len / 250.0).round() as u32).clamp(8, 20);
        let stair = Stair {
            id: EntityId(self.doc.next_id()),
            floor: self.floor(),
            start,
            end,
            width_mm: 1000,
            steps,
        };
        self.history
            .execute(Box::new(AddStair { stair }), &mut self.doc);
        self.note = Some(format!("楼梯 {:.2} m · {} 级", len / 1000.0, steps));
        self.touch();
    }

    pub fn active_level_info(&self) -> (String, i32) {
        let index = self.floor_index();
        let level = self.doc.levels.get(index);
        (
            level
                .map(|l| l.name.clone())
                .unwrap_or_else(|| format!("{}F", index + 1)),
            level.map(|l| l.height_mm).unwrap_or(2800),
        )
    }

    pub fn set_active_level_info(&mut self, name: &str, height_mm: i32) -> Result<(), String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("楼层名称不能为空".into());
        }
        if trimmed.chars().count() > 12 {
            return Err("楼层名称最多 12 个字符".into());
        }
        if !(1_800..=6_000).contains(&height_mm) {
            return Err("层高需在 1800–6000 mm 之间".into());
        }
        let index = self.floor_index();
        let before = self.doc.clone();
        let mut after = before.clone();
        if index >= after.levels.len() {
            after.ensure_levels();
        }
        let level = &mut after.levels[index];
        if level.name == trimmed && level.height_mm == height_mm {
            return Ok(());
        }
        level.name = trimmed.to_string();
        level.height_mm = height_mm;
        self.history.execute(
            Box::new(LevelSnapshot {
                before,
                after,
                label: "edit level",
            }),
            &mut self.doc,
        );
        self.note = Some(format!("{trimmed} · 层高 {height_mm} mm"));
        let elevation = self.doc.level_elevation_mm(index) as f32 / 1000.0;
        self.cam3.target.y = elevation + 1.2;
        self.walk.pos.y = elevation + EYE_HEIGHT;
        self.touch();
        Ok(())
    }

    pub fn selected_stair_info(&self) -> Option<Stair> {
        let id = self.selected_stair?;
        self.doc.stairs.iter().find(|s| s.id == id).cloned()
    }

    pub fn set_selected_stair_size(
        &mut self,
        width_mm: Option<i32>,
        steps: Option<u32>,
    ) -> Result<(), String> {
        let before = self.selected_stair_info().ok_or("未选中楼梯")?;
        let mut after = before.clone();
        if let Some(width) = width_mm {
            if !(600..=2_000).contains(&width) {
                return Err("楼梯宽度需在 600–2000 mm 之间".into());
            }
            after.width_mm = width;
        }
        if let Some(count) = steps {
            if !(3..=40).contains(&count) {
                return Err("踏步数需在 3–40 级之间".into());
            }
            after.steps = count;
        }
        if before.width_mm == after.width_mm && before.steps == after.steps {
            return Ok(());
        }
        let width = after.width_mm;
        let steps = after.steps;
        self.history
            .execute(Box::new(UpdateStair { before, after }), &mut self.doc);
        self.note = Some(format!("楼梯已更新 · {width} mm · {steps} 级"));
        self.touch();
        Ok(())
    }

    pub fn reverse_selected_stair(&mut self) -> bool {
        let Some(before) = self.selected_stair_info() else {
            return false;
        };
        let mut after = before.clone();
        after.start = before.end;
        after.end = before.start;
        self.history
            .execute(Box::new(UpdateStair { before, after }), &mut self.doc);
        self.note = Some("楼梯方向已反向".into());
        self.touch();
        true
    }

    /// Finalise a piece that is still under the pointer. Called on release,
    /// and defensively when another stroke starts.
    fn finish_placement(&mut self) {
        let Some(place) = self.placing.take() else {
            return;
        };
        let after = self
            .doc
            .furniture
            .iter()
            .find(|f| f.id == place.id)
            .map(|f| f.center)
            .filter(|c| *c != place.before);
        if let Some(after) = after {
            self.history.execute(
                Box::new(MoveFurniture {
                    id: place.id,
                    before: place.before,
                    after,
                }),
                &mut self.doc,
            );
        }
        self.tool = Tool::Select;
        self.note = Some("家具已放置".into());
        self.touch();
    }

    fn furniture_clear_against(&self, item: &Furniture, center: Pt, others: &[Furniture]) -> bool {
        let mut probe = item.clone();
        probe.center = center;
        furniture_blocking_wall(&probe, &self.walls_for_floor(self.floor())).is_none()
            && furniture_blocking_other(&probe, others, &[]).is_none()
    }

    /// Pick the requested centre when clear, otherwise nudge the piece along a
    /// small ring until it fits. Returns `None` when there is no nearby space.
    fn clear_placement_center(&self, item: &Furniture, desired: Pt) -> Option<Pt> {
        self.clear_center_against(item, desired, &self.furniture_for_floor(self.floor()))
    }

    fn clear_center_against(
        &self,
        item: &Furniture,
        desired: Pt,
        others: &[Furniture],
    ) -> Option<Pt> {
        if self.furniture_clear_against(item, desired, others) {
            return Some(desired);
        }
        for radius in (100..=3000).step_by(100) {
            for step in 0..16 {
                let angle = std::f64::consts::TAU * (step as f64) / 16.0;
                let p = Pt::new(
                    desired.x + (angle.cos() * radius as f64).round() as i32,
                    desired.y + (angle.sin() * radius as f64).round() as i32,
                );
                if self.furniture_clear_against(item, p, others) {
                    return Some(p);
                }
            }
        }
        None
    }

    fn note_wall_blocked(&mut self) {
        if self.note.as_deref() != Some("碰撞阻挡 · 家具不能穿墙或重叠") {
            self.note = Some("碰撞阻挡 · 家具不能穿墙或重叠".into());
        }
    }

    /// Measurements live in the document so they survive save/load and undo.
    fn sync_measurements(&mut self) {
        let floor = self.floor();
        self.measurements = self
            .doc
            .measures
            .iter()
            .filter(|m| m.floor == floor)
            .map(|m| Measurement { from: m.a, to: m.b })
            .collect();
    }

    pub fn measure_count(&self) -> usize {
        self.measurements.len()
    }

    pub fn clear_measurements(&mut self) {
        if self.doc.measures.is_empty() && self.measurements.is_empty() {
            return;
        }
        let before = self.doc.measures.clone();
        self.history
            .execute(Box::new(ClearMeasures { before }), &mut self.doc);
        self.sync_measurements();
        self.note = Some("测量已清除".into());
        self.touch();
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Document-level dirty flag (camera movement does not count).
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    // -------------------------------------------------------------- pointer

    pub fn pointer_down(&mut self, pos: [f32; 2], button: PointerButton, shift: bool) {
        self.pointer = pos;
        if self.view == ViewMode::Walk {
            if button == PointerButton::Left {
                // A direct click on a nearby door toggles it; elsewhere the
                // press starts a look-drag.
                if let Some(id) = self.door_at_screen(pos) {
                    self.toggle_door(id);
                    return;
                }
                self.walk_look = Some(pos);
            }
            return;
        }
        if self.view == ViewMode::Three {
            if button == PointerButton::Left {
                if self.begin_furniture_drag_3d(pos) {
                    return;
                }
                self.orbit_drag = Some(pos);
                self.orbit_start = Some(pos);
            }
            return;
        }
        match button {
            PointerButton::Middle => {
                self.pan = Some(pos);
                return;
            }
            PointerButton::Left if self.space_down => {
                self.pan = Some(pos);
                self.touch();
                return;
            }
            PointerButton::Left => {}
            _ => return,
        }

        // A previous piece may still be live if a pointer-up was swallowed by
        // a toolbar click; settle it before starting a new gesture.
        if self.placing.is_some() {
            let wanted = self.tool;
            self.finish_placement();
            self.tool = wanted;
        }

        if self.tool == Tool::Measure {
            let world = self.snap_measure(self.pointer_world());
            match self.measure_from {
                None => {
                    self.measure_from = Some(world);
                    self.note = Some("测量：点击第二点".into());
                }
                Some(from) => {
                    let to = constrained_end(from, world, shift, 1);
                    let len = ((to.x - from.x) as f64).hypot((to.y - from.y) as f64);
                    if len >= 50.0 {
                        let measure = Measure {
                            id: EntityId(self.doc.next_id()),
                            a: from,
                            b: to,
                            floor: self.active_level as u32,
                        };
                        self.history
                            .execute(Box::new(AddMeasure { measure }), &mut self.doc);
                        self.sync_measurements();
                        self.note = Some(format!("{:.2} m", len / 1000.0));
                    }
                    self.measure_from = None;
                }
            }
            self.touch();
            return;
        }

        if self.tool == Tool::Furniture {
            if !self.furniture_armed {
                self.note = Some("先在家具库选择一件家具".into());
                self.touch();
                return;
            }
            let spec = &CATALOG[self.catalog_index % CATALOG.len()];
            let world = self.pointer_world();
            let desired = Pt::new(
                snap_to_grid(world.x, DEFAULT_GRID_MM),
                snap_to_grid(world.y, DEFAULT_GRID_MM),
            );
            let mut item = Furniture::from_spec(EntityId(self.doc.next_id()), spec, desired, 0.0);
            item.floor = self.floor();
            let Some(center) = self.clear_placement_center(&item, desired) else {
                self.note = Some("此处没有足够空间放置家具".into());
                self.touch();
                return;
            };
            let nudged = center != desired;
            item.center = center;
            let id = item.id;
            self.history
                .execute(Box::new(AddFurniture { item }), &mut self.doc);
            self.selected_furniture = Some(id);
            self.furniture_selection = vec![id];
            self.selected_room = None;
            self.selection.clear();
            // Keep the piece live under the pointer until release, so a
            // press-drag places it exactly where the mouse ends up.
            self.placing = Some(PlaceDrag {
                id,
                before: center,
                start_pointer: pos,
                scale_at_down: self.cam.scale,
            });
            self.tool = Tool::Select;
            self.furniture_armed = false;
            self.note = Some(if nudged {
                "已避开墙体 · 拖动可调整".into()
            } else {
                "拖动可调整位置，松手放置".into()
            });
            self.touch();
            return;
        }

        if matches!(self.tool, Tool::Door | Tool::Window) {
            let floor_walls = self.walls_for_floor(self.floor());
            if let Some(wall_id) = hit_wall(&floor_walls, &self.cam, pos, 6.0) {
                let wall = self.doc.walls.iter().find(|w| w.id == wall_id).cloned();
                if let Some(wall) = wall {
                    let width = opening_width(self.tool);
                    let len = ((wall.end.x - wall.start.x) as f64)
                        .hypot((wall.end.y - wall.start.y) as f64)
                        as i32;
                    if width > 0 && len >= width {
                        let center = wall_offset_at(&wall, self.pointer_world());
                        let offset = clamp_offset(&wall, center - width / 2, width);
                        let clash = self.doc.openings.iter().any(|o| {
                            o.wall_id == wall_id
                                && offset < o.offset_mm + o.width_mm
                                && offset + width > o.offset_mm
                        });
                        if !clash {
                            let kind = if self.tool == Tool::Door {
                                OpeningKind::Door
                            } else {
                                OpeningKind::Window
                            };
                            let opening = Opening {
                                id: EntityId(self.doc.next_id()),
                                wall_id,
                                kind,
                                offset_mm: offset,
                                width_mm: width,
                                height_mm: if kind == OpeningKind::Door {
                                    2100
                                } else {
                                    1500
                                },
                                sill_mm: if kind == OpeningKind::Door { 0 } else { 900 },
                                swing_deg: 90.0,
                                hinge_right: false,
                                price: None,
                            };
                            self.history
                                .execute(Box::new(AddOpening { opening }), &mut self.doc);
                        }
                    }
                }
            }
            self.touch();
            return;
        }

        if self.tool == Tool::Stair {
            let world = self.pointer_world();
            match self.draft {
                None => {
                    self.draft = Some(world);
                    self.wall_pressed = true;
                    self.note = Some("拖到楼梯终点松手，或再点一下确定".into());
                }
                Some(start) => {
                    let end = constrained_end(start, world, true, DEFAULT_GRID_MM);
                    self.commit_stair(start, end);
                    self.draft = None;
                    self.wall_pressed = false;
                }
            }
            self.touch();
            return;
        }

        if self.tool == Tool::Wall {
            let raw = self.pointer_world();
            match self.draft {
                None => {
                    let start = self.snap_wall_start(raw);
                    self.draft = Some(start);
                    self.wall_pressed = true;
                    self.note = Some("拖到终点松手，或再点一下确定".into());
                }
                Some(start) => {
                    let end = self.snap_wall_end(start, raw);
                    self.commit_wall(start, end);
                    self.draft = None;
                    self.wall_pressed = false;
                }
            }
            self.touch();
            return;
        }

        if self.tool == Tool::Demolish {
            let floor_walls = self.walls_for_floor(self.floor());
            if let Some((wall_id, wall_kind)) = hit_wall(&floor_walls, &self.cam, pos, 8.0)
                .and_then(|id| {
                    self.doc
                        .walls
                        .iter()
                        .find(|w| w.id == id)
                        .map(|w| (id, w.kind))
                })
            {
                if wall_kind == WallKind::Bearing {
                    self.note = Some("承重墙不可拆 · 仅非承重墙可拆改".into());
                } else {
                    let kind = wall_kind_name(wall_kind);
                    self.selection.clear();
                    self.selection.push(wall_id);
                    self.delete_selection();
                    self.note = Some(format!("已拆除{kind}"));
                }
            }
            self.touch();
            return;
        }

        if let Some(item) = self.selected_furniture_item() {
            {
                let handle = self.furniture_handle(item);
                let close = ((pos[0] - handle[0]).powi(2) + (pos[1] - handle[1]).powi(2)).sqrt();
                if close <= 11.0 {
                    self.rotate_drag = Some(RotateDrag {
                        id: item.id,
                        before: item.rotation_deg,
                    });
                    self.drag = None;
                    self.box_select = None;
                    self.touch();
                    return;
                }
            }
        }

        if let Some(oid) = self.hit_opening() {
            if !shift {
                self.selection.clear();
                self.selected_room = None;
                self.selected_furniture = None;
                self.furniture_selection.clear();
                self.selected_stair = None;
            }
            self.selected_opening = Some(oid);
            let grabbed = self
                .doc
                .openings
                .iter()
                .find(|o| o.id == oid)
                .and_then(|opening| {
                    self.doc
                        .walls
                        .iter()
                        .find(|w| w.id == opening.wall_id)
                        .map(|wall| (opening, wall))
                });
            if let Some((opening, wall)) = grabbed {
                let (_, ux, uy) = wall_axis(wall);
                let world = self.cam.screen_to_world(pos);
                let along =
                    (world.x - wall.start.x) as f64 * ux + (world.y - wall.start.y) as f64 * uy;
                self.opening_drag = Some(OpeningDrag {
                    id: oid,
                    before: opening.offset_mm,
                    grab: along - opening.offset_mm as f64,
                });
            }
            self.box_select = None;
            self.drag = None;
            self.touch();
            return;
        }

        let floor_walls = self.walls_for_floor(self.floor());
        let hit = hit_wall(&floor_walls, &self.cam, pos, 6.0);
        match hit {
            Some(id) => {
                if shift {
                    if let Some(i) = self.selection.iter().position(|s| *s == id) {
                        self.selection.remove(i);
                    } else {
                        self.selection.push(id);
                    }
                } else if !self.selection.contains(&id) {
                    self.selection = vec![id];
                }
                if !shift {
                    self.selected_room = None;
                    self.selected_furniture = None;
                    self.furniture_selection.clear();
                    self.selected_opening = None;
                    self.selected_stair = None;
                }
                self.drag =
                    DragSession::begin(&self.walls_for_floor(self.floor()), &self.selection, pos);
            }
            None => {
                let world = self.pointer_world();
                let furn = self
                    .doc
                    .furniture
                    .iter()
                    .rev()
                    .find(|f| f.floor == self.floor() && furniture_hit(f, world))
                    .map(|f| (f.id, f.center));
                let stair = if shift || furn.is_some() {
                    None
                } else {
                    self.doc
                        .stairs
                        .iter()
                        .rev()
                        .find(|s| s.floor == self.floor() && point_in_stair(s, world, 8.0))
                        .map(|s| s.id)
                };
                let room = if shift || furn.is_some() || stair.is_some() {
                    None
                } else {
                    self.doc
                        .rooms
                        .iter()
                        .find(|r| r.floor == self.floor() && point_in_polygon(world, &r.polygon))
                        .map(|r| r.id)
                };
                if !shift {
                    self.selection.clear();
                    self.selected_room = None;
                    self.selected_opening = None;
                    self.selected_stair = None;
                }
                if let Some((fid, _)) = furn {
                    if shift {
                        if let Some(pos) = self.furniture_selection.iter().position(|id| *id == fid)
                        {
                            self.furniture_selection.remove(pos);
                            self.selected_furniture = self.furniture_selection.first().copied();
                        } else {
                            self.furniture_selection.push(fid);
                            self.selected_furniture = Some(fid);
                        }
                        self.furniture_drag = None;
                        self.box_select = None;
                        self.drag = None;
                    } else {
                        if !self.furniture_selection.contains(&fid) {
                            self.furniture_selection = vec![fid];
                        }
                        self.selected_furniture = Some(fid);
                        self.selected_stair = None;
                        let before = self
                            .furniture_selection
                            .iter()
                            .filter_map(|id| {
                                self.doc
                                    .furniture
                                    .iter()
                                    .find(|f| f.id == *id)
                                    .map(|f| (*id, f.center))
                            })
                            .collect();
                        self.furniture_drag = Some(FurnitureDrag {
                            anchor: fid,
                            start_pointer: pos,
                            scale_at_down: self.cam.scale,
                            started: false,
                            before,
                        });
                        self.box_select = None;
                        self.drag = None;
                    }
                } else if let Some(stair_id) = stair {
                    self.selected_stair = Some(stair_id);
                    self.selected_room = None;
                    self.selected_furniture = None;
                    self.furniture_selection.clear();
                    self.box_select = None;
                    self.drag = None;
                } else if let Some(room_id) = room {
                    self.selected_room = Some(room_id);
                    self.selected_stair = None;
                    if !shift {
                        self.selected_furniture = None;
                        self.furniture_selection.clear();
                    }
                    self.box_select = None;
                    self.drag = None;
                } else {
                    self.box_select = Some(pos);
                    self.drag = None;
                    if !shift {
                        self.selected_furniture = None;
                        self.furniture_selection.clear();
                    }
                }
            }
        }
        self.touch();
    }

    pub fn pointer_move(&mut self, pos: [f32; 2]) {
        self.pointer = pos;
        if self.view == ViewMode::Walk {
            if let Some(anchor) = self.walk_look {
                self.walk.look(pos[0] - anchor[0], pos[1] - anchor[1]);
                self.walk_look = Some(pos);
            }
            return;
        }
        if self.view == ViewMode::Three {
            if let Some(drag) = self.furniture_drag_3d {
                if let Some(hit) = self.ground_point(pos) {
                    let x = ((hit[0] - drag.grab[0]) * 1000.0).round() as i32;
                    let y = ((hit[1] - drag.grab[1]) * 1000.0).round() as i32;
                    let centre = Pt::new(
                        snap_to_grid(x, DEFAULT_GRID_MM),
                        snap_to_grid(y, DEFAULT_GRID_MM),
                    );
                    if let Some(item) = self.doc.furniture.iter().find(|f| f.id == drag.id).cloned()
                    {
                        let mut probe = item.clone();
                        probe.center = centre;
                        if blocked_along_move(
                            &probe,
                            item.center,
                            centre,
                            &self.walls_for_floor(self.floor()),
                        )
                        .is_some()
                            || furniture_path_blocked(
                                &probe,
                                item.center,
                                centre,
                                &self.furniture_for_floor(self.floor()),
                                &[drag.id],
                            )
                            .is_some()
                        {
                            self.note_wall_blocked();
                            return;
                        }
                        if let Some(item) = self.doc.furniture.iter_mut().find(|f| f.id == drag.id)
                        {
                            item.center = centre;
                        }
                        self.touch();
                    }
                }
                return;
            }
            if let Some(anchor) = self.orbit_drag {
                self.cam3.orbit(pos[0] - anchor[0], pos[1] - anchor[1]);
                self.orbit_drag = Some(pos);
            }
            return;
        }
        if let Some(anchor) = self.pan {
            self.cam.pan_px(pos[0] - anchor[0], pos[1] - anchor[1]);
            self.pan = Some(pos);
            self.touch();
            return;
        }
        if let Some(place) = self.placing {
            let dx_px = (pos[0] - place.start_pointer[0]) as f64;
            let dy_px = (pos[1] - place.start_pointer[1]) as f64;
            let factor = place.scale_at_down.max(1e-6);
            let centre = Pt::new(
                place.before.x + (dx_px / factor).round() as i32,
                place.before.y - (dy_px / factor).round() as i32,
            );
            let Some(probe) = self
                .doc
                .furniture
                .iter()
                .find(|f| f.id == place.id)
                .cloned()
            else {
                return;
            };
            if blocked_along_move(
                &probe,
                probe.center,
                centre,
                &self.walls_for_floor(self.floor()),
            )
            .is_some()
                || furniture_path_blocked(
                    &probe,
                    probe.center,
                    centre,
                    &self.furniture_for_floor(self.floor()),
                    &[place.id],
                )
                .is_some()
            {
                self.note_wall_blocked();
                self.touch();
                return;
            }
            if let Some(item) = self.doc.furniture.iter_mut().find(|f| f.id == place.id)
                && item.center != centre
            {
                item.center = centre;
                self.touch();
            }
            return;
        }
        if let Some(drag) = self.rotate_drag {
            if let Some(item) = self.doc.furniture.iter().find(|f| f.id == drag.id) {
                let center = self.cam.world_to_screen(item.center);
                let dx = (pos[0] - center[0]) as f64;
                let dy = (pos[1] - center[1]) as f64;
                if dx.abs() + dy.abs() > 1.0 {
                    let deg = (-dx).atan2(-dy).to_degrees();
                    // Shift gives the original free-angle behaviour; without
                    // it the handle snaps to 5° for tidy plan angles.
                    let rotation = if self.shift_down {
                        deg.rem_euclid(360.0)
                    } else {
                        ((deg / 5.0).round() * 5.0).rem_euclid(360.0)
                    };
                    let Some(base) = self.doc.furniture.iter().find(|f| f.id == drag.id).cloned()
                    else {
                        return;
                    };
                    let mut probe = base;
                    probe.rotation_deg = rotation;
                    if furniture_blocking_wall(&probe, &self.walls_for_floor(self.floor()))
                        .is_some()
                    {
                        self.note_wall_blocked();
                        return;
                    }
                    if let Some(item) = self.doc.furniture.iter_mut().find(|f| f.id == drag.id) {
                        item.rotation_deg = rotation;
                    }
                    self.touch();
                }
            }
            return;
        }
        if let Some(drag) = self.opening_drag {
            let Some(opening) = self.doc.openings.iter().find(|o| o.id == drag.id).cloned() else {
                self.opening_drag = None;
                return;
            };
            let Some(wall) = self
                .doc
                .walls
                .iter()
                .find(|w| w.id == opening.wall_id)
                .cloned()
            else {
                return;
            };
            let (len, ux, uy) = wall_axis(&wall);
            let max = (len - opening.width_mm as f64).max(0.0);
            let world = self.pointer_world();
            let along = (world.x - wall.start.x) as f64 * ux + (world.y - wall.start.y) as f64 * uy;
            let snapped = (((along - drag.grab) / 50.0).round() * 50.0).clamp(0.0, max) as i32;
            if snapped != opening.offset_mm {
                if let Some(item) = self.doc.openings.iter_mut().find(|o| o.id == drag.id) {
                    item.offset_mm = snapped;
                }
                self.note = Some(format!("门窗位置 {snapped}mm"));
                self.touch();
            }
            return;
        }
        if let Some(drag) = self.furniture_drag.clone() {
            let moved = ((pos[0] - drag.start_pointer[0]).powi(2)
                + (pos[1] - drag.start_pointer[1]).powi(2))
            .sqrt();
            // Follow the pointer from the very first pixel. The old 3 px dead
            // zone made the piece feel detached at the start of a drag; the
            // `started` flag now only decides whether release commits history,
            // and a sub-threshold wobble is restored on pointer-up.
            if moved < 0.5 {
                return;
            }
            if !drag.started
                && moved >= 2.0
                && let Some(live) = self.furniture_drag.as_mut()
            {
                live.started = true;
            }
            let anchor_before = drag
                .before
                .iter()
                .find(|(id, _)| *id == drag.anchor)
                .map(|(_, p)| *p)
                .or_else(|| {
                    self.doc
                        .furniture
                        .iter()
                        .find(|f| f.id == drag.anchor)
                        .map(|f| f.center)
                });
            let Some(anchor_before) = anchor_before else {
                return;
            };
            let dx_px = (pos[0] - drag.start_pointer[0]) as f64;
            let dy_px = (pos[1] - drag.start_pointer[1]) as f64;
            let factor = drag.scale_at_down.max(1e-6);
            // No grid or neighbour snapping while the pointer is down: it
            // quantised the piece into visible steps and made it feel sticky.
            let target = Pt::new(
                anchor_before.x + (dx_px / factor).round() as i32,
                anchor_before.y - (dy_px / factor).round() as i32,
            );
            let dx = target.x - anchor_before.x;
            let dy = target.y - anchor_before.y;
            let moving: Vec<EntityId> = drag.before.iter().map(|(id, _)| *id).collect();

            // The whole group must have a clear path. Testing every swept
            // footprint also prevents a fast pointer from teleporting a sofa
            // from one room into the next through a wall, or across another
            // piece of furniture.
            let mut blocked = false;
            for (id, before) in &drag.before {
                let Some(base) = self.doc.furniture.iter().find(|f| f.id == *id) else {
                    continue;
                };
                let target = Pt::new(before.x + dx, before.y + dy);
                if blocked_along_move(base, *before, target, &self.walls_for_floor(self.floor()))
                    .is_some()
                    || furniture_path_blocked(
                        base,
                        *before,
                        target,
                        &self.furniture_for_floor(self.floor()),
                        &moving,
                    )
                    .is_some()
                {
                    blocked = true;
                    break;
                }
            }
            if blocked {
                self.note_wall_blocked();
                self.touch();
                return;
            }

            let mut changed = false;
            for (id, before) in &drag.before {
                if let Some(f) = self.doc.furniture.iter_mut().find(|f| f.id == *id) {
                    let target = Pt::new(before.x + dx, before.y + dy);
                    if f.center != target {
                        f.center = target;
                        changed = true;
                    }
                }
            }
            if changed {
                self.touch();
            }
            return;
        }
        if self.tool == Tool::Measure {
            if self.measure_from.is_some() {
                self.touch();
            }
            return;
        }
        if let Some(drag) = &self.drag {
            if drag.apply(&mut self.doc.walls, &self.cam, pos) {
                self.touch();
            }
            return;
        }
        if self.box_select.is_some() {
            self.touch();
            return;
        }
        let hover = if matches!(self.tool, Tool::Select | Tool::Demolish) {
            {
                let floor_walls = self.walls_for_floor(self.floor());
                hit_wall(&floor_walls, &self.cam, pos, 6.0)
            }
        } else {
            None
        };
        if hover != self.hover || self.draft.is_some() {
            self.hover = hover;
            self.touch();
        }
    }

    /// True while a gesture owns the pointer. Moving over floating chrome in
    /// the middle of a drag must keep feeding the drag, not get swallowed.
    pub fn pointer_captured(&self) -> bool {
        self.furniture_drag.is_some()
            || self.placing.is_some()
            || self.rotate_drag.is_some()
            || self.opening_drag.is_some()
            || self.drag.is_some()
            || self.pan.is_some()
            || self.box_select.is_some()
            || self.wall_pressed
    }

    pub fn pointer_up(&mut self, pos: [f32; 2], button: PointerButton, shift: bool) {
        self.pointer = pos;
        if self.view == ViewMode::Walk {
            self.walk_look = None;
            return;
        }
        if self.view == ViewMode::Three {
            if let Some(drag) = self.furniture_drag_3d.take() {
                let after = self
                    .doc
                    .furniture
                    .iter()
                    .find(|f| f.id == drag.id)
                    .map(|f| f.center)
                    .filter(|c| *c != drag.before);
                if let Some(after) = after {
                    self.history.execute(
                        Box::new(MoveFurniture {
                            id: drag.id,
                            before: drag.before,
                            after,
                        }),
                        &mut self.doc,
                    );
                    self.note = Some(format!("家具移到 {},{}mm", after.x, after.y));
                }
                self.orbit_drag = None;
                self.orbit_start = None;
                return;
            }
            let moved = self
                .orbit_start
                .map(|start| ((pos[0] - start[0]).powi(2) + (pos[1] - start[1]).powi(2)).sqrt())
                .unwrap_or(f32::MAX);
            self.orbit_drag = None;
            self.orbit_start = None;
            if button == PointerButton::Left && moved < 5.0 {
                self.pick_3d(pos);
            }
            return;
        }
        if self.pan.is_some() && (button == PointerButton::Middle || button == PointerButton::Left)
        {
            self.pan = None;
            self.touch();
            return;
        }
        if button == PointerButton::Middle {
            self.pan = None;
            self.touch();
            return;
        }
        if button != PointerButton::Left {
            return;
        }

        if self.tool == Tool::Stair && self.wall_pressed {
            self.wall_pressed = false;
            if let Some(start) = self.draft {
                let end = constrained_end(start, self.pointer_world(), true, DEFAULT_GRID_MM);
                let len = ((end.x - start.x) as f64).hypot((end.y - start.y) as f64);
                if len >= 500.0 {
                    self.commit_stair(start, end);
                    self.draft = None;
                } else {
                    self.note = Some("再点一下确定楼梯终点".into());
                }
                self.touch();
            }
            return;
        }

        if self.tool == Tool::Wall && self.wall_pressed {
            self.wall_pressed = false;
            if let Some(start) = self.draft {
                let end = self.snap_wall_end(start, self.pointer_world());
                let len = ((end.x - start.x) as f64).hypot((end.y - start.y) as f64);
                if len >= 50.0 {
                    self.commit_wall(start, end);
                    self.draft = None;
                } else {
                    // A plain click starts a two-click stroke.
                    self.note = Some("再点一下确定终点".into());
                }
                self.touch();
            }
            return;
        }

        if self.placing.is_some() {
            self.finish_placement();
            return;
        }

        if let Some(drag) = self.opening_drag.take() {
            let after = self
                .doc
                .openings
                .iter()
                .find(|o| o.id == drag.id)
                .map(|o| o.offset_mm)
                .unwrap_or(drag.before);
            if after != drag.before {
                self.history.execute(
                    Box::new(MoveOpening {
                        id: drag.id,
                        before: drag.before,
                        after,
                    }),
                    &mut self.doc,
                );
                self.note = Some(format!("门窗移到 {after}mm"));
            }
            self.touch();
            return;
        }

        if let Some(drag) = self.rotate_drag.take() {
            let after = self
                .doc
                .furniture
                .iter()
                .find(|f| f.id == drag.id)
                .map(|f| f.rotation_deg)
                .filter(|r| (*r - drag.before).abs() > 1e-6);
            if let Some(after) = after {
                self.history.execute(
                    Box::new(RotateFurniture {
                        id: drag.id,
                        before: drag.before,
                        after,
                    }),
                    &mut self.doc,
                );
                self.note = Some(format!("家具转到 {after:.0}°"));
            }
            self.touch();
            return;
        }

        if let Some(drag) = self.furniture_drag.take() {
            if !drag.started {
                // A press with one or two pixels of hand jitter is a click,
                // not a move: put the pieces back where they were.
                let mut restored = false;
                for (id, before) in &drag.before {
                    if let Some(f) = self.doc.furniture.iter_mut().find(|f| f.id == *id)
                        && f.center != *before
                    {
                        f.center = *before;
                        restored = true;
                    }
                }
                if restored {
                    self.touch();
                }
                return;
            }
            let moves: Vec<FurnitureMove> = drag
                .before
                .iter()
                .filter_map(|(id, before)| {
                    self.doc
                        .furniture
                        .iter()
                        .find(|f| f.id == *id)
                        .map(|f| f.center)
                        .filter(|after| *after != *before)
                        .map(|after| FurnitureMove {
                            id: *id,
                            before: *before,
                            after,
                        })
                })
                .collect();
            if !moves.is_empty() {
                let label = if moves.len() == 1 {
                    "move-furniture"
                } else {
                    "move-furniture-group"
                };
                self.history
                    .execute(Box::new(MoveFurnitureGroup { moves, label }), &mut self.doc);
                self.note = Some(if drag.before.len() > 1 {
                    format!("移动 {} 件家具", drag.before.len())
                } else {
                    "家具已移动".into()
                });
            }
        }
        if let Some(drag) = self.drag.take() {
            self.commit_drag(&drag);
        }

        if let Some(anchor) = self.box_select.take() {
            let rect = [
                anchor[0].min(pos[0]),
                anchor[1].min(pos[1]),
                anchor[0].max(pos[0]),
                anchor[1].max(pos[1]),
            ];
            if (rect[2] - rect[0]) > 3.0 || (rect[3] - rect[1]) > 3.0 {
                let picked = walls_in_rect(&self.walls_for_floor(self.floor()), &self.cam, rect);
                if shift {
                    for id in picked {
                        if !self.selection.contains(&id) {
                            self.selection.push(id);
                        }
                    }
                } else {
                    self.selection = picked;
                }
            }
        }
        self.touch();
    }

    fn commit_drag(&mut self, drag: &DragSession) {
        let mut moves = Vec::new();
        for (id, before_start, before_end) in drag.origins() {
            let Some(w) = self.doc.walls.iter().find(|w| w.id == *id) else {
                continue;
            };
            if w.start != *before_start || w.end != *before_end {
                moves.push(WallMove {
                    id: *id,
                    before: [*before_start, *before_end],
                    after: [w.start, w.end],
                });
            }
        }
        if !moves.is_empty() {
            // The document already holds the "after" state; applying again is
            // idempotent and keeps the history consistent.
            self.history.execute(
                Box::new(MoveWalls {
                    moves,
                    before_rooms: None,
                }),
                &mut self.doc,
            );
        }
    }

    pub fn scroll(&mut self, pos: [f32; 2], delta_y: f32) {
        if self.view == ViewMode::Walk {
            self.walk_scroll(delta_y);
            return;
        }
        if self.view == ViewMode::Three {
            // Wheel-up (positive delta) zooms in, matching the 2D plan.
            self.cam3.zoom(-delta_y);
            return;
        }
        let factor = (delta_y as f64 * 0.0018).exp();
        self.cam.zoom_at(pos, factor);
    }

    // -------------------------------------------------------------- keyboard

    pub fn key(&mut self, key: &str, cmd: bool, shift: bool) {
        if key == "space" && !cmd {
            self.set_space_down(true);
            return;
        }
        if self.view == ViewMode::Walk {
            self.walk_fast = shift;
            match key {
                "w" | "W" | "up" | "arrowup" => {
                    self.walk_keys[0] = true;
                    return;
                }
                "s" | "S" | "down" | "arrowdown" => {
                    self.walk_keys[2] = true;
                    return;
                }
                "a" | "A" | "left" | "arrowleft" => {
                    self.walk_keys[1] = true;
                    return;
                }
                "d" | "D" | "right" | "arrowright" => {
                    self.walk_keys[3] = true;
                    return;
                }
                "e" | "E" => {
                    self.toggle_door_near();
                    return;
                }
                "f" | "F" | "escape" | "g" | "G" => {
                    self.set_view(ViewMode::Three);
                    return;
                }
                _ => {}
            }
        }
        match key {
            "z" | "Z" if cmd && shift => {
                if self.history.redo(&mut self.doc) {
                    self.active_level = self
                        .active_level
                        .min(self.doc.level_count().saturating_sub(1));
                    self.prune_selection();
                    self.sync_measurements();
                    self.touch();
                }
            }
            "z" | "Z" if cmd => {
                if self.history.undo(&mut self.doc) {
                    self.active_level = self
                        .active_level
                        .min(self.doc.level_count().saturating_sub(1));
                    self.prune_selection();
                    self.sync_measurements();
                    self.touch();
                }
            }
            "s" | "S" if cmd => self.save(),
            "t" | "T" if !cmd => {
                let next = if self.view == ViewMode::Plan {
                    ViewMode::Three
                } else {
                    ViewMode::Plan
                };
                self.set_view(next);
            }
            "v" | "V" if !cmd => self.set_tool(Tool::Select),
            "+" | "=" if !cmd => self.zoom_steps(1.25),
            "-" if !cmd => self.zoom_steps(0.8),
            "f" | "F" if !cmd => self.fit_view(),
            "g" | "G" if !cmd => {
                let next = if self.view == ViewMode::Walk {
                    ViewMode::Three
                } else {
                    ViewMode::Walk
                };
                self.set_view(next);
            }
            "r" | "R" if !cmd && shift => self.rotate_furniture_by(-90.0),
            "d" | "D" if cmd => {
                self.copy_selection();
                self.paste();
            }
            "escape" => {
                if self.tool == Tool::Measure {
                    self.measure_from = None;
                    self.clear_measurements();
                    self.set_tool(Tool::Select);
                    return;
                }
                self.draft = None;
                self.box_select = None;
                self.selection.clear();
                self.selected_room = None;
                self.selected_furniture = None;
                self.selected_opening = None;
                self.tool = Tool::Select;
            }
            "delete" | "backspace" => self.delete_selection(),
            "c" | "C" if cmd => {
                self.copy_selection();
            }
            "v" | "V" if cmd => self.paste(),
            "m" | "M" if !cmd && !shift => self.set_tool(Tool::Measure),
            "m" | "M" if !cmd && shift => self.cycle_material(),
            "x" | "X" if !cmd => self.set_tool(Tool::Demolish),
            "r" | "R" if !cmd => self.rotate_furniture(),
            "[" if !cmd => self.resize_furniture(-100, 0),
            "]" if !cmd => self.resize_furniture(100, 0),
            "," if !cmd => self.resize_furniture(0, -100),
            "." if !cmd => self.resize_furniture(0, 100),
            "c" | "C" if !cmd => self.recolor_furniture(),
            "d" | "D" if !cmd => {
                self.doc.settings.show_dimensions = !self.doc.settings.show_dimensions;
                self.note = Some(format!(
                    "标注{}",
                    if self.doc.settings.show_dimensions {
                        "开"
                    } else {
                        "关"
                    }
                ));
            }
            "tab" => {
                let current = self.catalog_index;
                let start = current + 1;
                let mut chosen = None;
                for step in 0..CATALOG.len() {
                    let at = (start + step) % CATALOG.len();
                    if self.catalog_filter == FurnitureCategory::All
                        || CATALOG[at].category == self.catalog_filter
                    {
                        chosen = Some(at);
                        break;
                    }
                }
                if let Some(at) = chosen {
                    self.choose_catalog_item(at);
                }
            }
            _ => return,
        }
        self.touch();
    }

    pub fn key_up(&mut self, key: &str, shift: bool) {
        if key == "space" {
            self.set_space_down(false);
            return;
        }
        self.walk_fast = shift;
        match key {
            "w" | "W" | "up" | "arrowup" => self.walk_keys[0] = false,
            "s" | "S" | "down" | "arrowdown" => self.walk_keys[2] = false,
            "a" | "A" | "left" | "arrowleft" => self.walk_keys[1] = false,
            "d" | "D" | "right" | "arrowright" => self.walk_keys[3] = false,
            _ => {}
        }
    }

    pub fn resize_furniture(&mut self, dw: i32, dd: i32) {
        let Some(id) = self.selected_furniture else {
            return;
        };
        let Some(item) = self.doc.furniture.iter().find(|f| f.id == id) else {
            return;
        };
        let before = (item.width_mm, item.depth_mm, item.color.clone());
        let after = (
            (item.width_mm + dw).clamp(200, 6000),
            (item.depth_mm + dd).clamp(200, 6000),
            item.color.clone(),
        );
        if before == after {
            return;
        }
        let mut probe = item.clone();
        probe.width_mm = after.0;
        probe.depth_mm = after.1;
        if furniture_blocking_wall(&probe, &self.walls_for_floor(self.floor())).is_some()
            || furniture_blocking_other(&probe, &self.furniture_for_floor(self.floor()), &[id])
                .is_some()
        {
            self.note_wall_blocked();
            return;
        }
        self.history
            .execute(Box::new(SetFurniture { id, before, after }), &mut self.doc);
    }

    fn set_furniture_size(&mut self, width: Option<i32>, depth: Option<i32>) {
        let Some(id) = self.selected_furniture else {
            return;
        };
        let Some(item) = self.doc.furniture.iter().find(|f| f.id == id) else {
            return;
        };
        let before = (item.width_mm, item.depth_mm, item.color.clone());
        let after = (
            width.map(|v| v.clamp(200, 6000)).unwrap_or(item.width_mm),
            depth.map(|v| v.clamp(200, 6000)).unwrap_or(item.depth_mm),
            item.color.clone(),
        );
        if before == after {
            return;
        }
        let mut probe = item.clone();
        probe.width_mm = after.0;
        probe.depth_mm = after.1;
        if furniture_blocking_wall(&probe, &self.walls_for_floor(self.floor())).is_some()
            || furniture_blocking_other(&probe, &self.furniture_for_floor(self.floor()), &[id])
                .is_some()
        {
            self.note_wall_blocked();
            return;
        }
        self.history
            .execute(Box::new(SetFurniture { id, before, after }), &mut self.doc);
    }

    fn set_furniture_position(&mut self, x: Option<i32>, y: Option<i32>) {
        let Some(id) = self.selected_furniture else {
            return;
        };
        let Some(item) = self.doc.furniture.iter().find(|f| f.id == id) else {
            return;
        };
        let before = item.center;
        let after = Pt::new(
            x.map(|v| v.clamp(-100_000, 100_000)).unwrap_or(before.x),
            y.map(|v| v.clamp(-100_000, 100_000)).unwrap_or(before.y),
        );
        if before == after {
            return;
        }
        let mut probe = item.clone();
        probe.center = after;
        if furniture_blocking_wall(&probe, &self.walls_for_floor(self.floor())).is_some()
            || furniture_blocking_other(&probe, &self.furniture_for_floor(self.floor()), &[id])
                .is_some()
        {
            self.note_wall_blocked();
            return;
        }
        self.history
            .execute(Box::new(MoveFurniture { id, before, after }), &mut self.doc);
    }

    fn set_furniture_price(&mut self, price: i32) {
        let Some(id) = self.selected_furniture else {
            return;
        };
        let Some(item) = self.doc.furniture.iter().find(|f| f.id == id) else {
            return;
        };
        let after = Some(price.clamp(0, 1_000_000) as f64);
        if item.price == after {
            return;
        }
        self.history.execute(
            Box::new(SetFurniturePrice {
                id,
                before: item.price,
                after,
            }),
            &mut self.doc,
        );
        self.note = Some(format!("家具单价改为 ¥{price}"));
    }

    fn set_furniture_color(&mut self, hex: &'static str) {
        let Some(id) = self.selected_furniture else {
            return;
        };
        let Some(item) = self.doc.furniture.iter().find(|f| f.id == id) else {
            return;
        };
        if item.color.eq_ignore_ascii_case(hex) {
            return;
        }
        let before = (item.width_mm, item.depth_mm, item.color.clone());
        let after = (item.width_mm, item.depth_mm, hex.to_string());
        self.history
            .execute(Box::new(SetFurniture { id, before, after }), &mut self.doc);
    }

    fn recolor_furniture(&mut self) {
        const PALETTE: [&str; 6] = [
            "#8b6f55", "#a9b3c4", "#5f6673", "#b7a58c", "#c9ced6", "#9c8468",
        ];
        let Some(id) = self.selected_furniture else {
            return;
        };
        let Some(item) = self.doc.furniture.iter().find(|f| f.id == id) else {
            return;
        };
        let before = (item.width_mm, item.depth_mm, item.color.clone());
        let idx = PALETTE
            .iter()
            .position(|c| *c == item.color)
            .map(|i| i + 1)
            .unwrap_or(0);
        let after = (
            item.width_mm,
            item.depth_mm,
            PALETTE[idx % PALETTE.len()].to_string(),
        );
        self.history
            .execute(Box::new(SetFurniture { id, before, after }), &mut self.doc);
    }

    fn rotate_furniture(&mut self) {
        self.rotate_furniture_by(90.0);
    }

    fn rotate_furniture_by(&mut self, delta_deg: f64) {
        let Some(id) = self.selected_furniture else {
            return;
        };
        let Some(item) = self.doc.furniture.iter().find(|f| f.id == id) else {
            return;
        };
        let before = item.rotation_deg;
        let after = (before + delta_deg).rem_euclid(360.0);
        let mut probe = item.clone();
        probe.rotation_deg = after;
        if furniture_blocking_wall(&probe, &self.walls_for_floor(self.floor())).is_some() {
            self.note_wall_blocked();
            return;
        }
        self.history.execute(
            Box::new(RotateFurniture { id, before, after }),
            &mut self.doc,
        );
    }

    fn move_furniture_layer(&mut self, front: bool) {
        let Some(id) = self.selected_furniture else {
            return;
        };
        let Some(before) = self.doc.furniture.iter().position(|f| f.id == id) else {
            return;
        };
        let after = if front {
            self.doc.furniture.len().saturating_sub(1)
        } else {
            0
        };
        if before == after {
            return;
        }
        self.history.execute(
            Box::new(ReorderFurniture { id, before, after }),
            &mut self.doc,
        );
        self.note = Some(if front {
            "家具置顶".to_string()
        } else {
            "家具置底".to_string()
        });
    }

    fn flip_opening_hinge(&mut self) {
        let Some(id) = self.selected_opening else {
            return;
        };
        let Some(opening) = self.doc.openings.iter().find(|o| o.id == id) else {
            return;
        };
        if opening.kind != OpeningKind::Door {
            return;
        }
        let before = opening.hinge_right;
        self.history.execute(
            Box::new(SetOpeningHinge {
                id,
                before,
                after: !before,
            }),
            &mut self.doc,
        );
        self.note = Some(format!("门铰链改到{}侧", if before { "左" } else { "右" }));
    }

    fn selected_furniture_ids(&self) -> Vec<EntityId> {
        if let Some(primary) = self
            .selected_furniture
            .filter(|primary| !self.furniture_selection.contains(primary))
        {
            return vec![primary];
        }
        if !self.furniture_selection.is_empty() {
            self.furniture_selection.clone()
        } else {
            self.selected_furniture.into_iter().collect()
        }
    }

    fn align_furniture(&mut self, axis: u8, mode: u8) {
        let ids = self.selected_furniture_ids();
        if ids.len() < 2 {
            return;
        }
        let mut bounds = Vec::new();
        for id in &ids {
            if let Some(item) = self.doc.furniture.iter().find(|f| f.id == *id) {
                let (min, max) = furniture_bounds(item);
                bounds.push((*id, item.center, min, max));
            }
        }
        if bounds.len() < 2 {
            return;
        }
        let (lo, hi) = if axis == 0 {
            (
                bounds.iter().map(|(_, _, min, _)| min.x).min().unwrap_or(0),
                bounds.iter().map(|(_, _, _, max)| max.x).max().unwrap_or(0),
            )
        } else {
            (
                bounds.iter().map(|(_, _, min, _)| min.y).min().unwrap_or(0),
                bounds.iter().map(|(_, _, _, max)| max.y).max().unwrap_or(0),
            )
        };
        let target = match mode {
            0 => lo,
            2 => hi,
            _ => (lo + hi) / 2,
        };
        let mut moves = Vec::new();
        for (id, before, min, max) in bounds {
            let current = if axis == 0 {
                match mode {
                    0 => min.x,
                    2 => max.x,
                    _ => (min.x + max.x) / 2,
                }
            } else {
                match mode {
                    0 => min.y,
                    2 => max.y,
                    _ => (min.y + max.y) / 2,
                }
            };
            let delta = target - current;
            if delta.abs() < 1 {
                continue;
            }
            let after = if axis == 0 {
                Pt::new(before.x + delta, before.y)
            } else {
                Pt::new(before.x, before.y + delta)
            };
            moves.push(FurnitureMove { id, before, after });
        }
        if moves.is_empty() {
            return;
        }
        for mv in &moves {
            let Some(base) = self.doc.furniture.iter().find(|f| f.id == mv.id) else {
                continue;
            };
            let mut probe = base.clone();
            probe.center = mv.after;
            if blocked_along_move(
                &probe,
                mv.before,
                mv.after,
                &self.walls_for_floor(self.floor()),
            )
            .is_some()
            {
                self.note_wall_blocked();
                return;
            }
        }
        let count = moves.len();
        self.history.execute(
            Box::new(MoveFurnitureGroup {
                moves,
                label: "align-furniture",
            }),
            &mut self.doc,
        );
        self.note = Some(format!("已对齐 {count} 件家具"));
    }

    pub fn project_info(&self) -> (String, String, String, String) {
        (
            self.doc.meta.name.clone(),
            self.doc.meta.client.clone(),
            self.doc.meta.address.clone(),
            self.doc.meta.note.clone(),
        )
    }

    /// Update the document header and persist immediately; returns a note for the UI.
    pub fn update_project_info(
        &mut self,
        name: &str,
        client: &str,
        address: &str,
        note: &str,
    ) -> Result<(), String> {
        if !name.trim().is_empty() {
            self.doc.meta.name = name.trim().to_owned();
        }
        self.doc.meta.client = client.trim().to_owned();
        self.doc.meta.address = address.trim().to_owned();
        self.doc.meta.note = note.trim().to_owned();
        self.touch();
        let result = self.write_doc();
        if result.is_ok() {
            self.saved_revision = self.revision;
            self.dirty = false;
            crate::recent::remember(&self.saved_path, &self.doc.meta.name);
        }
        self.note = Some(match &result {
            Ok(()) => format!("方案信息已保存 · {}", self.doc.meta.name),
            Err(e) => format!("方案信息保存失败 · {e}"),
        });
        result
    }

    pub fn quote_settings(&self) -> QuoteSettings {
        self.doc.quote.clone()
    }

    /// Update labour/auxiliary/tax/discount rates. Values are already in yuan
    /// per m² and fractions (0.09 = 9%); the UI parses percentages before this.
    pub fn update_quote_settings(
        &mut self,
        labor_per_m2: f64,
        auxiliary_per_m2: f64,
        tax_rate: f64,
        discount_rate: f64,
    ) {
        let after = QuoteSettings {
            labor_per_m2: labor_per_m2.clamp(0.0, 10_000.0),
            auxiliary_per_m2: auxiliary_per_m2.clamp(0.0, 10_000.0),
            tax_rate: tax_rate.clamp(0.0, 0.5),
            discount_rate: discount_rate.clamp(0.0, 0.5),
        };
        if self.doc.quote == after {
            return;
        }
        let before = self.doc.quote.clone();
        self.history
            .execute(Box::new(SetQuoteSettings { before, after }), &mut self.doc);
        self.note = Some("报价费率已更新".into());
        self.touch();
    }

    fn flip_opening_swing(&mut self) {
        let Some(id) = self.selected_opening else {
            return;
        };
        let Some(opening) = self.doc.openings.iter().find(|o| o.id == id) else {
            return;
        };
        if opening.kind != OpeningKind::Door {
            return;
        }
        let before = opening.swing_deg;
        let after = if before >= 0.0 { -90.0 } else { 90.0 };
        self.history.execute(
            Box::new(SetOpeningSwing { id, before, after }),
            &mut self.doc,
        );
        self.note = Some("门开启方向已反向".to_string());
    }

    pub fn furniture_count(&self) -> usize {
        self.doc.furniture.len()
    }

    pub fn clear_furniture(&mut self) {
        if self.doc.furniture.is_empty() {
            return;
        }
        let before = self.doc.furniture.clone();
        self.history
            .execute(Box::new(ClearFurniture { before }), &mut self.doc);
        self.selected_furniture = None;
        self.furniture_selection.clear();
        self.note = Some("家具已清空".into());
        self.touch();
    }

    fn cycle_material(&mut self) {
        let Some(room_id) = self.selected_room else {
            return;
        };
        let floor_ids: Vec<EntityId> = self
            .doc
            .materials
            .iter()
            .filter(|m| m.surface == FinishSurface::Floor)
            .map(|m| m.id)
            .collect();
        if floor_ids.is_empty() {
            return;
        }
        let Some(room) = self.doc.rooms.iter().find(|r| r.id == room_id) else {
            return;
        };
        let before = room.material_id;
        let current = before.and_then(|id| floor_ids.iter().position(|x| *x == id));
        let next = match current {
            Some(i) => (i + 1) % floor_ids.len(),
            None => 0,
        };
        let after = Some(floor_ids[next]);
        if before == after {
            return;
        }
        self.history.execute(
            Box::new(SetRoomMaterial {
                room_id,
                before,
                after,
            }),
            &mut self.doc,
        );
    }

    /// Choose a specific flooring swatch, undoable like the cycle shortcut.
    /// Add a blank custom material and return its id.
    pub fn add_material(&mut self) -> EntityId {
        let before = self.doc.materials.clone();
        let id = EntityId(self.doc.next_id());
        let mut after = before.clone();
        after.push(Material {
            id,
            name: "新材料".into(),
            price_per_m2: 0.0,
            kind: "wood".into(),
            color: "#c49a6c".into(),
            surface: fp_model::FinishSurface::Floor,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
        });
        self.history.execute(
            Box::new(MaterialSnapshot {
                before,
                after,
                label: "add material",
            }),
            &mut self.doc,
        );
        self.note = Some("已新增材料，可修改名称、单价和纹理".into());
        self.touch();
        id
    }

    /// Replace one material. The id is kept; rooms referencing it update
    /// automatically because they store only the id.
    pub fn update_material(&mut self, after: Material) {
        let Some(index) = self.doc.materials.iter().position(|m| m.id == after.id) else {
            return;
        };
        if self.doc.materials[index] == after {
            return;
        }
        let before = self.doc.materials.clone();
        let mut next = before.clone();
        next[index] = after;
        self.history.execute(
            Box::new(MaterialSnapshot {
                before,
                after: next,
                label: "edit material",
            }),
            &mut self.doc,
        );
        self.note = Some("材料已更新".into());
        self.touch();
    }

    /// Delete an unused material. The last library entry is protected.
    pub fn remove_material(&mut self, id: EntityId) {
        let used = self.material_usage(id);
        if used > 0 {
            self.note = Some(format!("该饰面仍被 {used} 处使用，先改对应材质"));
            return;
        }
        if self.doc.materials.len() <= 1 {
            self.note = Some("至少保留一种材料".into());
            return;
        }
        let before = self.doc.materials.clone();
        let after: Vec<Material> = before.iter().filter(|m| m.id != id).cloned().collect();
        self.history.execute(
            Box::new(MaterialSnapshot {
                before,
                after,
                label: "remove material",
            }),
            &mut self.doc,
        );
        self.note = Some("材料已删除".into());
        self.touch();
    }

    pub fn material_usage(&self, id: EntityId) -> usize {
        self.material_usage_info(id).0
    }

    /// Count and surface area for one finish, across floors, walls and
    /// ceilings. The biller adds the material's own waste rate on top.
    pub fn material_usage_info(&self, id: EntityId) -> (usize, f64) {
        let mut count = 0usize;
        let mut area = 0.0f64;
        for room in self.doc.rooms.iter().filter(|r| r.material_id == Some(id)) {
            count += 1;
            area += fp_geom::area(&room.polygon) / 1_000_000.0;
        }
        for room in self
            .doc
            .rooms
            .iter()
            .filter(|r| r.ceiling_material_id == Some(id))
        {
            count += 1;
            area += fp_geom::area(&room.polygon) / 1_000_000.0;
        }
        for wall in self
            .doc
            .walls
            .iter()
            .filter(|w| w.finish_id == Some(id))
        {
            count += 1;
            let length_mm =
                ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
            area += length_mm / 1000.0 * wall.height_mm as f64 / 1000.0;
        }
        (count, area)
    }

    pub fn material(&self, id: EntityId) -> Option<&Material> {
        self.doc.materials.iter().find(|m| m.id == id)
    }

    pub fn set_room_material(&mut self, material: EntityId) {
        let Some(room_id) = self.selected_room else {
            return;
        };
        let Some(room) = self.doc.rooms.iter().find(|r| r.id == room_id) else {
            return;
        };
        if !self
            .doc
            .materials
            .iter()
            .any(|m| m.id == material && m.surface == FinishSurface::Floor)
        {
            return;
        }
        let before = room.material_id;
        if before == Some(material) {
            return;
        }
        self.history.execute(
            Box::new(SetRoomMaterial {
                room_id,
                before,
                after: Some(material),
            }),
            &mut self.doc,
        );
        self.touch();
    }

    /// Copy the selected walls, furniture and opening into the clipboard.
    fn copy_selection(&mut self) -> bool {
        let mut clip = Clipboard::default();
        for id in &self.selection {
            if let Some(wall) = self.doc.walls.iter().find(|w| w.id == *id) {
                clip.walls.push(wall.clone());
            }
        }
        for id in self.selected_furniture_ids() {
            if let Some(item) = self.doc.furniture.iter().find(|f| f.id == id) {
                clip.furniture.push(item.clone());
            }
        }
        if let Some(opening) = self
            .selected_opening
            .and_then(|id| self.doc.openings.iter().find(|o| o.id == id))
        {
            clip.openings.push(opening.clone());
        }
        if clip.is_empty() {
            self.note = Some("没有可复制的对象".to_string());
            return false;
        }
        self.clipboard = Some(clip);
        self.paste_count = 0;
        self.note = Some("已复制".to_string());
        true
    }

    /// Paste the clipboard shifted by one grid step, giving every copy a fresh id.
    fn paste(&mut self) {
        let Some(clip) = self.clipboard.clone() else {
            self.note = Some("剪贴板是空的".to_string());
            return;
        };
        self.paste_count += 1;
        let offset = Pt::new(300 * self.paste_count as i32, 300 * self.paste_count as i32);
        let mut raw = self.doc.next_id();
        let mut fresh = || {
            let id = EntityId(raw);
            raw += 1;
            id
        };
        let mut walls = Vec::new();
        let mut remap = std::collections::HashMap::new();
        for wall in &clip.walls {
            let id = fresh();
            remap.insert(wall.id, id);
            let mut copy = wall.clone();
            copy.id = id;
            copy.floor = self.floor();
            copy.start = Pt::new(wall.start.x + offset.x, wall.start.y + offset.y);
            copy.end = Pt::new(wall.end.x + offset.x, wall.end.y + offset.y);
            walls.push(copy);
        }
        let mut furniture = Vec::new();
        let mut placed = self.doc.furniture.clone();
        for item in &clip.furniture {
            let mut copy = item.clone();
            copy.id = fresh();
            copy.floor = self.floor();
            let desired = Pt::new(item.center.x + offset.x, item.center.y + offset.y);
            // A paste must never be born stacked on top of another piece or
            // inside a wall. Nudge the copy to the nearest clear spot.
            if let Some(clear) = self.clear_center_against(&copy, desired, &placed) {
                copy.center = clear;
            } else {
                copy.center = desired;
            }
            copy.room_id = None;
            placed.push(copy.clone());
            furniture.push(copy);
        }
        let mut openings = Vec::new();
        for opening in &clip.openings {
            let wall_id = remap
                .get(&opening.wall_id)
                .copied()
                .unwrap_or(opening.wall_id);
            let wall_exists = walls.iter().any(|w| w.id == wall_id)
                || self
                    .doc
                    .walls
                    .iter()
                    .any(|w| w.id == wall_id && w.floor == self.floor());
            if !wall_exists {
                continue;
            }
            let mut copy = opening.clone();
            copy.id = fresh();
            copy.wall_id = wall_id;
            openings.push(copy);
        }
        let wall_ids: Vec<EntityId> = walls.iter().map(|w| w.id).collect();
        let furniture_ids: Vec<EntityId> = furniture.iter().map(|f| f.id).collect();
        let opening_ids: Vec<EntityId> = openings.iter().map(|o| o.id).collect();
        self.history.execute(
            Box::new(PasteBundle {
                walls,
                furniture,
                openings,
                before_rooms: None,
            }),
            &mut self.doc,
        );
        self.selection = wall_ids;
        self.selected_furniture = furniture_ids.first().copied();
        self.furniture_selection = furniture_ids;
        self.selected_opening = opening_ids.first().copied();
        self.selected_room = None;
        self.note = Some("已粘贴".to_string());
        self.touch();
    }

    fn delete_selection(&mut self) {
        let ids = self.selected_furniture_ids();
        if !ids.is_empty() {
            let items: Vec<(usize, Furniture)> = ids
                .iter()
                .filter_map(|id| {
                    self.doc
                        .furniture
                        .iter()
                        .position(|f| f.id == *id)
                        .map(|index| (index, self.doc.furniture[index].clone()))
                })
                .collect();
            if !items.is_empty() {
                self.history
                    .execute(Box::new(RemoveFurnitureBatch { items }), &mut self.doc);
                self.selected_furniture = None;
                self.furniture_selection.clear();
                self.touch();
                return;
            }
        }
        if let Some(sid) = self.selected_stair.take() {
            if let Some(stair) = self.doc.stairs.iter().find(|s| s.id == sid).cloned() {
                self.history
                    .execute(Box::new(RemoveStair { stair }), &mut self.doc);
                self.touch();
                return;
            }
        }
        let closed = self
            .selected_opening
            .take()
            .and_then(|oid| self.doc.openings.iter().position(|o| o.id == oid))
            .map(|pos| self.doc.openings.remove(pos));
        if let Some(opening) = closed {
            self.history
                .execute(Box::new(RemoveOpening { opening }), &mut self.doc);
            self.touch();
            return;
        }
        if self.selection.is_empty() {
            return;
        }
        let walls: Vec<Wall> = self
            .doc
            .walls
            .iter()
            .filter(|w| self.selection.contains(&w.id))
            .cloned()
            .collect();
        if walls.is_empty() {
            return;
        }
        let ids: Vec<EntityId> = walls.iter().map(|w| w.id).collect();
        let openings = self
            .doc
            .openings
            .iter()
            .filter(|o| ids.contains(&o.wall_id))
            .cloned()
            .collect();
        self.history.execute(
            Box::new(RemoveWalls {
                walls,
                openings,
                before_rooms: None,
            }),
            &mut self.doc,
        );
        self.selection.retain(|id| !ids.contains(id));
    }

    fn prune_selection(&mut self) {
        let ids: Vec<EntityId> = self.doc.walls.iter().map(|w| w.id).collect();
        self.selection.retain(|id| ids.contains(id));
        if self
            .selected_room
            .is_some_and(|id| !self.doc.rooms.iter().any(|r| r.id == id))
        {
            self.selected_room = None;
        }
        let furniture_ids: Vec<EntityId> = self.doc.furniture.iter().map(|f| f.id).collect();
        self.furniture_selection
            .retain(|id| furniture_ids.contains(id));
        if self
            .selected_furniture
            .is_some_and(|id| !furniture_ids.contains(&id))
        {
            self.selected_furniture = self.furniture_selection.first().copied();
        }
        if self
            .selected_opening
            .is_some_and(|id| !self.doc.openings.iter().any(|o| o.id == id))
        {
            self.selected_opening = None;
        }
    }

    /// Place a catalog item directly from a library drag at a viewport point.
    pub fn place_furniture_at(&mut self, index: usize, screen: [f32; 2]) {
        self.catalog_index = index % CATALOG.len();
        self.tool = Tool::Furniture;
        self.furniture_armed = true;
        self.pointer = screen;
        self.pointer_down(screen, PointerButton::Left, false);
        self.finish_placement();
        self.touch();
    }

    /// Opening the library takes the rail out of wall/window mode, but
    /// deliberately does not arm the first catalogue row.
    pub fn begin_furniture_browse(&mut self) {
        self.tool = Tool::Furniture;
        self.furniture_armed = false;
        self.draft = None;
        self.selection.clear();
        self.selected_room = None;
        self.selected_furniture = None;
        self.furniture_selection.clear();
        self.selected_opening = None;
        self.note = Some("请选择一件家具".into());
        self.touch();
    }

    /// Arm a specific catalog entry. Clicking a category or a row is an
    /// explicit "I want to place furniture", so it must also take the tool out
    /// of wall/window mode and drop any plan selection; otherwise the old tool
    /// stays highlighted while the first sofa row already looks selected.
    pub fn choose_catalog_item(&mut self, index: usize) {
        let index = index % CATALOG.len();
        self.catalog_index = index;
        self.tool = Tool::Furniture;
        self.furniture_armed = true;
        self.draft = None;
        self.selection.clear();
        self.selected_room = None;
        self.selected_furniture = None;
        self.furniture_selection.clear();
        self.selected_opening = None;
        self.note = Some(format!("家具：{}", CATALOG[index].name));
        self.touch();
    }

    /// Switch the furniture library category and arm its first item.
    pub fn set_catalog_filter(&mut self, category: FurnitureCategory) {
        self.catalog_filter = category;
        if category == FurnitureCategory::All {
            let current = self.catalog_index;
            self.choose_catalog_item(current);
            return;
        }
        if let Some((index, _)) = CATALOG
            .iter()
            .enumerate()
            .find(|(_, spec)| spec.category == category)
        {
            self.choose_catalog_item(index);
        }
    }

    /// Plan centre (metres) and half-diagonal radius, used to frame the whole flat.
    fn scene_centre_radius(&self) -> (Vec3, f32) {
        let (min, max) = plan_bounds(&self.doc);
        let centre = Vec3::new(
            (min.x + max.x) as f32 * 0.0005,
            0.0,
            (min.y + max.y) as f32 * 0.0005,
        );
        let radius = ((max.x - min.x) as f32).hypot((max.y - min.y) as f32) * 0.0005;
        (centre, radius.max(2.0))
    }

    /// Zoom the 2D view around the viewport centre (keyboard +/-).
    pub fn zoom_steps(&mut self, factor: f64) {
        if self.view != ViewMode::Plan {
            return;
        }
        let centre = [self.view_size[0] * 0.5, self.view_size[1] * 0.5];
        self.cam.zoom_at(centre, factor);
    }

    /// Current architectural scale, assuming the conventional CSS 96 dpi.
    /// The wheel can land between presets, so this is a real number.
    pub fn scale_denominator(&self) -> f64 {
        const CSS_MM_PER_PX: f64 = 25.4 / 96.0;
        1.0 / (self.cam.scale * CSS_MM_PER_PX).max(f64::MIN_POSITIVE)
    }

    /// Set the 2D view to a true 1:60 / 1:100 architectural scale. Unlike a
    /// screenshot the view stays pannable and editable at that scale.
    pub fn set_scale_denominator(&mut self, denominator: f64) {
        const CSS_MM_PER_PX: f64 = 25.4 / 96.0;
        self.cam.scale = (1.0 / (denominator.max(1.0) * CSS_MM_PER_PX)).clamp(MIN_SCALE, MAX_SCALE);
    }

    /// Space-to-pan state from the keyboard layer.
    pub fn set_space_down(&mut self, down: bool) {
        self.space_down = down;
        if !down && self.pan.is_some() {
            self.pan = None;
            self.touch();
        }
    }

    /// Time-of-day slider in the 3D panel; 0 = 06:00, 1 = 18:00.
    pub fn set_sun(&mut self, value: f32) {
        self.sun = value.clamp(0.0, 1.0);
    }

    /// Frame the whole plan in whichever view is active (F).
    pub fn fit_view(&mut self) {
        let (min, max) = plan_bounds(&self.doc);
        self.cam.fit(min, max, 80.0);
        self.cam3.fit(
            Vec3::new(min.x as f32 / 1000.0, 0.0, min.y as f32 / 1000.0),
            Vec3::new(max.x as f32 / 1000.0, 3.0, max.y as f32 / 1000.0),
        );
        self.note = Some("适应窗口".into());
    }

    /// Sun-driven lighting for the 3D pass; night swaps to a cold, dim moon.
    /// The selected piece when the select tool is active.
    fn selected_furniture_item(&self) -> Option<&Furniture> {
        if self.tool != Tool::Select {
            return None;
        }
        self.selected_furniture
            .and_then(|id| self.doc.furniture.iter().find(|f| f.id == id))
    }

    /// Screen position of the rotation handle above a selected piece of furniture.
    fn furniture_handle(&self, item: &Furniture) -> [f32; 2] {
        let center = self.cam.world_to_screen(item.center);
        let r = item.rotation_deg.to_radians() as f32;
        let half = (item.width_mm.max(item.depth_mm) as f32 * 0.5 * self.cam.scale as f32) + 22.0;
        [center[0] - r.sin() * half, center[1] - r.cos() * half]
    }

    /// World-space midpoint of a door, at handle height.
    fn door_midpoint(&self, opening: &Opening, wall: &Wall) -> Option<Vec3> {
        let len = ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
        if len < 1.0 {
            return None;
        }
        let dx = (wall.end.x - wall.start.x) as f32 / len as f32;
        let dz = (wall.end.y - wall.start.y) as f32 / len as f32;
        let along = (opening.offset_mm as f32 + opening.width_mm as f32 * 0.5) / 1000.0;
        Some(Vec3::new(
            wall.start.x as f32 / 1000.0 + dx * along,
            1.05,
            wall.start.y as f32 / 1000.0 + dz * along,
        ))
    }

    /// A door whose handle projects within 34 px of the click. This makes
    /// walking interactive beyond the E key: click the door itself.
    fn door_at_screen(&self, pos: [f32; 2]) -> Option<u128> {
        if self.view != ViewMode::Walk || self.view_size[1] <= 1.0 {
            return None;
        }
        let aspect = self.view_size[0] / self.view_size[1];
        let vp = self.walk.view_proj(aspect);
        let mut best: Option<(f32, u128)> = None;
        for opening in self
            .doc
            .openings
            .iter()
            .filter(|o| o.kind == OpeningKind::Door)
        {
            let Some(wall) = self.doc.walls.iter().find(|w| w.id == opening.wall_id) else {
                continue;
            };
            let Some(world) = self.door_midpoint(opening, wall) else {
                continue;
            };
            let distance = (world - self.walk.pos).length();
            if distance > 3.5 {
                continue;
            }
            let clip = vp * world.extend(1.0);
            if clip.w <= 0.0 {
                continue;
            }
            let ndc = clip.truncate() / clip.w;
            if !(-1.2..=1.2).contains(&ndc.x) || !(-1.2..=1.2).contains(&ndc.y) {
                continue;
            }
            let screen = [
                (ndc.x * 0.5 + 0.5) * self.view_size[0],
                (1.0 - (ndc.y * 0.5 + 0.5)) * self.view_size[1],
            ];
            let gap = ((screen[0] - pos[0]).powi(2) + (screen[1] - pos[1]).powi(2)).sqrt();
            if gap <= 34.0 && best.is_none_or(|(best_gap, _)| gap < best_gap) {
                best = Some((gap, opening.id.0));
            }
        }
        best.map(|(_, id)| id)
    }

    fn toggle_door(&mut self, id: u128) {
        match self.doors_open.iter().position(|x| *x == id) {
            Some(index) => {
                self.doors_open.remove(index);
                self.note = Some("门已关闭".into());
            }
            None => {
                self.doors_open.push(id);
                self.note = Some("门已打开".into());
            }
        }
        self.touch();
    }

    /// Open or close the closest door when walking, within 1.6 m.
    fn toggle_door_near(&mut self) {
        if self.view != ViewMode::Walk {
            return;
        }
        let p = self.walk.pos;
        let mut best: Option<(f32, u128)> = None;
        for opening in self
            .doc
            .openings
            .iter()
            .filter(|o| o.kind == OpeningKind::Door)
        {
            let Some(wall) = self.doc.walls.iter().find(|w| w.id == opening.wall_id) else {
                continue;
            };
            let Some(world) = self.door_midpoint(opening, wall) else {
                continue;
            };
            let dist = ((world.x - p.x).powi(2) + (world.z - p.z).powi(2)).sqrt();
            if dist < 1.6 && best.is_none_or(|(best_dist, _)| dist < best_dist) {
                best = Some((dist, opening.id.0));
            }
        }
        if let Some((_, id)) = best {
            self.toggle_door(id);
        }
    }

    pub fn look3d(&self) -> crate::render3d::Look {
        // 0..1 maps to 06:00..18:00. The sun rises in the east, peaks at noon
        // and sets in the west; the altitude drives both shadow length and the
        // warmth of the direct light.
        let t = self.sun.clamp(0.0, 1.0);
        let arc = (std::f32::consts::PI * t).sin().max(0.015);
        let azimuth = (t - 0.5) * std::f32::consts::PI;
        let light = [azimuth.sin(), -arc.max(0.05), 0.35];
        let mut lamps = [[0.0f32; 4]; 8];
        let mut lamp_count = 0usize;
        if self.night {
            for item in &self.doc.furniture {
                let item_h = fp_model::catalog::CATALOG
                    .iter()
                    .find(|s| s.asset_id == item.asset_id)
                    .map(|s| s.height_mm as f32 / 1000.0)
                    .unwrap_or(0.6);
                let (lx, ly, lz) = match item.asset_id.as_str() {
                    "floor_lamp" => (0.0, 1.35, 0.0),
                    "nightstand" => (
                        -item.width_mm as f32 / 2000.0 + 0.2,
                        item_h + 0.3,
                        -item.depth_mm as f32 / 2000.0 + 0.2,
                    ),
                    "desk" => (
                        -item.width_mm as f32 / 2000.0 + 0.2,
                        item_h + 0.3,
                        -item.depth_mm as f32 / 2000.0 + 0.14,
                    ),
                    _ => continue,
                };
                if lamp_count >= lamps.len() {
                    break;
                }
                let (sin, cos) = (item.rotation_deg as f32).to_radians().sin_cos();
                let cx = item.center.x as f32 / 1000.0;
                let cz = item.center.y as f32 / 1000.0;
                lamps[lamp_count] = [cx + lx * cos - lz * sin, ly, cz + lx * sin + lz * cos, 4.0];
                lamp_count += 1;
            }
        }
        let look = if self.night {
            crate::render3d::Look {
                light: [light[0] * 0.55, -0.55, light[2] * 0.55],
                ambient: 0.055,
                diffuse: 0.19,
                tint: [0.52, 0.60, 0.95],
                exposure: 0.72,
                ssao: 0.30,
                clear: [0.015, 0.020, 0.048],
                lamps,
                lamp_count: lamp_count as f32,
                ..crate::render3d::Look::day()
            }
        } else {
            crate::render3d::Look {
                light,
                ambient: 0.50 + 0.18 * arc,
                diffuse: 0.25 + 0.24 * arc,
                tint: [1.0, 0.84 + 0.14 * arc, 0.70 + 0.27 * arc],
                exposure: 0.76,
                ssao: 0.40,
                // Match the plan sheet's warm paper background instead of
                // dropping the whole window into a dark "night" look.
                clear: [0.965, 0.957, 0.941],
                lamps,
                lamp_count: lamp_count as f32,
                ..crate::render3d::Look::day()
            }
        };
        let (min, max) = plan_bounds(&self.doc);
        let cx = (min.x + max.x) as f32 / 2000.0;
        let cz = (min.y + max.y) as f32 / 2000.0;
        let dx = (max.x - min.x).max(1) as f32 / 1000.0;
        let dz = (max.y - min.y).max(1) as f32 / 1000.0;
        let radius = (dx.max(dz) * 0.75 + 3.0).clamp(4.0, 40.0);
        let dir = glam::Vec3::from(look.light).normalize();
        let centre = glam::Vec3::new(cx, 1.4, cz);
        let eye = centre - dir * (radius * 2.6);
        let vp = glam::Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.1, radius * 5.4)
            * glam::Mat4::look_at_rh(eye, centre, glam::Vec3::Z);
        let mut look = look;
        look.light_vp = vp.to_cols_array_2d();
        look.shadows = 0.82;
        look
    }

    pub fn preset3d(&mut self, preset: Preset3d) {
        let (centre, radius) = self.scene_centre_radius();
        self.cam3.target = centre;
        self.cam3.distance = (radius * 1.9).clamp(4.0, MAX_DISTANCE);
        self.cam3.yaw = match preset {
            Preset3d::Top => 0.0,
            Preset3d::Bird => 0.85,
            Preset3d::Oblique => 0.55,
        };
        self.cam3.pitch = match preset {
            Preset3d::Top => 1.42,
            Preset3d::Bird => 0.92,
            Preset3d::Oblique => 0.38,
        };
        self.view = ViewMode::Three;
        self.note = Some(
            match preset {
                Preset3d::Top => "3D 俯视",
                Preset3d::Bird => "3D 鸟瞰",
                Preset3d::Oblique => "3D 斜视",
            }
            .to_string(),
        );
    }

    /// Fly the orbit camera to a room's centre; also selects that room.
    pub fn fly_to_room(&mut self, id: EntityId) {
        let Some(room) = self.doc.rooms.iter().find(|r| r.id == id) else {
            return;
        };
        let count = room.polygon.len().max(1) as f32;
        let cx = room.polygon.iter().map(|p| p.x as f32).sum::<f32>() / count;
        let cy = room.polygon.iter().map(|p| p.y as f32).sum::<f32>() / count;
        let mut min = Pt::new(i32::MAX, i32::MAX);
        let mut max = Pt::new(i32::MIN, i32::MIN);
        for p in &room.polygon {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
        let diag = ((max.x - min.x) as f32).hypot((max.y - min.y) as f32) * 0.001;
        let name = room.name.clone();
        self.cam3.target = Vec3::new(cx * 0.001, 0.9, cy * 0.001);
        self.cam3.distance = (diag * 1.55).clamp(4.0, MAX_DISTANCE);
        self.cam3.pitch = 0.72;
        self.cam3.yaw = 0.85;
        self.selected_room = Some(id);
        self.view = ViewMode::Three;
        self.note = Some(format!("飞向 {name}"));
    }

    pub fn room_list(&self) -> Vec<(EntityId, String, f64)> {
        self.doc
            .rooms
            .iter()
            .filter(|r| r.polygon.len() >= 3)
            .map(|r| {
                (
                    r.id,
                    r.name.clone(),
                    fp_geom::area(&r.polygon) / 1_000_000.0,
                )
            })
            .collect()
    }

    pub fn layer_on(&self, layer: Layer) -> bool {
        let settings = &self.doc.settings;
        match layer {
            Layer::RoomNames => settings.show_room_names,
            Layer::Furniture => settings.show_furniture,
            Layer::Grid => settings.show_grid,
            Layer::Dimensions => settings.show_dimensions,
            Layer::Bearing => settings.show_bearing,
            Layer::Underlay => settings.show_underlay,
        }
    }

    pub fn toggle_layer(&mut self, layer: Layer) {
        {
            let settings = &mut self.doc.settings;
            match layer {
                Layer::RoomNames => settings.show_room_names = !settings.show_room_names,
                Layer::Furniture => settings.show_furniture = !settings.show_furniture,
                Layer::Grid => settings.show_grid = !settings.show_grid,
                Layer::Dimensions => settings.show_dimensions = !settings.show_dimensions,
                Layer::Bearing => settings.show_bearing = !settings.show_bearing,
                Layer::Underlay => settings.show_underlay = !settings.show_underlay,
            }
        }
        self.touch();
    }

    pub fn set_modifiers(&mut self, shift: bool) {
        self.shift_down = shift;
    }

    /// Snap a raw world point to the nearest wall end / room vertex within
    /// 260 mm, otherwise to the drafting grid.
    pub fn snap_measure(&self, raw: Pt) -> Pt {
        const SNAP_MM: f64 = 260.0;
        let mut best = raw;
        let mut best_d = SNAP_MM;
        let mut consider = |candidate: Pt| {
            let d = ((candidate.x - raw.x) as f64).hypot((candidate.y - raw.y) as f64);
            if d < best_d {
                best_d = d;
                best = candidate;
            }
        };
        for wall in &self.doc.walls {
            consider(wall.start);
            consider(wall.end);
        }
        for room in &self.doc.rooms {
            for point in &room.polygon {
                consider(*point);
            }
        }
        if best_d < SNAP_MM {
            best
        } else {
            Pt::new(
                snap_to_grid(raw.x, DEFAULT_GRID_MM),
                snap_to_grid(raw.y, DEFAULT_GRID_MM),
            )
        }
    }

    pub fn set_view(&mut self, view: ViewMode) {
        self.view = view;
        self.orbit_drag = None;
        self.orbit_start = None;
        self.walk_look = None;
        self.box_select = None;
        self.walk_keys = [false; 4];
        self.walk_stick = [false; 4];
        if view == ViewMode::Walk {
            self.walk = walk_camera(&self.doc);
        }
    }

    /// Hold/release a direction on the on-screen walk pad: 0 forward,
    /// 1 left, 2 back, 3 right.
    pub fn set_walk_stick(&mut self, direction: usize, held: bool) {
        if let Some(slot) = self.walk_stick.get_mut(direction) {
            *slot = held;
        }
    }

    pub fn clear_walk_stick(&mut self) {
        self.walk_stick = [false; 4];
    }

    /// Advance the first-person camera by `dt` seconds of held WASD input.
    pub fn walk_step(&mut self, dt: f32) {
        if self.view != ViewMode::Walk || dt <= 0.0 {
            return;
        }
        let (forward, right) = self.walk.basis();
        let mut f = 0.0f32;
        let mut r = 0.0f32;
        if self.walk_keys[0] || self.walk_stick[0] {
            f += 1.0;
        }
        if self.walk_keys[2] || self.walk_stick[2] {
            f -= 1.0;
        }
        if self.walk_keys[3] || self.walk_stick[3] {
            r += 1.0;
        }
        if self.walk_keys[1] || self.walk_stick[1] {
            r -= 1.0;
        }
        if f == 0.0 && r == 0.0 {
            return;
        }
        let speed = if self.walk_fast { 5.2 } else { 2.3 };
        let step = (forward * f + right * r).normalize_or_zero() * speed * dt.min(0.05);
        self.walk_try_step(step);
    }

    /// Walking with the wheel: one notch moves about 0.35 m forward, and
    /// wheel-up moves forward. Trackpad pixel deltas are normalised to the
    /// same scale and clamped so a single flick cannot cross a room.
    pub fn walk_scroll(&mut self, delta_y: f32) {
        if self.view != ViewMode::Walk {
            return;
        }
        let forward = self.walk.basis().0;
        let step = forward * (delta_y / 20.0).clamp(-2.0, 2.0) * 0.35;
        self.walk_try_step(step);
    }

    fn walk_try_step(&mut self, step: glam::Vec3) {
        if step.length_squared() < 1e-8 {
            return;
        }
        let p = self.walk.pos;
        let candidates = [
            p + step,
            glam::Vec3::new(p.x + step.x, p.y, p.z),
            glam::Vec3::new(p.x, p.y, p.z + step.z),
        ];
        for candidate in candidates {
            if self.stand_ok(candidate.x, candidate.z) {
                self.walk.pos = candidate;
                return;
            }
        }
    }

    /// A body needs to sit inside a room, clear of its wall lines and of furniture.
    fn stand_ok(&self, x: f32, z: f32) -> bool {
        let p = Pt::new((x * 1000.0).round() as i32, (z * 1000.0).round() as i32);
        let in_room = self.doc.rooms.iter().any(|room| {
            point_in_polygon(p, &room.polygon) && polygon_margin(p, &room.polygon) >= 120.0
        });
        in_room && !self.furniture_blocks(x, z)
    }

    /// True when the point is inside a furniture footprint inflated by a body radius.
    fn furniture_blocks(&self, x: f32, z: f32) -> bool {
        const BODY: f32 = 0.24;
        self.doc.furniture.iter().any(|f| {
            let (sin, cos) = (f.rotation_deg as f32).to_radians().sin_cos();
            let dx = x - f.center.x as f32 / 1000.0;
            let dz = z - f.center.y as f32 / 1000.0;
            let local_x = dx * cos + dz * sin;
            let local_z = -dx * sin + dz * cos;
            local_x.abs() < f.width_mm as f32 / 2000.0 + BODY
                && local_z.abs() < f.depth_mm as f32 / 2000.0 + BODY
        })
    }

    /// Cached extruded geometry. Rebuilt only when the document, the cut-away
    /// setting or the door state changes, not on every frame.
    pub fn mesh3d_parts(&mut self, walk: bool) -> (u64, std::sync::Arc<fp_render3d::MeshParts>) {
        if let Some((rev, cached_walk, cached_cut, parts)) = &self.mesh_cache
            && *rev == self.mesh_revision
            && *cached_walk == walk
            && *cached_cut == self.cutaway
        {
            return (self.mesh_epoch(walk), parts.clone());
        }
        let parts = if walk {
            self.mesh3d_walk_parts()
        } else {
            self.mesh3d_with(self.cutaway)
        };
        let parts = std::sync::Arc::new(parts);
        self.mesh_cache = Some((self.mesh_revision, walk, self.cutaway, parts.clone()));
        (self.mesh_epoch(walk), parts)
    }

    /// Epoch handed to the 3D renderer. It has to change whenever the mesh
    /// changes, otherwise the upload is skipped and the old walls stay on
    /// screen until something else forces a redraw.
    fn mesh_epoch(&self, walk: bool) -> u64 {
        self.mesh_revision * 16
            + u64::from(walk) * 8
            + u64::from(self.cutaway) * 4
            + u64::from(self.night) * 2
    }

    /// Extruded geometry plus a ceiling, used by the first-person view.
    fn mesh3d_walk_parts(&self) -> fp_render3d::MeshParts {
        let parts = self.mesh3d_with(false);
        let mut mesh = parts.vertices;
        let opaque = parts.opaque;
        let mut ceilings = Vec::new();
        let top_level = self.doc.level_count().saturating_sub(1);
        for level in 0..self.doc.level_count() {
            if self.show_roof && level == top_level {
                continue;
            }
            let floor = level as u32;
            let elevation = self.doc.level_elevation_mm(level) as f32 / 1000.0;
            let rooms: Vec<Room> = self
                .doc
                .rooms
                .iter()
                .filter(|r| r.floor == floor)
                .cloned()
                .collect();
            let mut part = ceiling_mesh_with_materials(&rooms, CEILING_HEIGHT, &self.doc.materials);
            for vertex in &mut part {
                vertex.pos[1] += elevation;
            }
            ceilings.extend(part);
        }
        let ceiling_len = ceilings.len();
        if opaque < mesh.len() {
            let glass = mesh.split_off(opaque);
            mesh.extend(ceilings);
            mesh.extend(glass);
            fp_render3d::MeshParts {
                vertices: mesh,
                opaque: opaque + ceiling_len,
            }
        } else {
            mesh.extend(ceilings);
            fp_render3d::MeshParts {
                vertices: mesh,
                opaque: opaque + ceiling_len,
            }
        }
    }

    /// Extruded geometry plus a ceiling, used by the first-person view.
    #[allow(dead_code)] // exercised by tests and the off-screen exporter
    pub fn mesh3d_walk(&self) -> Vec<MeshVertex> {
        self.mesh3d_walk_parts().vertices
    }

    /// Extruded geometry, with the selected wall / room / furniture tinted.
    #[allow(dead_code)] // exercised by tests and the off-screen exporter
    pub fn mesh3d(&self) -> Vec<MeshVertex> {
        self.mesh3d_with(self.cutaway).vertices
    }

    fn mesh3d_with(&self, cutaway: bool) -> fp_render3d::MeshParts {
        const CUT_MM: i32 = 1350;
        let levels = self.doc.level_count();
        let mut scene = fp_render3d::MeshParts::new();
        scene.push_opaque(fp_render3d::ground_mesh());
        for level in 0..levels {
            let floor = level as u32;
            let elevation = self.doc.level_elevation_mm(level) as f32 / 1000.0;
            let walls: Vec<Wall> = self
                .doc
                .walls
                .iter()
                .filter(|w| w.floor == floor)
                .map(|w| {
                    let mut copy = w.clone();
                    if cutaway {
                        copy.height_mm = copy.height_mm.min(CUT_MM);
                    }
                    copy
                })
                .collect();
            let openings: Vec<Opening> = self
                .doc
                .openings
                .iter()
                .filter(|o| walls.iter().any(|w| w.id == o.wall_id))
                .map(|o| {
                    let mut copy = o.clone();
                    if cutaway {
                        copy.sill_mm = copy.sill_mm.min(CUT_MM);
                        copy.height_mm = copy.height_mm.min((CUT_MM - copy.sill_mm).max(0));
                    }
                    copy
                })
                .collect();

            let mut floor_parts = fp_render3d::MeshParts::new();
            for wall in &walls {
                let mut part = wall_mesh_with_finishes(
                    std::slice::from_ref(wall),
                    &openings,
                    &self.doc.materials,
                );
                if self.selection.contains(&wall.id) {
                    paint(&mut part, HIGHLIGHT_3D);
                }
                floor_parts.push_opaque(part);
            }
            for room in self.doc.rooms.iter().filter(|r| r.floor == floor) {
                let mut part = floor_mesh(std::slice::from_ref(room), &self.doc.materials);
                if self.selected_room == Some(room.id) {
                    paint(&mut part, HIGHLIGHT_3D);
                }
                floor_parts.push_opaque(part);
            }
            if !cutaway {
                for opening in &openings {
                    if opening.kind != OpeningKind::Door {
                        continue;
                    }
                    let Some(wall) = walls.iter().find(|w| w.id == opening.wall_id) else {
                        continue;
                    };
                    let swing = if self.doors_open.contains(&opening.id.0) {
                        if opening.swing_deg < 0.0 { -78.0 } else { 78.0 }
                    } else {
                        0.0
                    };
                    floor_parts.push_opaque(fp_render3d::door_leaf_mesh(wall, opening, swing));
                }
            }
            if self.doc.settings.show_furniture {
                for item in self.doc.furniture.iter().filter(|f| f.floor == floor) {
                    let mut parts = fp_render3d::furniture_mesh_parts(std::slice::from_ref(item));
                    if self.furniture_selection.contains(&item.id) {
                        paint(&mut parts.vertices, HIGHLIGHT_3D);
                    }
                    floor_parts.push_mixed(parts);
                }
            }
            if !cutaway {
                floor_parts.push_glass(fp_render3d::opening_glass_mesh(&walls, &openings));
            }
            for stair in self.doc.stairs.iter().filter(|s| s.floor == floor) {
                floor_parts.push_opaque(fp_render3d::stair_mesh(
                    stair,
                    self.doc.level_height_mm(level) as f32 / 1000.0,
                ));
            }
            for vertex in &mut floor_parts.vertices {
                vertex.pos[1] += elevation;
            }
            scene.push_mixed(floor_parts);
        }
        if self.show_roof && levels > 0 {
            let top = levels - 1;
            let roof_height =
                (self.doc.level_elevation_mm(top) + self.doc.level_height_mm(top)) as f32 / 1000.0;
            let rooms: Vec<Room> = self
                .doc
                .rooms
                .iter()
                .filter(|r| r.floor == top as u32)
                .cloned()
                .collect();
            let roof = self.doc.roof.normalized();
            scene.push_opaque(fp_render3d::roof_mesh_for(
                &rooms,
                roof_height,
                roof.kind,
                roof.pitch_deg,
                roof.overhang_mm,
            ));
        }
        scene
    }

    /// Ray-pick from a screen point in the 3D view (furniture → wall → room).
    /// Camera ray for a screen point in the 3D view.
    fn ray_3d(&self, screen: [f32; 2]) -> Option<(Vec3, Vec3)> {
        let size = self.view_size;
        if size[0] <= 1.0 || size[1] <= 1.0 {
            return None;
        }
        let ndc_x = screen[0] / size[0] * 2.0 - 1.0;
        let ndc_y = 1.0 - screen[1] / size[1] * 2.0;
        let inv = self.cam3.view_proj(size[0] / size[1]).inverse();
        let near = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let far = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        let near = near.truncate() / near.w;
        let far = far.truncate() / far.w;
        Some((near, (far - near).normalize()))
    }

    /// Press on a furniture box in 3D: select it and start a ground-plane drag.
    fn begin_furniture_drag_3d(&mut self, pos: [f32; 2]) -> bool {
        let Some((origin, dir)) = self.ray_3d(pos) else {
            return false;
        };
        let Some(Pick3d::Furniture(id)) = pick_scene_at(
            &self.doc,
            origin,
            dir,
            self.floor(),
            self.doc.level_elevation_mm(self.floor_index()) as f32 / 1000.0,
        ) else {
            return false;
        };
        let Some(item) = self.doc.furniture.iter().find(|f| f.id == id) else {
            return false;
        };
        let centre = [item.center.x as f32 * 0.001, item.center.y as f32 * 0.001];
        let Some(hit) = self.ground_point(pos) else {
            return false;
        };
        let before = item.center;
        let name = item.name.clone();
        self.selected_furniture = Some(id);
        self.selection.clear();
        self.furniture_drag_3d = Some(Drag3d {
            id,
            before,
            grab: [hit[0] - centre[0], hit[1] - centre[1]],
        });
        self.note = Some(format!("拖动 {name}"));
        self.touch();
        true
    }

    /// Where a 3D ray crosses the floor plane, in metres (x, z).
    fn ground_point(&self, screen: [f32; 2]) -> Option<[f32; 2]> {
        let (origin, dir) = self.ray_3d(screen)?;
        if dir.y.abs() < 1e-4 {
            return None;
        }
        let elevation = self.doc.level_elevation_mm(self.floor_index()) as f32 / 1000.0;
        let t = (elevation + 0.02 - origin.y) / dir.y;
        if t <= 0.0 {
            return None;
        }
        let hit = origin + dir * t;
        Some([hit.x, hit.z])
    }

    pub fn pick_3d(&mut self, screen: [f32; 2]) {
        let Some((near, dir)) = self.ray_3d(screen) else {
            return;
        };
        let Some(hit) = pick_scene_at(
            &self.doc,
            near,
            dir,
            self.floor(),
            self.doc.level_elevation_mm(self.floor_index()) as f32 / 1000.0,
        ) else {
            self.note = Some("没点到东西".to_string());
            return;
        };
        self.selected_opening = None;
        match hit {
            Pick3d::Wall(id) => {
                self.selection = vec![id];
                self.selected_furniture = None;
                self.furniture_selection.clear();
                self.selected_room = None;
                let len = self
                    .doc
                    .walls
                    .iter()
                    .find(|w| w.id == id)
                    .map(|w| {
                        ((w.end.x - w.start.x) as f64).hypot((w.end.y - w.start.y) as f64) / 1000.0
                    })
                    .unwrap_or(0.0);
                self.note = Some(format!("已选中墙 · {len:.2} m"));
            }
            Pick3d::Furniture(id) => {
                self.selection.clear();
                self.selected_room = None;
                self.selected_furniture = Some(id);
                self.furniture_selection = vec![id];
                let name = self
                    .doc
                    .furniture
                    .iter()
                    .find(|f| f.id == id)
                    .map(|f| f.name.clone())
                    .unwrap_or_default();
                self.note = Some(format!("已选中家具 · {name}"));
            }
            Pick3d::Room(id) => {
                self.selection.clear();
                self.selected_furniture = None;
                self.furniture_selection.clear();
                self.selected_room = Some(id);
                let room = self.doc.rooms.iter().find(|r| r.id == id);
                if let Some(room) = room {
                    let area = fp_geom::area(&room.polygon) / 1_000_000.0;
                    self.note = Some(format!("已选中房间 · {} {area:.1}㎡", room.name));
                }
            }
        }
        self.touch();
    }

    pub fn set_view_size(&mut self, size: [f32; 2]) {
        if (self.view_size[0] - size[0]).abs() > 0.5 || (self.view_size[1] - size[1]).abs() > 0.5 {
            self.view_size = size;
            self.cam.resize(size);
            self.touch();
        }
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.furniture_armed = tool == Tool::Furniture;
        self.draft = None;
        if tool == Tool::Demolish {
            self.note = Some("拆墙：点击非承重墙，承重墙会拒绝".into());
        }
        self.touch();
    }

    // ------------------------------------------------------------ persistence

    pub fn open_path(&mut self, path: PathBuf) {
        let result = fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| Document::from_json(&text).map_err(|e| e.to_string()));
        match result {
            Ok(doc) => {
                let name = doc.meta.name.clone();
                self.adopt_document(doc, path.clone());
                self.note = Some(format!("已打开 · {}", display_path(&path)));
                crate::recent::remember(&path, &name);
            }
            Err(e) => self.note = Some(format!("打开失败 · {e}")),
        }
    }

    /// Replace the whole document from a template. The caller supplies a fresh
    /// path so a new project never overwrites the plan that was open.
    pub fn new_document(&mut self, doc: Document, path: PathBuf) {
        let name = doc.meta.name.clone();
        self.adopt_document(doc, path.clone());
        self.view = ViewMode::Plan;
        self.tool = Tool::Select;
        let result = self.write_doc();
        if result.is_ok() {
            self.saved_revision = self.revision;
            self.dirty = false;
        } else {
            self.dirty = true;
            self.revision += 1;
            self.saved_revision = self.revision.saturating_sub(1);
        }
        self.note = Some(match result {
            Ok(()) => format!("已新建 · {} · {}", name, display_path(&path)),
            Err(e) => format!("新建失败 · {e}"),
        });
        crate::recent::remember(&path, &name);
    }

    fn adopt_document(&mut self, mut doc: Document, path: PathBuf) {
        // Old plans (and blank sheets) may predate the standard flooring
        // palette; repair them on open so material cycling and the quote work.
        crate::templates::ensure_materials(&mut doc);
        let (min, max) = plan_bounds(&doc);
        let underlay_image = doc.underlay.as_ref().and_then(|u| {
            UnderlayImage::load(std::path::Path::new(&u.path))
                .ok()
                .map(Arc::new)
        });
        self.doc = doc;
        self.underlay_image = underlay_image;
        self.history = History::new(200);
        self.active_level = 0;
        self.selection.clear();
        self.selected_room = None;
        self.selected_furniture = None;
        self.furniture_selection.clear();
        self.selected_opening = None;
        self.selected_stair = None;
        self.furniture_drag = None;
        self.opening_drag = None;
        self.furniture_drag_3d = None;
        self.rotate_drag = None;
        self.measure_from = None;
        let floor = self.floor();
        self.measurements = self
            .doc
            .measures
            .iter()
            .filter(|m| m.floor == floor)
            .map(|m| Measurement { from: m.a, to: m.b })
            .collect();
        self.doors_open.clear();
        self.clipboard = None;
        self.paste_count = 0;
        self.draft = None;
        self.box_select = None;
        self.drag = None;
        self.pan = None;
        self.hover = None;
        self.walk_keys = [false; 4];
        self.walk_stick = [false; 4];
        self.walk_fast = false;
        self.walk_look = None;
        self.cam.fit(min, max, 80.0);
        self.walk = walk_camera(&self.doc);
        self.cam3 = OrbitCamera::new(Vec3::ZERO);
        self.cam3.fit(
            Vec3::new(min.x as f32 / 1000.0, 0.0, min.y as f32 / 1000.0),
            Vec3::new(max.x as f32 / 1000.0, 3.0, max.y as f32 / 1000.0),
        );
        self.saved_path = path;
        self.revision += 1;
        self.saved_revision = self.revision;
        self.dirty = false;
    }

    pub fn save_as(&mut self, path: PathBuf) {
        self.saved_path = path;
        self.save();
    }

    pub fn save_now(&mut self) {
        self.save();
    }

    fn write_doc(&self) -> Result<(), String> {
        let json = self.doc.to_json().map_err(|e| e.to_string())?;
        if let Some(parent) = self.saved_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&self.saved_path, json).map_err(|e| e.to_string())
    }

    fn save(&mut self) {
        let result = self.write_doc();
        if result.is_ok() {
            self.saved_revision = self.revision;
            self.dirty = false;
            crate::recent::remember(&self.saved_path, &self.doc.meta.name);
        }
        self.note = Some(match result {
            Ok(()) => format!("已保存 · {}", display_path(&self.saved_path)),
            Err(e) => format!("保存失败 · {e}"),
        });
    }

    /// Silent background save; returns true when the document was written.
    pub fn autosave_if_dirty(&mut self) -> bool {
        if self.revision == self.saved_revision {
            return false;
        }
        if self.write_doc().is_err() {
            return false;
        }
        self.saved_revision = self.revision;
        true
    }

    // ---------------------------------------------------------------- status

    /// Stable identity of the current selection. The desktop shell uses it to
    /// tell "the user selected something new" from "the same panel was closed",
    /// so a closed inspector reopens on the next selection instead of staying
    /// silently hidden.
    pub fn selection_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for id in &self.selection {
            id.0.hash(&mut hasher);
        }
        self.selected_opening.map(|id| id.0).hash(&mut hasher);
        self.selected_room.map(|id| id.0).hash(&mut hasher);
        self.selected_furniture.map(|id| id.0).hash(&mut hasher);
        self.selected_stair.map(|id| id.0).hash(&mut hasher);
        for id in &self.furniture_selection {
            id.0.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Description of the current selection, rendered as the side panel.
    #[allow(clippy::collapsible_if)]
    pub fn inspector(&self) -> Option<InspectorView> {
        let t = crate::i18n::T(self.lang);
        if let Some(stair) = self.selected_stair_info() {
            let floor_height = self.doc.level_height_mm(stair.floor as usize) as f64;
            let run =
                ((stair.end.x - stair.start.x) as f64).hypot((stair.end.y - stair.start.y) as f64);
            let rise = if stair.steps > 0 {
                floor_height / stair.steps as f64
            } else {
                0.0
            };
            let tread = if stair.steps > 1 {
                run / stair.steps as f64
            } else {
                0.0
            };
            return Some(InspectorView {
                title: t.s("楼梯", "Stair").into(),
                subtitle: format!(
                    "{} · {:.2} m",
                    self.doc.level_name(stair.floor as usize),
                    run / 1000.0
                ),
                kind: InspectorKind::Stair,
                fields: vec![
                    InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("宽度", "Width").into(),
                        value: stair.width_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::StairWidth),
                    },
                    InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("踏步数", "Steps").into(),
                        value: stair.steps.to_string(),
                        unit: t.s("级", "steps").into(),
                        edit: Some(InspectorEdit::StairSteps),
                    },
                ],
                rotation: None,
                swatches: Vec::new(),
                lines: vec![
                    t.f(
                        &format!("总长 {:.2} m", run / 1000.0),
                        &format!("Run {:.2} m", run / 1000.0),
                    ),
                    t.f(
                        &format!("踏步高 {:.0} mm", rise),
                        &format!("Rise {:.0} mm", rise),
                    ),
                    t.f(
                        &format!("踏面 {:.0} mm", tread),
                        &format!("Tread {:.0} mm", tread),
                    ),
                    t.f(
                        &format!("{} 级 · 宽 {} mm", stair.steps, stair.width_mm),
                        &format!("{} steps · {} mm", stair.steps, stair.width_mm),
                    ),
                ],
                actions: vec![
                    (t.s("反向", "Reverse"), InspectorAction::StairReverse),
                    (t.s("删除", "Delete"), InspectorAction::RemoveStair),
                ],
                material_options: Vec::new(),
                wall_finish_options: Vec::new(),
                ceiling_options: Vec::new(),
                wall_kind: None,
                wall_finish: None,
            });
        }
        if let Some(id) = self.selected_opening {
            if let Some(o) = self.doc.openings.iter().find(|o| o.id == id) {
                let door = o.kind == OpeningKind::Door;
                let mut lines = vec![t.f(
                    &format!("墙 #{}", o.wall_id.0),
                    &format!("Wall #{}", o.wall_id.0),
                )];
                let mut actions = Vec::new();
                if door {
                    lines.push(t.f(
                        &format!("开向 {}", if o.swing_deg >= 0.0 { "A" } else { "B" }),
                        &format!("Swing {}", if o.swing_deg >= 0.0 { "A" } else { "B" }),
                    ));
                    actions.push((
                        t.s("开向反向", "Flip swing"),
                        InspectorAction::OpeningFlipSwing,
                    ));
                    lines.push(t.f(
                        &format!("铰链 {}", if o.hinge_right { "右侧" } else { "左侧" }),
                        &format!("Hinge {}", if o.hinge_right { "right" } else { "left" }),
                    ));
                    actions.push((
                        t.s("铰链换侧", "Flip hinge"),
                        InspectorAction::OpeningFlipHinge,
                    ));
                }
                actions.push((t.s("删除", "Delete"), InspectorAction::RemoveOpening));
                let mut fields = Vec::new();
                if door {
                    fields.push(InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("宽", "Width").into(),
                        value: o.width_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::OpeningWidth),
                    });
                    fields.push(InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("高", "Height").into(),
                        value: o.height_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::OpeningHeight),
                    });
                } else {
                    fields.push(InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("宽", "Width").into(),
                        value: o.width_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::OpeningWidth),
                    });
                    fields.push(InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("高", "Height").into(),
                        value: o.height_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::OpeningHeight),
                    });
                    fields.push(InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("台高", "Sill").into(),
                        value: o.sill_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::OpeningSill),
                    });
                }
                fields.push(InspectorField {
                    group: t.s("造价", "Cost").into(),
                    label: t.s("单价", "Price").into(),
                    value: format!("{:.0}", o.effective_price()),
                    unit: "¥".into(),
                    edit: Some(InspectorEdit::OpeningPrice),
                });
                return Some(InspectorView {
                    title: if door {
                        t.s("门", "Door")
                    } else {
                        t.s("窗", "Window")
                    }
                    .into(),
                    subtitle: t.f(
                        "门窗 · 点击数值可直接修改",
                        "Opening · click values to edit",
                    ),
                    kind: if door {
                        InspectorKind::OpeningDoor
                    } else {
                        InspectorKind::OpeningWindow
                    },
                    fields,
                    rotation: None,
                    swatches: Vec::new(),
                    lines,
                    actions,
                    material_options: Vec::new(),
                    wall_finish_options: Vec::new(),
                    ceiling_options: Vec::new(),
                    wall_kind: None,
                    wall_finish: None,
                });
            }
        }
        if self.furniture_selection.len() > 1 {
            let count = self.furniture_selection.len();
            return Some(InspectorView {
                title: t.f(&format!("{count} 件家具"), &format!("{count} furniture")),
                subtitle: t
                    .f(
                        "批量选择 · Shift 点击增减",
                        "Multi-select · Shift-click to add",
                    )
                    .into(),
                kind: InspectorKind::Multi,
                fields: Vec::new(),
                rotation: None,
                swatches: Vec::new(),
                lines: vec![t.f(
                    "Shift 点击继续增减 · 组拖动已启用",
                    "Shift-click to add/remove · group drag on",
                )],
                material_options: Vec::new(),
                wall_finish_options: Vec::new(),
                ceiling_options: Vec::new(),
                wall_kind: None,
                wall_finish: None,
                actions: vec![
                    (
                        t.s("左对齐", "Align left"),
                        InspectorAction::FurnitureAlignLeft,
                    ),
                    (
                        t.s("水平居中", "Centre X"),
                        InspectorAction::FurnitureAlignCenterX,
                    ),
                    (
                        t.s("右对齐", "Align right"),
                        InspectorAction::FurnitureAlignRight,
                    ),
                    (
                        t.s("上对齐", "Align top"),
                        InspectorAction::FurnitureAlignTop,
                    ),
                    (
                        t.s("垂直居中", "Centre Y"),
                        InspectorAction::FurnitureAlignCenterY,
                    ),
                    (
                        t.s("下对齐", "Align bottom"),
                        InspectorAction::FurnitureAlignBottom,
                    ),
                    (t.s("删除", "Delete"), InspectorAction::RemoveFurniture),
                ],
            });
        }
        if let Some(id) = self.selected_furniture {
            if let Some(f) = self.doc.furniture.iter().find(|f| f.id == id) {
                let layer = self
                    .doc
                    .furniture
                    .iter()
                    .position(|item| item.id == f.id)
                    .map(|index| (index + 1, self.doc.furniture.len()))
                    .unwrap_or((0, 0));
                let fields = vec![
                    InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("宽", "Width").into(),
                        value: f.width_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::FurnitureWidth),
                    },
                    InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("深", "Depth").into(),
                        value: f.depth_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::FurnitureDepth),
                    },
                    InspectorField {
                        group: t.s("位置", "Position").into(),
                        label: t.s("左右位置", "Left–right").into(),
                        value: f.center.x.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::FurnitureX),
                    },
                    InspectorField {
                        group: t.s("位置", "Position").into(),
                        label: t.s("上下位置", "Up–down").into(),
                        value: f.center.y.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::FurnitureY),
                    },
                    InspectorField {
                        group: t.s("造价", "Cost").into(),
                        label: t.s("单价", "Price").into(),
                        value: format!("{:.0}", f.effective_price()),
                        unit: "¥".into(),
                        edit: Some(InspectorEdit::FurniturePrice),
                    },
                ];
                let swatches = FABRIC_SWATCHES
                    .iter()
                    .map(|(name, hex)| {
                        ((*name).to_string(), *hex, f.color.eq_ignore_ascii_case(hex))
                    })
                    .collect();
                return Some(InspectorView {
                    title: f.name.clone(),
                    subtitle: t.f(
                        &format!("家具 · 点击数值可精确编辑 · 层序 {}/{}", layer.0, layer.1),
                        &format!(
                            "Furniture · click values to edit · layer {}/{}",
                            layer.0, layer.1
                        ),
                    ),
                    kind: InspectorKind::Furniture,
                    fields,
                    rotation: Some(f.rotation_deg),
                    swatches,
                    lines: Vec::new(),
                    material_options: Vec::new(),
                    wall_finish_options: Vec::new(),
                    ceiling_options: Vec::new(),
                    wall_kind: None,
                    wall_finish: None,
                    actions: vec![
                        (
                            t.s("置顶", "To front"),
                            InspectorAction::FurnitureBringFront,
                        ),
                        (t.s("置底", "To back"), InspectorAction::FurnitureSendBack),
                        (t.s("删除", "Delete"), InspectorAction::RemoveFurniture),
                    ],
                });
            }
        }
        if let Some(id) = self.selected_room {
            if let Some(r) = self.doc.rooms.iter().find(|r| r.id == id) {
                let material = r
                    .material_id
                    .and_then(|m| self.doc.materials.iter().find(|x| x.id == m))
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| t.f("未指定", "unassigned"));
                let price = r
                    .material_id
                    .and_then(|m| self.doc.materials.iter().find(|x| x.id == m))
                    .map(|m| m.price_per_m2)
                    .unwrap_or(0.0);
                let (net, billed, _, cost) = self.room_quote(id).unwrap_or((0.0, 0.0, 0.0, 0.0));
                let furn_count = self
                    .doc
                    .furniture
                    .iter()
                    .filter(|f| f.room_id == Some(id))
                    .count();
                let furn_cost: f64 = self
                    .doc
                    .furniture
                    .iter()
                    .filter(|f| f.room_id == Some(id))
                    .map(|f| f.effective_price())
                    .sum();
                let fields = vec![
                    InspectorField {
                        group: t.s("规格", "Spec").into(),
                        label: t.s("面积", "Area").into(),
                        value: format!("{net:.1}"),
                        unit: "m²".into(),
                        edit: None,
                    },
                    InspectorField {
                        group: t.s("规格", "Spec").into(),
                        label: t.s("材质", "Material").into(),
                        value: material.clone(),
                        unit: String::new(),
                        edit: None,
                    },
                    InspectorField {
                        group: t.s("造价", "Cost").into(),
                        label: t.s("单价", "Rate").into(),
                        value: format!("¥{price:.0}"),
                        unit: "/m²".into(),
                        edit: None,
                    },
                    InspectorField {
                        group: t.s("造价", "Cost").into(),
                        label: t.s("含损耗", "Billed").into(),
                        value: format!("¥{cost:.0}"),
                        unit: format!("{billed:.1} m²"),
                        edit: None,
                    },
                    InspectorField {
                        group: t.s("造价", "Cost").into(),
                        label: t.s("家具", "Furniture").into(),
                        value: format!("¥{furn_cost:.0}"),
                        unit: format!("{furn_count} 件"),
                        edit: None,
                    },
                    InspectorField {
                        group: t.s("造价", "Cost").into(),
                        label: t.s("房间合计", "Room total").into(),
                        value: format!("¥{:.0}", cost + furn_cost),
                        unit: String::new(),
                        edit: None,
                    },
                ];
                return Some(InspectorView {
                    title: r.name.clone(),
                    subtitle: t
                        .f("房间 · 地面/顶面饰面与造价", "Room · floor & ceiling finishes")
                        .into(),
                    kind: InspectorKind::Room,
                    fields,
                    rotation: None,
                    swatches: Vec::new(),
                    lines: Vec::new(),
                    material_options: self
                        .doc
                        .materials
                        .iter()
                        .filter(|m| m.surface == FinishSurface::Floor)
                        .map(|m| {
                            (
                                m.name.clone(),
                                m.color.clone(),
                                m.id,
                                r.material_id == Some(m.id),
                            )
                        })
                        .collect(),
                    wall_finish_options: Vec::new(),
                    ceiling_options: self
                        .doc
                        .materials
                        .iter()
                        .filter(|m| m.surface == FinishSurface::Ceiling)
                        .map(|m| {
                            (
                                m.name.clone(),
                                m.color.clone(),
                                m.id,
                                r.ceiling_material_id == Some(m.id),
                            )
                        })
                        .collect(),
                    wall_kind: None,
                    wall_finish: None,
                    actions: vec![(t.s("重命名", "Rename"), InspectorAction::RenameRoom)],
                });
            }
        }
        if self.selection.len() == 1 {
            if let Some(w) = self.doc.walls.iter().find(|w| w.id == self.selection[0]) {
                let len = ((w.end.x - w.start.x) as f64).hypot((w.end.y - w.start.y) as f64);
                let fields = vec![
                    InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("长度", "Length").into(),
                        value: format!("{len:.0}"),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::WallLength),
                    },
                    InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("厚度", "Thickness").into(),
                        value: w.thickness_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::WallThickness),
                    },
                    InspectorField {
                        group: t.s("尺寸", "Size").into(),
                        label: t.s("高度", "Height").into(),
                        value: w.height_mm.to_string(),
                        unit: "mm".into(),
                        edit: Some(InspectorEdit::WallHeight),
                    },
                ];
                return Some(InspectorView {
                    title: wall_kind_name(w.kind).to_string(),
                    subtitle: t.f(
                        &format!("墙体 · 长度 {len:.0} mm · 饰面来自饰面库"),
                        &format!("Wall · {len:.0} mm · finish comes from the library"),
                    ),
                    kind: InspectorKind::Wall,
                    fields,
                    rotation: None,
                    swatches: Vec::new(),
                    lines: Vec::new(),
                    material_options: Vec::new(),
                    wall_finish_options: self
                        .doc
                        .materials
                        .iter()
                        .filter(|m| m.surface == FinishSurface::Wall)
                        .map(|m| {
                            (
                                m.name.clone(),
                                m.color.clone(),
                                m.id,
                                w.finish_id == Some(m.id),
                            )
                        })
                        .collect(),
                    ceiling_options: Vec::new(),
                    wall_kind: Some(w.kind),
                    wall_finish: Some(w.finish),
                    actions: vec![
                        (
                            t.s("应用到本层", "Apply to floor"),
                            InspectorAction::ApplyWallFinishToFloor,
                        ),
                        (t.s("删除", "Delete"), InspectorAction::RemoveWall),
                    ],
                });
            }
        } else if self.selection.len() > 1 {
            return Some(InspectorView {
                title: t.f(
                    &format!("{} 面墙", self.selection.len()),
                    &format!("{} walls", self.selection.len()),
                ),
                subtitle: t.f("批量选择", "Multi-select").into(),
                kind: InspectorKind::Multi,
                fields: Vec::new(),
                rotation: None,
                swatches: Vec::new(),
                lines: vec![t.f("批量操作暂只支持删除", "bulk edit: delete only")],
                material_options: Vec::new(),
                wall_finish_options: self
                    .doc
                    .materials
                    .iter()
                    .filter(|m| m.surface == FinishSurface::Wall)
                    .map(|m| (m.name.clone(), m.color.clone(), m.id, false))
                    .collect(),
                ceiling_options: Vec::new(),
                wall_kind: None,
                wall_finish: None,
                actions: vec![
                    (t.s("批量删除", "Delete all"), InspectorAction::RemoveWall),
                ],
            });
        }
        None
    }

    pub fn apply_action(&mut self, action: InspectorAction) {
        match action {
            InspectorAction::CycleWallKind => self.cycle_wall_kind(),
            InspectorAction::SetWallFinish(finish) => self.set_wall_finish(finish),
            InspectorAction::SetWallFinishMaterial(id) => self.set_wall_finish_material(id),
            InspectorAction::ApplyWallFinishToSelection(id) => {
                self.apply_wall_finish_to_selection(id)
            }
            InspectorAction::ApplyWallFinishToFloor => self.apply_wall_finish_to_floor(),
            InspectorAction::SetCeilingMaterial(id) => self.set_room_ceiling_material(id),
            InspectorAction::WallThickness(d) => self.adjust_wall(d, 0),
            InspectorAction::WallHeight(d) => self.adjust_wall(0, d),
            InspectorAction::RemoveWall => self.delete_selection(),
            InspectorAction::OpeningWidth(d) => self.adjust_opening(d, 0, 0),
            InspectorAction::OpeningHeight(d) => self.adjust_opening(0, d, 0),
            InspectorAction::OpeningSill(d) => self.adjust_opening(0, 0, d),
            InspectorAction::RemoveOpening => self.remove_opening(),
            InspectorAction::CycleMaterial => self.cycle_material(),
            InspectorAction::SetMaterial(id) => self.set_room_material(id),
            InspectorAction::RenameRoom => {}
            InspectorAction::FurnitureRotate => self.rotate_furniture(),
            InspectorAction::FurnitureRotateFine(d) => self.rotate_furniture_by(d as f64),
            InspectorAction::FurnitureWidth(d) => self.resize_furniture(d, 0),
            InspectorAction::FurnitureDepth(d) => self.resize_furniture(0, d),
            InspectorAction::FurnitureColor => self.recolor_furniture(),
            InspectorAction::FurnitureSetColor(hex) => self.set_furniture_color(hex),
            InspectorAction::FurnitureSetWidth(v) => self.set_furniture_size(Some(v), None),
            InspectorAction::FurnitureSetDepth(v) => self.set_furniture_size(None, Some(v)),
            InspectorAction::FurnitureSetX(v) => self.set_furniture_position(Some(v), None),
            InspectorAction::FurnitureSetY(v) => self.set_furniture_position(None, Some(v)),
            InspectorAction::FurnitureSetPrice(v) => self.set_furniture_price(v),
            InspectorAction::WallSetThickness(v) => self.set_wall_size(Some(v), None),
            InspectorAction::WallSetHeight(v) => self.set_wall_size(None, Some(v)),
            InspectorAction::WallSetLength(v) => self.set_wall_length(v),
            InspectorAction::SetWallKind(kind) => self.set_wall_kind(kind),
            InspectorAction::OpeningSetWidth(v) => self.set_opening_size(Some(v), None, None),
            InspectorAction::OpeningSetHeight(v) => self.set_opening_size(None, Some(v), None),
            InspectorAction::OpeningSetSill(v) => self.set_opening_size(None, None, Some(v)),
            InspectorAction::OpeningSetPrice(v) => self.set_opening_price(v),
            InspectorAction::FurnitureBringFront => self.move_furniture_layer(true),
            InspectorAction::FurnitureSendBack => self.move_furniture_layer(false),
            InspectorAction::FurnitureAlignLeft => self.align_furniture(0, 0),
            InspectorAction::FurnitureAlignCenterX => self.align_furniture(0, 1),
            InspectorAction::FurnitureAlignRight => self.align_furniture(0, 2),
            InspectorAction::FurnitureAlignTop => self.align_furniture(1, 0),
            InspectorAction::FurnitureAlignCenterY => self.align_furniture(1, 1),
            InspectorAction::FurnitureAlignBottom => self.align_furniture(1, 2),
            InspectorAction::RemoveFurniture => self.delete_selection(),
            InspectorAction::OpeningFlipSwing => self.flip_opening_swing(),
            InspectorAction::OpeningFlipHinge => self.flip_opening_hinge(),
            InspectorAction::StairSetWidth(v) => {
                let _ = self.set_selected_stair_size(Some(v), None);
            }
            InspectorAction::StairSetSteps(v) => {
                let _ = self.set_selected_stair_size(None, Some(v));
            }
            InspectorAction::StairReverse => {
                self.reverse_selected_stair();
            }
            InspectorAction::RemoveStair => self.delete_selection(),
        }
        self.touch();
    }

    /// Current room id and name, for the rename dialog.
    pub fn selected_room_prompt(&self) -> Option<(EntityId, String)> {
        let id = self.selected_room?;
        let room = self.doc.rooms.iter().find(|r| r.id == id)?;
        Some((id, room.name.clone()))
    }

    pub fn rename_room(&mut self, id: EntityId, name: &str) {
        let Some(room) = self.doc.rooms.iter().find(|r| r.id == id) else {
            return;
        };
        if room.name == name {
            return;
        }
        self.history.execute(
            Box::new(RenameRoom {
                id,
                before: Some(room.name.clone()),
                after: name.to_string(),
            }),
            &mut self.doc,
        );
        self.note = Some(format!("房间改名为 {name}"));
        self.touch();
    }

    fn selected_wall_id(&self) -> Option<EntityId> {
        self.selection.first().copied()
    }

    fn cycle_wall_kind(&mut self) {
        let Some(id) = self.selected_wall_id() else {
            return;
        };
        let Some(w) = self.doc.walls.iter().find(|w| w.id == id) else {
            return;
        };
        let before = w.kind;
        let after = match before {
            WallKind::Partition => WallKind::Exterior,
            WallKind::Exterior => WallKind::Bearing,
            WallKind::Bearing => WallKind::Low,
            WallKind::Low => WallKind::Partition,
        };
        self.history
            .execute(Box::new(SetWallKind { id, before, after }), &mut self.doc);
    }

    fn set_wall_kind(&mut self, kind: WallKind) {
        let Some(id) = self.selected_wall_id() else {
            return;
        };
        let Some(w) = self.doc.walls.iter().find(|w| w.id == id) else {
            return;
        };
        if w.kind == kind {
            return;
        }
        let before = w.kind;
        self.history.execute(
            Box::new(SetWallKind {
                id,
                before,
                after: kind,
            }),
            &mut self.doc,
        );
    }

    fn set_wall_finish(&mut self, finish: WallFinish) {
        let Some(id) = self.selected_wall_id() else {
            return;
        };
        let Some(w) = self.doc.walls.iter().find(|w| w.id == id) else {
            return;
        };
        let kind = match finish {
            WallFinish::Paint => "paint",
            WallFinish::Wallpaper => "wallpaper",
            WallFinish::WoodPanel => "wood_panel",
            WallFinish::Tile => "tile",
        };
        let after_id = crate::templates::built_in_finish_id(&self.doc, FinishSurface::Wall, kind);
        let before = (w.finish_id, w.finish);
        let after = (after_id, finish);
        if before == after {
            return;
        }
        self.history.execute(
            Box::new(SetWallFinishId { id, before, after }),
            &mut self.doc,
        );
        self.touch();
    }

    /// Pick a wall finish from the library. The legacy enum stays in sync so
    /// an older reader still sees the closest built-in type.
    fn set_wall_finish_material(&mut self, material: EntityId) {
        let Some(wall_id) = self.selected_wall_id() else {
            return;
        };
        let Some(material_record) = self
            .doc
            .materials
            .iter()
            .find(|m| m.id == material && m.surface == FinishSurface::Wall)
            .cloned()
        else {
            return;
        };
        let Some(wall) = self.doc.walls.iter().find(|w| w.id == wall_id) else {
            return;
        };
        let before = (wall.finish_id, wall.finish);
        let after_finish = match material_record.kind.as_str() {
            "wallpaper" => WallFinish::Wallpaper,
            "wood_panel" => WallFinish::WoodPanel,
            "tile" => WallFinish::Tile,
            _ => WallFinish::Paint,
        };
        if before == (Some(material), after_finish) {
            return;
        }
        self.history.execute(
            Box::new(SetWallFinishId {
                id: wall_id,
                before,
                after: (Some(material), after_finish),
            }),
            &mut self.doc,
        );
        self.touch();
    }

    fn apply_wall_finish_to_selection(&mut self, material: EntityId) {
        let ids: Vec<EntityId> = self.selection.clone();
        if ids.is_empty() {
            return;
        }
        self.apply_wall_finish_to_walls(&ids, material);
    }

    fn apply_wall_finish_to_floor(&mut self) {
        let Some(wall) = self.selected_wall_id().and_then(|id| {
            self.doc.walls.iter().find(|w| w.id == id)
        }) else {
            return;
        };
        let floor = wall.floor;
        let Some(finish_id) = wall.finish_id else {
            return;
        };
        let ids: Vec<EntityId> = self
            .doc
            .walls
            .iter()
            .filter(|w| w.floor == floor)
            .map(|w| w.id)
            .collect();
        self.apply_wall_finish_to_walls(&ids, finish_id);
    }

    fn apply_wall_finish_to_walls(&mut self, ids: &[EntityId], material: EntityId) {
        let Some(record) = self
            .doc
            .materials
            .iter()
            .find(|m| m.id == material && m.surface == FinishSurface::Wall)
            .cloned()
        else {
            return;
        };
        let finish = match record.kind.as_str() {
            "wallpaper" => WallFinish::Wallpaper,
            "wood_panel" => WallFinish::WoodPanel,
            "tile" => WallFinish::Tile,
            _ => WallFinish::Paint,
        };
        let mut before = Vec::new();
        let mut after = Vec::new();
        for wall in self.doc.walls.iter().filter(|w| ids.contains(&w.id)) {
            before.push((wall.id, wall.finish_id, wall.finish));
            after.push((wall.id, Some(material), finish));
        }
        if before == after {
            return;
        }
        self.history.execute(
            Box::new(SetWallFinishes { before, after }),
            &mut self.doc,
        );
        self.touch();
    }

    fn set_room_ceiling_material(&mut self, material: EntityId) {
        let Some(room_id) = self.selected_room else {
            return;
        };
        let valid = self
            .doc
            .materials
            .iter()
            .any(|m| m.id == material && m.surface == FinishSurface::Ceiling);
        if !valid {
            return;
        }
        let Some(room) = self.doc.rooms.iter().find(|r| r.id == room_id) else {
            return;
        };
        let before = room.ceiling_material_id;
        if before == Some(material) {
            return;
        }
        self.history.execute(
            Box::new(SetRoomCeilingMaterial {
                room_id,
                before,
                after: Some(material),
            }),
            &mut self.doc,
        );
        self.touch();
    }

    fn set_wall_length(&mut self, length_mm: i32) {
        let Some(id) = self.selected_wall_id() else {
            return;
        };
        let Some(w) = self.doc.walls.iter().find(|w| w.id == id) else {
            return;
        };
        let before = [w.start, w.end];
        let dx = (w.end.x - w.start.x) as f64;
        let dy = (w.end.y - w.start.y) as f64;
        let current = dx.hypot(dy);
        if current < 1.0 {
            return;
        }
        let target = (length_mm as f64).clamp(300.0, 30_000.0);
        let after = [
            w.start,
            Pt::new(
                (w.start.x as f64 + dx / current * target).round() as i32,
                (w.start.y as f64 + dy / current * target).round() as i32,
            ),
        ];
        if after == before {
            return;
        }
        // Keep joints connected: every other wall endpoint that sat on the old
        // end follows the moved corner instead of leaving a gap.
        let old_end = before[1];
        let new_end = after[1];
        let mut moves = vec![WallMove { id, before, after }];
        for other in self.doc.walls.iter().filter(|other| other.id != id) {
            let before_other = [other.start, other.end];
            let mut after_other = before_other;
            if before_other[0] == old_end {
                after_other[0] = new_end;
            }
            if before_other[1] == old_end {
                after_other[1] = new_end;
            }
            if before_other != after_other {
                moves.push(WallMove {
                    id: other.id,
                    before: before_other,
                    after: after_other,
                });
            }
        }
        self.history.execute(
            Box::new(MoveWalls {
                moves,
                before_rooms: None,
            }),
            &mut self.doc,
        );
        self.note = Some(format!("墙体长度改为 {:.0} mm", target));
    }

    fn set_wall_size(&mut self, thickness: Option<i32>, height: Option<i32>) {
        let Some(id) = self.selected_wall_id() else {
            return;
        };
        let Some(w) = self.doc.walls.iter().find(|w| w.id == id) else {
            return;
        };
        let before = (w.thickness_mm, w.height_mm);
        let after = (
            thickness
                .map(|v| v.clamp(60, 500))
                .unwrap_or(w.thickness_mm),
            height.map(|v| v.clamp(1000, 4000)).unwrap_or(w.height_mm),
        );
        if before == after {
            return;
        }
        self.history
            .execute(Box::new(SetWallSize { id, before, after }), &mut self.doc);
    }

    fn adjust_wall(&mut self, dthick: i32, dheight: i32) {
        let Some(id) = self.selected_wall_id() else {
            return;
        };
        let Some(w) = self.doc.walls.iter().find(|w| w.id == id) else {
            return;
        };
        let before = (w.thickness_mm, w.height_mm);
        let after = (
            (w.thickness_mm + dthick).clamp(60, 500),
            (w.height_mm + dheight).clamp(1000, 4000),
        );
        if before == after {
            return;
        }
        self.history
            .execute(Box::new(SetWallSize { id, before, after }), &mut self.doc);
    }

    fn set_opening_size(&mut self, width: Option<i32>, height: Option<i32>, sill: Option<i32>) {
        let Some(id) = self.selected_opening else {
            return;
        };
        let Some(o) = self.doc.openings.iter().find(|o| o.id == id) else {
            return;
        };
        let before = (o.width_mm, o.height_mm, o.sill_mm);
        let after = (
            width.map(|v| v.clamp(300, 4000)).unwrap_or(o.width_mm),
            height.map(|v| v.clamp(300, 3000)).unwrap_or(o.height_mm),
            sill.map(|v| v.clamp(0, 2000)).unwrap_or(o.sill_mm),
        );
        if before == after {
            return;
        }
        self.history
            .execute(Box::new(SetOpening { id, before, after }), &mut self.doc);
    }

    fn set_opening_price(&mut self, price: i32) {
        let Some(id) = self.selected_opening else {
            return;
        };
        let Some(o) = self.doc.openings.iter().find(|o| o.id == id) else {
            return;
        };
        let after = Some(price.clamp(0, 1_000_000) as f64);
        if o.price == after {
            return;
        }
        self.history.execute(
            Box::new(SetOpeningPrice {
                id,
                before: o.price,
                after,
            }),
            &mut self.doc,
        );
        self.note = Some(format!("门窗单价改为 ¥{price}"));
    }

    fn adjust_opening(&mut self, dw: i32, dh: i32, dsill: i32) {
        let Some(id) = self.selected_opening else {
            return;
        };
        let Some(o) = self.doc.openings.iter().find(|o| o.id == id) else {
            return;
        };
        let before = (o.width_mm, o.height_mm, o.sill_mm);
        let after = (
            (o.width_mm + dw).clamp(300, 4000),
            (o.height_mm + dh).clamp(300, 3000),
            (o.sill_mm + dsill).clamp(0, 2000),
        );
        if before == after {
            return;
        }
        self.history
            .execute(Box::new(SetOpening { id, before, after }), &mut self.doc);
    }

    fn remove_opening(&mut self) {
        let Some(id) = self.selected_opening.take() else {
            return;
        };
        if let Some(pos) = self.doc.openings.iter().position(|o| o.id == id) {
            let opening = self.doc.openings.remove(pos);
            self.history
                .execute(Box::new(RemoveOpening { opening }), &mut self.doc);
        }
    }

    /// Opening under the cursor. Doors are picked by their whole swing area,
    /// not just the leaf line: the drawn arc and the leaf itself sit away from
    /// the wall centreline, so a centreline-only hit test made doors feel
    /// unselectable.
    fn hit_opening(&self) -> Option<EntityId> {
        let world = self.pointer_world();
        let tol = (12.0 / self.cam.scale).max(160.0);
        self.doc
            .openings
            .iter()
            .rev()
            .find(|o| {
                let Some(w) = self
                    .doc
                    .walls
                    .iter()
                    .find(|w| w.id == o.wall_id && w.floor == self.floor())
                else {
                    return false;
                };
                let len = ((w.end.x - w.start.x) as f64).hypot((w.end.y - w.start.y) as f64);
                if len < 1.0 {
                    return false;
                }
                let dx = (w.end.x - w.start.x) as f64 / len;
                let dy = (w.end.y - w.start.y) as f64 / len;
                let at = |mm: i32| {
                    Pt::new(
                        (w.start.x as f64 + dx * mm as f64).round() as i32,
                        (w.start.y as f64 + dy * mm as f64).round() as i32,
                    )
                };
                let a = at(o.offset_mm);
                let b = at(o.offset_mm + o.width_mm);
                if distance_point_segment(world, a, b) <= tol {
                    return true;
                }
                if matches!(o.kind, OpeningKind::Door | OpeningKind::Sliding) {
                    let side = if o.swing_deg >= 0.0 { 1.0 } else { -1.0 };
                    let nx = -dy * side;
                    let ny = dx * side;
                    let hinge = if o.hinge_right { b } else { a };
                    let jamb = if o.hinge_right { a } else { b };
                    let leaf_end = Pt::new(
                        (hinge.x as f64 + nx * o.width_mm as f64).round() as i32,
                        (hinge.y as f64 + ny * o.width_mm as f64).round() as i32,
                    );
                    if distance_point_segment(world, hinge, leaf_end) <= tol {
                        return true;
                    }
                    // The drawn swing arc is a quarter circle of radius = door
                    // width. Pick it only in that sector, not a full disc, so
                    // clicking the floor around a door does not hijack the room.
                    let vx = (world.x - hinge.x) as f64;
                    let vy = (world.y - hinge.y) as f64;
                    let jx = (jamb.x - hinge.x) as f64;
                    let jy = (jamb.y - hinge.y) as f64;
                    let in_sector = vx * nx + vy * ny >= -1.0 && vx * jx + vy * jy >= -1.0;
                    let radius = vx.hypot(vy);
                    if in_sector && (radius - o.width_mm as f64).abs() <= tol {
                        return true;
                    }
                }
                false
            })
            .map(|o| o.id)
    }

    /// Text drawn on top of the plan: wall lengths and room names.
    pub fn labels(&self) -> Vec<PlanLabel> {
        let mut out = Vec::new();
        if self.view != ViewMode::Plan {
            return out;
        }
        let floor = self.floor();
        if self.doc.settings.show_dimensions {
            for wall in self.doc.walls.iter().filter(|w| w.floor == floor) {
                let a = self.cam.world_to_screen(wall.start);
                let b = self.cam.world_to_screen(wall.end);
                let len_px = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                if len_px < 48.0 {
                    continue;
                }
                let len_mm =
                    ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
                let dir = [(b[0] - a[0]) / len_px, (b[1] - a[1]) / len_px];
                let normal = [-dir[1], dir[0]];
                let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                out.push(PlanLabel {
                    text: format!("{len_mm:.0}"),
                    pos: [mid[0] + normal[0] * 13.0, mid[1] + normal[1] * 13.0],
                    bold: false,
                });
            }
        }
        for measurement in &self.measurements {
            let a = self.cam.world_to_screen(measurement.from);
            let b = self.cam.world_to_screen(measurement.to);
            let len_px = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            if len_px < 1.0 {
                continue;
            }
            let dir = [(b[0] - a[0]) / len_px, (b[1] - a[1]) / len_px];
            let normal = [-dir[1], dir[0]];
            let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            out.push(PlanLabel {
                text: format!("{:.2} m", measurement.length_mm() / 1000.0),
                pos: [mid[0] + normal[0] * 14.0, mid[1] + normal[1] * 14.0],
                bold: true,
            });
        }
        if self.tool == Tool::Measure
            && let Some(from) = self.measure_from
        {
            let to = constrained_end(
                from,
                self.snap_measure(self.pointer_world()),
                self.shift_down,
                1,
            );
            let a = self.cam.world_to_screen(from);
            let b = self.cam.world_to_screen(to);
            let len_px = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            if len_px >= 1.0 {
                out.push(PlanLabel {
                    text: format!(
                        "{:.2} m",
                        ((to.x - from.x) as f64).hypot((to.y - from.y) as f64) / 1000.0
                    ),
                    pos: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5 - 14.0],
                    bold: true,
                });
            }
        }
        for room in self.doc.rooms.iter().filter(|r| r.floor == floor) {
            if !self.doc.settings.show_room_names {
                break;
            }
            let area = fp_geom::area(&room.polygon) / 1.0e6;
            if area < 1.5 {
                continue;
            }
            let n = room.polygon.len().max(1) as f64;
            let cx = room.polygon.iter().map(|p| p.x as f64).sum::<f64>() / n;
            let cy = room.polygon.iter().map(|p| p.y as f64).sum::<f64>() / n;
            let pos = self
                .cam
                .world_to_screen(Pt::new(cx.round() as i32, cy.round() as i32));
            out.push(PlanLabel {
                text: format!("{} {:.1}m²", room.name, area),
                pos,
                bold: true,
            });
        }
        out
    }

    /// Ground-material estimate with the same 5% cutting allowance as the
    /// export. Only counted rooms with a priced material contribute.
    pub fn quote_totals(&self) -> QuoteTotals {
        const WASTE: f64 = 0.05;
        let mut out = QuoteTotals::default();
        for room in self.doc.rooms.iter().filter(|r| r.counted) {
            let net = fp_geom::area(&room.polygon).abs() / 1.0e6;
            let price = room
                .material_id
                .and_then(|id| self.doc.materials.iter().find(|m| m.id == id))
                .map(|m| m.price_per_m2)
                .unwrap_or(0.0);
            let billed = net * (1.0 + WASTE);
            let cost = billed * price;
            out.net_area += net;
            out.billed_area += billed;
            out.material_cost += cost;
            out.waste_cost += (billed - net) * price;
        }
        out
    }

    /// Net / billed area and cost for one room, used by the inspector.
    pub fn room_quote(&self, id: EntityId) -> Option<(f64, f64, f64, f64)> {
        const WASTE: f64 = 0.05;
        let room = self.doc.rooms.iter().find(|r| r.id == id)?;
        let net = fp_geom::area(&room.polygon).abs() / 1.0e6;
        let price = room
            .material_id
            .and_then(|mid| self.doc.materials.iter().find(|m| m.id == mid))
            .map(|m| m.price_per_m2)
            .unwrap_or(0.0);
        let billed = net * (1.0 + WASTE);
        Some((net, billed, price, billed * price))
    }

    pub fn status(&self) -> String {
        let quote = self.quote_totals();
        let area = quote.net_area.max(0.0);
        let t = crate::i18n::T(self.lang);
        let view_label = match self.view {
            ViewMode::Plan => "2D".to_string(),
            ViewMode::Three => format!("3D {:.1}m", self.cam3.distance),
            ViewMode::Walk => t.f("漫游 1.62m", "walk 1.62m"),
        };
        let doors = self
            .doc
            .openings
            .iter()
            .filter(|o| o.kind == fp_model::OpeningKind::Door)
            .count();
        let windows = self.doc.openings.len() - doors;
        let scale = self.scale_denominator();
        let base = t.f(
            &format!(
                "{} 面墙 · {} 门 · {} 窗 · {:.1} m² 套内 · 选中 {} · {} · 1:{:.0} · 地面 ¥{:.0}",
                self.doc.walls.len(),
                doors,
                windows,
                area,
                self.selection.len(),
                view_label,
                scale,
                quote.material_cost
            ),
            &format!(
                "{} walls · {} doors · {} windows · {:.1} m² usable · {} selected · {} · 1:{:.0} · floor ¥{:.0}",
                self.doc.walls.len(),
                doors,
                windows,
                area,
                self.selection.len(),
                view_label,
                scale,
                quote.material_cost
            ),
        );
        let furn_note = self.selected_furniture.and_then(|id| {
            self.doc
                .furniture
                .iter()
                .find(|f| f.id == id)
                .map(|f| format!("{} {}×{}", f.name, f.width_mm, f.depth_mm))
        });
        let base = match furn_note {
            Some(f) => format!("{base} · {f}"),
            None => base,
        };
        let base = match self.selected_room.and_then(|id| {
            self.doc.rooms.iter().find(|r| r.id == id).map(|r| {
                let area = fp_geom::area(&r.polygon).abs() / 1.0e6;
                let cost = self.room_quote(id).map(|q| q.3).unwrap_or(0.0);
                format!("{} {:.1}m² · ¥{:.0}", r.name, area, cost)
            })
        }) {
            Some(room) => format!("{base} · {room}"),
            None => base,
        };
        match &self.note {
            Some(note) => format!("{base} · {note}"),
            None => base,
        }
    }
}

/// Recover rooms from wall centrelines, keeping names/materials of rooms whose
/// old polygon still contains the new room's centroid.
fn recompute_rooms(doc: &mut Document) {
    let levels = doc.level_count();
    let old = doc.rooms.clone();
    let default_material = doc
        .materials
        .iter()
        .find(|m| m.surface == FinishSurface::Floor)
        .map(|m| m.id);
    let default_ceiling =
        crate::templates::built_in_finish_id(doc, FinishSurface::Ceiling, "paint");
    let mut fallback = doc.next_id();
    let mut rooms: Vec<Room> = Vec::new();
    for floor in 0..levels {
        let segments: Vec<(Pt, Pt)> = doc
            .walls
            .iter()
            .filter(|w| w.floor as usize == floor)
            .map(|w| (w.start, w.end))
            .collect();
        let polygons = detect_rooms(&segments, ROOM_JOIN_TOLERANCE_MM, 0.3);
        let old_floor: Vec<&Room> = old.iter().filter(|r| r.floor as usize == floor).collect();
        for polygon in polygons {
            let n = polygon.len().max(1) as f64;
            let cx = polygon.iter().map(|p| p.x as f64).sum::<f64>() / n;
            let cy = polygon.iter().map(|p| p.y as f64).sum::<f64>() / n;
            let centroid = Pt::new(cx.round() as i32, cy.round() as i32);
            let hit = old_floor
                .iter()
                .find(|r| r.polygon.len() >= 3 && point_in_polygon(centroid, &r.polygon));
            let (id, name, material_id, counted, ceiling_material_id) = match hit {
                Some(r) => (
                    r.id,
                    r.name.clone(),
                    r.material_id,
                    r.counted,
                    r.ceiling_material_id,
                ),
                None => {
                    let id = EntityId(fallback);
                    fallback += 1;
                    (
                        id,
                        format!(
                            "房间 {}",
                            rooms.iter().filter(|r| r.floor as usize == floor).count() + 1
                        ),
                        default_material,
                        true,
                        default_ceiling,
                    )
                }
            };
            rooms.push(Room {
                id,
                polygon,
                name,
                material_id,
                counted,
                floor: floor as u32,
                ceiling_material_id,
            });
        }
    }
    doc.rooms = rooms;
}
// ------------------------------------------------------------------- commands

struct AddMeasure {
    measure: Measure,
}

impl Command for AddMeasure {
    fn label(&self) -> &str {
        "add-measure"
    }
    fn apply(&mut self, doc: &mut Document) {
        if !doc.measures.iter().any(|m| m.id == self.measure.id) {
            doc.measures.push(self.measure.clone());
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.measures.retain(|m| m.id != self.measure.id);
    }
}

struct ClearMeasures {
    before: Vec<Measure>,
}

impl Command for ClearMeasures {
    fn label(&self) -> &str {
        "clear-measures"
    }
    fn apply(&mut self, doc: &mut Document) {
        doc.measures.clear();
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.measures = self.before.clone();
    }
}

struct LevelSnapshot {
    before: Document,
    after: Document,
    label: &'static str,
}

impl Command for LevelSnapshot {
    fn label(&self) -> &str {
        self.label
    }

    fn apply(&mut self, doc: &mut Document) {
        *doc = self.after.clone();
    }

    fn undo(&mut self, doc: &mut Document) {
        *doc = self.before.clone();
    }
}

struct RoofSnapshot {
    before: Document,
    after: Document,
    label: &'static str,
}

impl Command for RoofSnapshot {
    fn label(&self) -> &str {
        self.label
    }
    fn apply(&mut self, doc: &mut Document) {
        *doc = self.after.clone();
    }
    fn undo(&mut self, doc: &mut Document) {
        *doc = self.before.clone();
    }
}

struct AddStair {
    stair: Stair,
}

impl Command for AddStair {
    fn label(&self) -> &str {
        "add stair"
    }

    fn apply(&mut self, doc: &mut Document) {
        doc.stairs.push(self.stair.clone());
    }

    fn undo(&mut self, doc: &mut Document) {
        doc.stairs.retain(|s| s.id != self.stair.id);
    }
}

struct UpdateStair {
    before: Stair,
    after: Stair,
}

impl Command for UpdateStair {
    fn label(&self) -> &str {
        "update stair"
    }

    fn apply(&mut self, doc: &mut Document) {
        if let Some(stair) = doc.stairs.iter_mut().find(|s| s.id == self.after.id) {
            *stair = self.after.clone();
        }
    }

    fn undo(&mut self, doc: &mut Document) {
        if let Some(stair) = doc.stairs.iter_mut().find(|s| s.id == self.before.id) {
            *stair = self.before.clone();
        }
    }
}

struct RemoveStair {
    stair: Stair,
}

impl Command for RemoveStair {
    fn label(&self) -> &str {
        "remove stair"
    }

    fn apply(&mut self, doc: &mut Document) {
        doc.stairs.retain(|s| s.id != self.stair.id);
    }

    fn undo(&mut self, doc: &mut Document) {
        doc.stairs.push(self.stair.clone());
    }
}

struct AddWall {
    wall: Wall,
    before_rooms: Option<Vec<Room>>,
}

struct RenameRoom {
    id: EntityId,
    before: Option<String>,
    after: String,
}

impl Command for RenameRoom {
    fn label(&self) -> &str {
        "rename-room"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(room) = doc.rooms.iter_mut().find(|r| r.id == self.id) {
            room.name = self.after.clone();
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let (Some(before), Some(room)) = (
            self.before.clone(),
            doc.rooms.iter_mut().find(|r| r.id == self.id),
        ) {
            room.name = before;
        }
    }
}

struct PasteBundle {
    walls: Vec<Wall>,
    furniture: Vec<Furniture>,
    openings: Vec<Opening>,
    before_rooms: Option<Vec<Room>>,
}

impl Command for PasteBundle {
    fn label(&self) -> &str {
        "paste"
    }
    fn apply(&mut self, doc: &mut Document) {
        if self.before_rooms.is_none() {
            self.before_rooms = Some(doc.rooms.clone());
        }
        doc.walls.extend(self.walls.iter().cloned());
        doc.furniture.extend(self.furniture.iter().cloned());
        doc.openings.extend(self.openings.iter().cloned());
        recompute_rooms(doc);
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.walls
            .retain(|w| !self.walls.iter().any(|p| p.id == w.id));
        doc.furniture
            .retain(|f| !self.furniture.iter().any(|p| p.id == f.id));
        doc.openings
            .retain(|o| !self.openings.iter().any(|p| p.id == o.id));
        if let Some(rooms) = self.before_rooms.take() {
            doc.rooms = rooms;
        }
    }
}

impl Command for AddWall {
    fn label(&self) -> &str {
        "add-wall"
    }
    fn apply(&mut self, doc: &mut Document) {
        if self.before_rooms.is_none() {
            self.before_rooms = Some(doc.rooms.clone());
        }
        doc.walls.push(self.wall.clone());
        recompute_rooms(doc);
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.walls.retain(|w| w.id != self.wall.id);
        if let Some(rooms) = self.before_rooms.take() {
            doc.rooms = rooms;
        }
    }
}

struct WallMove {
    id: EntityId,
    before: [Pt; 2],
    after: [Pt; 2],
}

struct MoveWalls {
    moves: Vec<WallMove>,
    before_rooms: Option<Vec<Room>>,
}

impl Command for MoveWalls {
    fn label(&self) -> &str {
        "move-walls"
    }
    fn apply(&mut self, doc: &mut Document) {
        if self.before_rooms.is_none() {
            self.before_rooms = Some(doc.rooms.clone());
        }
        for m in &self.moves {
            if let Some(w) = doc.walls.iter_mut().find(|w| w.id == m.id) {
                w.start = m.after[0];
                w.end = m.after[1];
            }
        }
        recompute_rooms(doc);
    }
    fn undo(&mut self, doc: &mut Document) {
        for m in &self.moves {
            if let Some(w) = doc.walls.iter_mut().find(|w| w.id == m.id) {
                w.start = m.before[0];
                w.end = m.before[1];
            }
        }
        if let Some(rooms) = self.before_rooms.take() {
            doc.rooms = rooms;
        }
    }
}

struct AddFurniture {
    item: Furniture,
}

impl Command for AddFurniture {
    fn label(&self) -> &str {
        "add-furniture"
    }
    fn apply(&mut self, doc: &mut Document) {
        doc.furniture.push(self.item.clone());
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.furniture.retain(|f| f.id != self.item.id);
    }
}

#[derive(Clone, Copy)]
struct FurnitureMove {
    id: EntityId,
    before: Pt,
    after: Pt,
}

struct MoveFurnitureGroup {
    moves: Vec<FurnitureMove>,
    label: &'static str,
}

impl Command for MoveFurnitureGroup {
    fn label(&self) -> &str {
        self.label
    }
    fn apply(&mut self, doc: &mut Document) {
        for mv in &self.moves {
            if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == mv.id) {
                f.center = mv.after;
            }
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        for mv in &self.moves {
            if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == mv.id) {
                f.center = mv.before;
            }
        }
    }
}

struct MoveFurniture {
    id: EntityId,
    before: Pt,
    after: Pt,
}

impl Command for MoveFurniture {
    fn label(&self) -> &str {
        "move-furniture"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == self.id) {
            f.center = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == self.id) {
            f.center = self.before;
        }
    }
}

struct RotateFurniture {
    id: EntityId,
    before: f64,
    after: f64,
}

impl Command for RotateFurniture {
    fn label(&self) -> &str {
        "rotate-furniture"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == self.id) {
            f.rotation_deg = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == self.id) {
            f.rotation_deg = self.before;
        }
    }
}

struct SetWallKind {
    id: EntityId,
    before: WallKind,
    after: WallKind,
}

impl Command for SetWallKind {
    fn label(&self) -> &str {
        "set-wall-kind"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(w) = doc.walls.iter_mut().find(|w| w.id == self.id) {
            w.kind = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(w) = doc.walls.iter_mut().find(|w| w.id == self.id) {
            w.kind = self.before;
        }
    }
}

struct SetWallFinishes {
    before: Vec<(EntityId, Option<EntityId>, WallFinish)>,
    after: Vec<(EntityId, Option<EntityId>, WallFinish)>,
}

impl Command for SetWallFinishes {
    fn label(&self) -> &str {
        "set-wall-finishes"
    }
    fn apply(&mut self, doc: &mut Document) {
        for (id, finish_id, finish) in &self.after {
            if let Some(w) = doc.walls.iter_mut().find(|w| w.id == *id) {
                w.finish_id = *finish_id;
                w.finish = *finish;
            }
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        for (id, finish_id, finish) in &self.before {
            if let Some(w) = doc.walls.iter_mut().find(|w| w.id == *id) {
                w.finish_id = *finish_id;
                w.finish = *finish;
            }
        }
    }
}

struct SetWallSize {
    id: EntityId,
    before: (i32, i32),
    after: (i32, i32),
}

impl Command for SetWallSize {
    fn label(&self) -> &str {
        "set-wall-size"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(w) = doc.walls.iter_mut().find(|w| w.id == self.id) {
            w.thickness_mm = self.after.0;
            w.height_mm = self.after.1;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(w) = doc.walls.iter_mut().find(|w| w.id == self.id) {
            w.thickness_mm = self.before.0;
            w.height_mm = self.before.1;
        }
    }
}

struct MoveOpening {
    id: EntityId,
    before: i32,
    after: i32,
}

impl Command for MoveOpening {
    fn label(&self) -> &str {
        "move-opening"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.offset_mm = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.offset_mm = self.before;
        }
    }
}

struct SetQuoteSettings {
    before: QuoteSettings,
    after: QuoteSettings,
}

impl Command for SetQuoteSettings {
    fn label(&self) -> &str {
        "set-quote-settings"
    }
    fn apply(&mut self, doc: &mut Document) {
        doc.quote = self.after.clone();
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.quote = self.before.clone();
    }
}

struct SetOpeningPrice {
    id: EntityId,
    before: Option<f64>,
    after: Option<f64>,
}

impl Command for SetOpeningPrice {
    fn label(&self) -> &str {
        "set-opening-price"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.price = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.price = self.before;
        }
    }
}

struct SetOpening {
    id: EntityId,
    before: (i32, i32, i32),
    after: (i32, i32, i32),
}

impl Command for SetOpening {
    fn label(&self) -> &str {
        "set-opening"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.width_mm = self.after.0;
            o.height_mm = self.after.1;
            o.sill_mm = self.after.2;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.width_mm = self.before.0;
            o.height_mm = self.before.1;
            o.sill_mm = self.before.2;
        }
    }
}

struct SetOpeningSwing {
    id: EntityId,
    before: f64,
    after: f64,
}

impl Command for SetOpeningSwing {
    fn label(&self) -> &str {
        "set-opening-swing"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.swing_deg = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.swing_deg = self.before;
        }
    }
}

struct SetOpeningHinge {
    id: EntityId,
    before: bool,
    after: bool,
}

impl Command for SetOpeningHinge {
    fn label(&self) -> &str {
        "set-opening-hinge"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.hinge_right = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(o) = doc.openings.iter_mut().find(|o| o.id == self.id) {
            o.hinge_right = self.before;
        }
    }
}

struct RemoveOpening {
    opening: Opening,
}

impl Command for RemoveOpening {
    fn label(&self) -> &str {
        "remove-opening"
    }
    fn apply(&mut self, doc: &mut Document) {
        doc.openings.retain(|o| o.id != self.opening.id);
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.openings.push(self.opening.clone());
    }
}

struct SetFurniture {
    id: EntityId,
    before: (i32, i32, String),
    after: (i32, i32, String),
}

impl Command for SetFurniture {
    fn label(&self) -> &str {
        "set-furniture"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == self.id) {
            f.width_mm = self.after.0;
            f.depth_mm = self.after.1;
            f.color = self.after.2.clone();
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == self.id) {
            f.width_mm = self.before.0;
            f.depth_mm = self.before.1;
            f.color = self.before.2.clone();
        }
    }
}

struct SetFurniturePrice {
    id: EntityId,
    before: Option<f64>,
    after: Option<f64>,
}

impl Command for SetFurniturePrice {
    fn label(&self) -> &str {
        "set-furniture-price"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == self.id) {
            f.price = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(f) = doc.furniture.iter_mut().find(|f| f.id == self.id) {
            f.price = self.before;
        }
    }
}

struct RemoveFurnitureBatch {
    items: Vec<(usize, Furniture)>,
}

impl Command for RemoveFurnitureBatch {
    fn label(&self) -> &str {
        "remove-furniture-batch"
    }
    fn apply(&mut self, doc: &mut Document) {
        let ids: Vec<EntityId> = self.items.iter().map(|(_, f)| f.id).collect();
        doc.furniture.retain(|f| !ids.contains(&f.id));
    }
    fn undo(&mut self, doc: &mut Document) {
        let mut sorted = self.items.clone();
        sorted.sort_by_key(|(index, _)| *index);
        for (index, item) in sorted {
            let at = index.min(doc.furniture.len());
            doc.furniture.insert(at, item);
        }
    }
}

struct ClearFurniture {
    before: Vec<Furniture>,
}

impl Command for ClearFurniture {
    fn label(&self) -> &str {
        "clear-furniture"
    }
    fn apply(&mut self, doc: &mut Document) {
        doc.furniture.clear();
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.furniture = self.before.clone();
    }
}

struct ReorderFurniture {
    id: EntityId,
    before: usize,
    after: usize,
}

impl Command for ReorderFurniture {
    fn label(&self) -> &str {
        "reorder-furniture"
    }
    fn apply(&mut self, doc: &mut Document) {
        move_furniture_to(doc, self.id, self.after);
    }
    fn undo(&mut self, doc: &mut Document) {
        move_furniture_to(doc, self.id, self.before);
    }
}

fn move_furniture_to(doc: &mut Document, id: EntityId, target: usize) {
    let Some(from) = doc.furniture.iter().position(|f| f.id == id) else {
        return;
    };
    let item = doc.furniture.remove(from);
    let at = target.min(doc.furniture.len());
    doc.furniture.insert(at, item);
}

struct MaterialSnapshot {
    before: Vec<Material>,
    after: Vec<Material>,
    label: &'static str,
}

impl Command for MaterialSnapshot {
    fn label(&self) -> &str {
        self.label
    }

    fn apply(&mut self, doc: &mut Document) {
        doc.materials = self.after.clone();
    }

    fn undo(&mut self, doc: &mut Document) {
        doc.materials = self.before.clone();
    }
}

struct SetRoomMaterial {
    room_id: EntityId,
    before: Option<EntityId>,
    after: Option<EntityId>,
}

impl Command for SetRoomMaterial {
    fn label(&self) -> &str {
        "set-room-material"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(r) = doc.rooms.iter_mut().find(|r| r.id == self.room_id) {
            r.material_id = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(r) = doc.rooms.iter_mut().find(|r| r.id == self.room_id) {
            r.material_id = self.before;
        }
    }
}

struct SetWallFinishId {
    id: EntityId,
    before: (Option<EntityId>, WallFinish),
    after: (Option<EntityId>, WallFinish),
}

impl Command for SetWallFinishId {
    fn label(&self) -> &str {
        "set-wall-finish-id"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(w) = doc.walls.iter_mut().find(|w| w.id == self.id) {
            w.finish_id = self.after.0;
            w.finish = self.after.1;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(w) = doc.walls.iter_mut().find(|w| w.id == self.id) {
            w.finish_id = self.before.0;
            w.finish = self.before.1;
        }
    }
}

struct SetRoomCeilingMaterial {
    room_id: EntityId,
    before: Option<EntityId>,
    after: Option<EntityId>,
}

impl Command for SetRoomCeilingMaterial {
    fn label(&self) -> &str {
        "set-room-ceiling-material"
    }
    fn apply(&mut self, doc: &mut Document) {
        if let Some(r) = doc.rooms.iter_mut().find(|r| r.id == self.room_id) {
            r.ceiling_material_id = self.after;
        }
    }
    fn undo(&mut self, doc: &mut Document) {
        if let Some(r) = doc.rooms.iter_mut().find(|r| r.id == self.room_id) {
            r.ceiling_material_id = self.before;
        }
    }
}

struct AddOpening {
    opening: Opening,
}

impl Command for AddOpening {
    fn label(&self) -> &str {
        "add-opening"
    }
    fn apply(&mut self, doc: &mut Document) {
        doc.openings.push(self.opening.clone());
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.openings.retain(|o| o.id != self.opening.id);
    }
}

struct RemoveWalls {
    walls: Vec<Wall>,
    openings: Vec<Opening>,
    before_rooms: Option<Vec<Room>>,
}

impl Command for RemoveWalls {
    fn label(&self) -> &str {
        "remove-walls"
    }
    fn apply(&mut self, doc: &mut Document) {
        if self.before_rooms.is_none() {
            self.before_rooms = Some(doc.rooms.clone());
        }
        let ids: Vec<EntityId> = self.walls.iter().map(|w| w.id).collect();
        doc.walls.retain(|w| !ids.contains(&w.id));
        doc.openings.retain(|o| !ids.contains(&o.wall_id));
        recompute_rooms(doc);
    }
    fn undo(&mut self, doc: &mut Document) {
        doc.walls.extend(self.walls.iter().cloned());
        doc.openings.extend(self.openings.iter().cloned());
        if let Some(rooms) = self.before_rooms.take() {
            doc.rooms = rooms;
        }
    }
}

// -------------------------------------------------------------------- helpers

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Middle,
    Right,
}

/// A fresh, collision-free file under `Documents/Fathom/Projects`, so creating
/// a new plan can autosave immediately without clobbering the previous one.
pub(crate) fn next_project_path(stem: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = PathBuf::from(home).join("Documents/Fathom/Projects");
    let _ = fs::create_dir_all(&dir);
    let cleaned: String = stem
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base = cleaned.trim_matches(['-', ' ']);
    let base = if base.is_empty() {
        "未命名户型"
    } else {
        base
    };
    let direct = dir.join(format!("{base}.json"));
    if !direct.exists() {
        return direct;
    }
    for n in 2..1000 {
        let candidate = dir.join(format!("{base}-{n}.json"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{base}-{}.json", std::process::id()))
}

pub(crate) fn user_plan_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let next = PathBuf::from(&home).join("Documents/Fathom/plan.json");
    let legacy = PathBuf::from(&home).join("Documents/Floorplan3D/plan.json");
    let legacy_is_newer = match (fs::metadata(&legacy), fs::metadata(&next)) {
        (Ok(a), Ok(b)) => match (a.modified(), b.modified()) {
            (Ok(a), Ok(b)) => a > b,
            _ => false,
        },
        (Ok(_), Err(_)) => true,
        _ => false,
    };
    if legacy_is_newer {
        if let Some(parent) = next.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::copy(&legacy, &next);
    }
    next
}

pub(crate) fn display_path(path: &std::path::Path) -> String {
    match std::env::var("HOME") {
        Ok(home) => path.to_string_lossy().replacen(&home, "~", 1).to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

/// Shortest distance from `p` to the polygon outline (millimetres).
fn polygon_margin(p: Pt, polygon: &[Pt]) -> f64 {
    if polygon.len() < 2 {
        return 0.0;
    }
    let mut best = f64::MAX;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        best = best.min(distance_point_segment(p, a, b));
    }
    best
}

/// Spawn the walkthrough in the centre of the largest room, looking down its long axis.
fn walk_camera(doc: &Document) -> WalkCamera {
    let Some(room) = doc
        .rooms
        .iter()
        .max_by(|a, b| fp_geom::area(&a.polygon).total_cmp(&fp_geom::area(&b.polygon)))
    else {
        return WalkCamera::new(Vec3::new(0.0, EYE_HEIGHT, 0.0), 0.0);
    };
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for p in &room.polygon {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_z = min_z.min(p.y);
        max_z = max_z.max(p.y);
    }
    let mut spot = None;
    for ix in 0..5 {
        for iz in 0..5 {
            let x = min_x + (max_x - min_x) * (2 * ix + 1) / 10;
            let z = min_z + (max_z - min_z) * (2 * iz + 1) / 10;
            let p = Pt::new(x, z);
            if point_in_polygon(p, &room.polygon) && polygon_margin(p, &room.polygon) >= 150.0 {
                spot = Some((x, z));
                break;
            }
        }
        if spot.is_some() {
            break;
        }
    }
    let (x, z) = spot.unwrap_or(((min_x + max_x) / 2, (min_z + max_z) / 2));
    let yaw = if (max_x - min_x) >= (max_z - min_z) {
        std::f32::consts::FRAC_PI_2
    } else {
        0.0
    };
    WalkCamera::new(
        Vec3::new(x as f32 / 1000.0, EYE_HEIGHT, z as f32 / 1000.0),
        yaw,
    )
}

fn load_demo() -> Document {
    let json = include_str!("../../../assets/samples/demo-apartment.json");
    Document::from_json(json).unwrap_or_else(|_| Document::new("未命名户型"))
}

pub fn plan_bounds(doc: &Document) -> (Pt, Pt) {
    let mut min = Pt::new(i32::MAX, i32::MAX);
    let mut max = Pt::new(i32::MIN, i32::MIN);
    for wall in &doc.walls {
        for p in [wall.start, wall.end] {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
    }
    if min.x > max.x {
        (Pt::new(0, 0), Pt::new(10_000, 8_000))
    } else {
        (min, max)
    }
}

pub fn plan_bounds_for_floor(doc: &Document, floor: u32) -> (Pt, Pt) {
    let mut min = Pt::new(i32::MAX, i32::MAX);
    let mut max = Pt::new(i32::MIN, i32::MIN);
    for wall in doc.walls.iter().filter(|w| w.floor == floor) {
        for p in [wall.start, wall.end] {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
    }
    for room in doc.rooms.iter().filter(|r| r.floor == floor) {
        for p in &room.polygon {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
    }
    if min.x > max.x {
        (Pt::new(0, 0), Pt::new(10_000, 8_000))
    } else {
        (min, max)
    }
}

fn default_thickness(doc: &Document) -> Mm {
    doc.walls
        .iter()
        .map(|w| w.thickness_mm)
        .next()
        .unwrap_or(120)
}

fn push_screen_line(batch: &mut Batch, a: [f32; 2], b: [f32; 2], width_px: f32, color: Color) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 1e-3 {
        return;
    }
    let n = [-d[1] / len * width_px * 0.5, d[0] / len * width_px * 0.5];
    batch.push_quad(
        [
            [a[0] + n[0], a[1] + n[1]],
            [b[0] + n[0], b[1] + n[1]],
            [b[0] - n[0], b[1] - n[1]],
            [a[0] - n[0], a[1] - n[1]],
        ],
        color,
    );
}

/// Dashed line in world space, so the dash pitch follows the drawing scale.
fn push_dashed_line(batch: &mut Batch, cam: &Camera2D, a: Pt, b: Pt, width_px: f32, color: Color) {
    let len = ((b.x - a.x) as f64).hypot((b.y - a.y) as f64);
    if len < 1.0 {
        return;
    }
    let ux = (b.x - a.x) as f64 / len;
    let uy = (b.y - a.y) as f64 / len;
    const DASH_MM: f64 = 260.0;
    const GAP_MM: f64 = 140.0;
    let mut along = 0.0;
    while along < len {
        let end = (along + DASH_MM).min(len);
        let p0 = Pt::new(
            a.x + (ux * along).round() as i32,
            a.y + (uy * along).round() as i32,
        );
        let p1 = Pt::new(
            a.x + (ux * end).round() as i32,
            a.y + (uy * end).round() as i32,
        );
        push_thick_line(batch, cam, p0, p1, width_px, color);
        along += DASH_MM + GAP_MM;
    }
}

fn push_world_cross(batch: &mut Batch, cam: &Camera2D, p: Pt, color: Color) {
    let c = cam.world_to_screen(p);
    const R: f32 = 6.0;
    push_screen_line(batch, [c[0] - R, c[1]], [c[0] + R, c[1]], 1.6, color);
    push_screen_line(batch, [c[0], c[1] - R], [c[0], c[1] + R], 1.6, color);
}

fn push_thick_line(batch: &mut Batch, cam: &Camera2D, a: Pt, b: Pt, width_px: f32, color: Color) {
    let sa = cam.world_to_screen(a);
    let sb = cam.world_to_screen(b);
    let d = [sb[0] - sa[0], sb[1] - sa[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 1e-3 {
        return;
    }
    push_screen_line(batch, sa, sb, width_px, color);
}

fn push_rect_outline(batch: &mut Batch, rect: [f32; 4], width: f32, color: Color) {
    let [x0, y0, x1, y1] = rect;
    let c = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    for i in 0..4 {
        let a = c[i];
        let b = c[(i + 1) % 4];
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if len < 1e-3 {
            continue;
        }
        let n = [-d[1] / len * width * 0.5, d[0] / len * width * 0.5];
        batch.push_quad(
            [
                [a[0] + n[0], a[1] + n[1]],
                [b[0] + n[0], b[1] + n[1]],
                [b[0] - n[0], b[1] - n[1]],
                [a[0] - n[0], a[1] - n[1]],
            ],
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fp_model::Material;

    fn empty() -> SceneState {
        SceneState::from_doc(
            Document::new("test"),
            [1000.0, 800.0],
            PathBuf::from("/tmp/fp-test-plan.json"),
        )
    }

    fn draw_wall(s: &mut SceneState) {
        s.set_tool(Tool::Wall);
        s.pointer_down([200.0, 400.0], PointerButton::Left, false);
        s.pointer_down([600.0, 400.0], PointerButton::Left, false);
    }

    fn room_state() -> SceneState {
        let mut doc = Document::new("pick-test");
        let pts = [
            (Pt::new(0, 0), Pt::new(6000, 0)),
            (Pt::new(6000, 0), Pt::new(6000, 4000)),
            (Pt::new(6000, 4000), Pt::new(0, 4000)),
            (Pt::new(0, 4000), Pt::new(0, 0)),
        ];
        for (i, (a, b)) in pts.into_iter().enumerate() {
            doc.walls.push(Wall::new(
                EntityId(100 + i as u128),
                a,
                b,
                120,
                WallKind::Partition,
            ));
        }
        let mut state = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/x.json"));
        recompute_rooms(&mut state.doc);
        state
    }

    #[test]
    fn library_drop_places_furniture_at_the_pointer() {
        let mut s = empty();
        s.place_furniture_at(0, [500.0, 400.0]);
        assert_eq!(s.doc.furniture.len(), 1);
        assert_eq!(s.furniture_selection.len(), 1);
        assert_eq!(s.tool, Tool::Select);
        assert!(s.placing.is_none());
    }

    #[test]
    fn walk_keys_follow_the_camera_basis() {
        let mut s = room_state();
        s.set_view(ViewMode::Walk);
        let (forward, right) = s.walk.basis();
        let before = s.walk.pos;
        s.key("d", false, false);
        s.walk_step(0.1);
        let delta = s.walk.pos - before;
        assert!(
            delta.dot(right) > 0.02,
            "D should move screen-right, got {delta:?}"
        );
        s.set_view(ViewMode::Walk);
        let before = s.walk.pos;
        s.key("w", false, false);
        s.walk_step(0.1);
        let delta = s.walk.pos - before;
        assert!(
            delta.dot(forward) > 0.02,
            "W should move forward, got {delta:?}"
        );
        s.set_view(ViewMode::Walk);
        let before = s.walk.pos;
        s.walk_scroll(20.0);
        let delta = s.walk.pos - before;
        assert!(
            delta.dot(forward) > 0.02,
            "wheel-up should move forward, got {delta:?}"
        );
    }

    #[test]
    fn ray_pick_hits_room_from_above() {
        let s = room_state();
        let hit = pick_scene(&s.doc, Vec3::new(3.0, 5.0, 2.0), Vec3::new(0.0, -1.0, 0.0));
        assert!(matches!(hit, Some(Pick3d::Room(_))), "{hit:?}");
    }

    #[test]
    fn ray_pick_hits_wall_sideways() {
        let s = room_state();
        // North wall runs from (0,0) to (6,0) at z = 0; shoot along +z from outside.
        let hit = pick_scene(&s.doc, Vec3::new(3.0, 1.4, -3.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(matches!(hit, Some(Pick3d::Wall(_))), "{hit:?}");
    }

    #[test]
    fn ray_pick_prefers_furniture_over_floor() {
        let mut s = room_state();
        s.doc.furniture.push(Furniture {
            id: EntityId(900),
            asset_id: "sofa".into(),
            name: "沙发".into(),
            center: Pt::new(3000, 2000),
            width_mm: 2200,
            depth_mm: 900,
            rotation_deg: 0.0,
            color: "#C9A227".into(),
            room_id: None,
            price: None,
            floor: 0,
        });
        let hit = pick_scene(&s.doc, Vec3::new(3.0, 3.0, 2.0), Vec3::new(0.0, -1.0, 0.0));
        assert!(matches!(hit, Some(Pick3d::Furniture(_))), "{hit:?}");
    }

    #[test]
    fn selected_wall_is_tinted_in_3d() {
        let mut s = room_state();
        s.selection = vec![EntityId(100)];
        let mesh = s.mesh3d();
        assert!(
            mesh.iter().any(|v| v.color == HIGHLIGHT_3D),
            "no highlighted vertices"
        );
        s.selection.clear();
        let plain = s.mesh3d();
        assert!(plain.iter().all(|v| v.color != HIGHLIGHT_3D));
    }

    #[test]
    fn drag_drawing_a_wall_commits_on_release() {
        let mut s = empty();
        s.set_tool(Tool::Wall);
        s.pointer_down([200.0, 300.0], PointerButton::Left, false);
        s.pointer_move([560.0, 300.0]);
        s.pointer_up([560.0, 300.0], PointerButton::Left, false);
        assert_eq!(s.doc.walls.len(), 1);
        let w = &s.doc.walls[0];
        let len = ((w.end.x - w.start.x) as f64).hypot((w.end.y - w.start.y) as f64);
        assert!(len > 1000.0, "drag should span a room, got {len}");
        assert!(s.draft.is_none());
    }

    #[test]
    fn drawing_a_wall_near_a_corner_snaps_to_the_existing_endpoint() {
        let mut s = room_state();
        let existing_end = s.doc.walls[0].end;
        s.set_tool(Tool::Wall);
        let start = s.cam.world_to_screen(Pt::new(existing_end.x, existing_end.y + 35));
        s.pointer_down(start, PointerButton::Left, false);
        let end = s.cam.world_to_screen(Pt::new(existing_end.x + 2000, existing_end.y));
        s.pointer_down(end, PointerButton::Left, false);
        assert_eq!(s.doc.walls.len(), 5);
        let added = s.doc.walls.last().unwrap();
        assert_eq!(added.start, existing_end, "start must weld to the corner");
    }

    #[test]
    fn drawing_a_wall_near_an_existing_wall_projects_a_t_junction() {
        let mut s = room_state();
        s.set_tool(Tool::Wall);
        let start = s.cam.world_to_screen(Pt::new(3000, 45));
        let end = s.cam.world_to_screen(Pt::new(3000, 4000));
        s.pointer_down(start, PointerButton::Left, false);
        s.pointer_down(end, PointerButton::Left, false);
        let added = s.doc.walls.last().unwrap();
        assert_eq!(added.start, Pt::new(3000, 0), "T-junction snapped to wall");
    }

    #[test]
    fn furniture_tool_shows_a_translucent_ghost_before_clicking() {
        let mut s = empty();
        let before = s.build_batch().vertices.len();
        s.set_tool(Tool::Furniture);
        s.set_pointer_over_ui(false);
        s.pointer_move([450.0, 360.0]);
        let after = s.build_batch();
        assert!(after.vertices.len() > before, "ghost should add vertices");
        assert!(
            after.vertices.iter().any(|v| v.color[3] < 0.9),
            "ghost should be translucent"
        );
    }

    #[test]
    fn dragging_a_new_furniture_piece_places_it_at_the_release_point() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([300.0, 300.0], PointerButton::Left, false);
        let start = s.doc.furniture[0].center;
        s.pointer_move([520.0, 430.0]);
        let end = s.doc.furniture[0].center;
        s.pointer_up([520.0, 430.0], PointerButton::Left, false);
        assert!(start != end, "piece should follow the drag");
        assert_eq!(s.doc.furniture.len(), 1);
        assert_eq!(s.tool, Tool::Select);
        assert!(s.placing.is_none());
    }

    #[test]
    fn draw_wall_then_undo_and_redo() {
        let mut s = empty();
        draw_wall(&mut s);
        assert_eq!(s.doc.walls.len(), 1);
        s.key("z", true, false);
        assert_eq!(s.doc.walls.len(), 0);
        s.key("z", true, true);
        assert_eq!(s.doc.walls.len(), 1);
    }

    #[test]
    fn delete_selected_then_undo() {
        let mut s = empty();
        draw_wall(&mut s);
        let id = s.doc.walls[0].id;
        s.selection = vec![id];
        s.key("delete", false, false);
        assert!(s.doc.walls.is_empty());
        s.key("z", true, false);
        assert_eq!(s.doc.walls.len(), 1);
    }

    #[test]
    fn demolish_tool_removes_partition_but_refuses_bearing() {
        let mut s = empty();
        draw_wall(&mut s);
        s.set_tool(Tool::Demolish);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert!(s.doc.walls.is_empty(), "partition is demolished");
        s.key("z", true, false);
        assert_eq!(s.doc.walls.len(), 1, "demolition is undoable");

        s.doc.walls[0].kind = WallKind::Bearing;
        s.set_tool(Tool::Demolish);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert_eq!(s.doc.walls.len(), 1, "bearing wall is protected");
        assert!(s.note.as_deref().unwrap_or("").contains("承重墙"));
    }

    #[test]
    fn architectural_scale_presets_are_real_ratios() {
        let mut s = empty();
        s.set_scale_denominator(60.0);
        assert!((s.scale_denominator() - 60.0).abs() < 0.2);
        s.set_scale_denominator(100.0);
        assert!((s.scale_denominator() - 100.0).abs() < 0.2);
    }

    #[test]
    fn quote_totals_include_five_percent_waste() {
        let mut s = empty();
        draw_rect(&mut s);
        let quote = s.quote_totals();
        assert!(quote.net_area > 0.0);
        assert!((quote.billed_area - quote.net_area * 1.05).abs() < 1e-6);
        assert!((quote.material_cost - quote.billed_area * 320.0).abs() < 1e-6);
        assert!((quote.waste_cost - quote.net_area * 0.05 * 320.0).abs() < 1e-6);
    }

    #[test]
    fn drag_is_a_single_undo_step() {
        let mut s = empty();
        draw_wall(&mut s);
        let before = s.doc.walls[0].start;
        s.set_tool(Tool::Select);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        s.pointer_move([440.0, 400.0]);
        s.pointer_up([440.0, 400.0], PointerButton::Left, false);
        assert_ne!(s.doc.walls[0].start, before, "drag should move the wall");
        assert_eq!(s.history.undo_len(), 2, "add wall + move");
        s.key("z", true, false);
        assert_eq!(s.doc.walls[0].start, before);
    }

    fn project(cam: &OrbitCamera, size: [f32; 2], p: Vec3) -> [f32; 2] {
        let clip = cam.view_proj(size[0] / size[1]) * Vec4::new(p.x, p.y, p.z, 1.0);
        let ndc = [clip.x / clip.w, clip.y / clip.w];
        [
            (ndc[0] * 0.5 + 0.5) * size[0],
            (0.5 - ndc[1] * 0.5) * size[1],
        ]
    }

    fn draw_rect(s: &mut SceneState) {
        s.set_tool(Tool::Wall);
        for (a, b) in [
            ([200.0, 400.0], [600.0, 400.0]),
            ([600.0, 400.0], [600.0, 100.0]),
            ([600.0, 100.0], [200.0, 100.0]),
            ([200.0, 100.0], [200.0, 400.0]),
        ] {
            s.pointer_down(a, PointerButton::Left, false);
            s.pointer_down(b, PointerButton::Left, false);
        }
    }

    #[test]
    fn opening_select_adjust_and_delete() {
        let mut s = empty();
        draw_wall(&mut s);
        s.set_tool(Tool::Door);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert_eq!(s.doc.openings.len(), 1);
        s.set_tool(Tool::Select);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert!(s.selected_opening.is_some(), "door picked from the plan");
        s.apply_action(InspectorAction::OpeningWidth(100));
        assert_eq!(s.doc.openings[0].width_mm, 1000);
        s.apply_action(InspectorAction::RemoveOpening);
        assert!(s.doc.openings.is_empty());
        s.key("z", true, false);
        assert_eq!(s.doc.openings.len(), 1);
        s.key("z", true, false);
        assert_eq!(s.doc.openings[0].width_mm, 900);
    }

    #[test]
    fn wall_properties_via_inspector() {
        let mut s = empty();
        draw_rect(&mut s);
        s.selection = vec![s.doc.walls[0].id];
        let thick = s.doc.walls[0].thickness_mm;
        s.apply_action(InspectorAction::WallThickness(20));
        assert_eq!(s.doc.walls[0].thickness_mm, thick + 20);
        s.apply_action(InspectorAction::CycleWallKind);
        assert_ne!(s.doc.walls[0].kind, WallKind::Partition);
        s.key("z", true, false);
        assert_eq!(s.doc.walls[0].kind, WallKind::Partition);
    }

    #[test]
    fn wall_length_and_kind_dropdown_are_exact_and_undoable() {
        let mut s = empty();
        draw_rect(&mut s);
        let id = s.doc.walls[0].id;
        s.selection = vec![id];
        let before = [s.doc.walls[0].start, s.doc.walls[0].end];
        s.apply_action(InspectorAction::WallSetLength(3000));
        let wall = s.doc.walls.iter().find(|w| w.id == id).unwrap();
        let len = ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
        assert!((len - 3000.0).abs() < 1.0);
        assert_eq!(wall.start, before[0]);
        assert_eq!(wall.kind, WallKind::Partition);

        s.apply_action(InspectorAction::SetWallKind(WallKind::Bearing));
        assert_eq!(s.doc.walls[0].kind, WallKind::Bearing);
        let view = s.inspector().expect("wall inspector");
        assert_eq!(view.wall_kind, Some(WallKind::Bearing));
        assert!(
            !view
                .actions
                .iter()
                .any(|(_, action)| matches!(action, InspectorAction::CycleWallKind))
        );

        s.key("z", true, false);
        assert_eq!(s.doc.walls[0].kind, WallKind::Partition);
        s.key("z", true, false);
        let wall = s.doc.walls.iter().find(|w| w.id == id).unwrap();
        assert_eq!([wall.start, wall.end], before);
    }

    #[test]
    fn furniture_position_is_editable_and_undoable() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([400.0, 300.0], PointerButton::Left, false);
        let before = s.doc.furniture[0].center;
        let view = s.inspector().expect("furniture inspector");
        assert!(
            view.fields
                .iter()
                .any(|field| field.edit == Some(InspectorEdit::FurnitureX))
        );
        assert!(
            view.fields
                .iter()
                .any(|field| field.edit == Some(InspectorEdit::FurnitureY))
        );
        s.apply_action(InspectorAction::FurnitureSetX(1200));
        s.apply_action(InspectorAction::FurnitureSetY(800));
        assert_eq!(s.doc.furniture[0].center, Pt::new(1200, 800));
        s.key("z", true, false);
        s.key("z", true, false);
        assert_eq!(s.doc.furniture[0].center, before);
    }

    #[test]
    fn furniture_resize_and_recolor_are_undoable() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([400.0, 300.0], PointerButton::Left, false);
        let w0 = s.doc.furniture[0].width_mm;
        s.key("]", false, false);
        assert_eq!(s.doc.furniture[0].width_mm, w0 + 100);
        s.key("[", false, false);
        assert_eq!(s.doc.furniture[0].width_mm, w0);
        let c0 = s.doc.furniture[0].color.clone();
        s.key("c", false, false);
        assert_ne!(s.doc.furniture[0].color, c0);
        s.key("z", true, false);
        assert_eq!(s.doc.furniture[0].color, c0);
    }

    #[test]
    fn arming_furniture_clears_wall_selection_and_highlights_nothing() {
        let mut s = empty();
        draw_rect(&mut s);
        s.selection = vec![s.doc.walls[0].id];
        s.begin_furniture_browse();
        assert_eq!(s.tool, Tool::Furniture);
        assert!(
            !s.furniture_armed,
            "opening the library does not arm a piece"
        );
        assert!(s.selection.is_empty(), "wall highlight is cleared");
        s.choose_catalog_item(0);
        assert!(s.furniture_armed);
        assert!(s.selection.is_empty());
        assert!(s.selected_furniture.is_none());
        assert!(s.selected_opening.is_none());
    }

    #[test]
    fn inspector_exact_furniture_size_and_fabric_are_undoable() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([400.0, 300.0], PointerButton::Left, false);
        let view = s.inspector().expect("furniture inspector");
        assert_eq!(view.kind, InspectorKind::Furniture);
        assert!(
            view.fields
                .iter()
                .any(|field| field.edit == Some(InspectorEdit::FurnitureWidth))
        );
        assert_eq!(view.swatches.len(), 5);
        let before = (s.doc.furniture[0].width_mm, s.doc.furniture[0].depth_mm);
        s.apply_action(InspectorAction::FurnitureSetWidth(1234));
        s.apply_action(InspectorAction::FurnitureSetDepth(789));
        assert_eq!(
            (s.doc.furniture[0].width_mm, s.doc.furniture[0].depth_mm),
            (1234, 789)
        );
        let color_before = s.doc.furniture[0].color.clone();
        s.apply_action(InspectorAction::FurnitureSetColor("#b8c3d2"));
        assert_eq!(s.doc.furniture[0].color, "#b8c3d2");
        s.key("z", true, false);
        assert_eq!(s.doc.furniture[0].color, color_before);
        s.key("z", true, false);
        s.key("z", true, false);
        assert_eq!(
            (s.doc.furniture[0].width_mm, s.doc.furniture[0].depth_mm),
            before
        );
    }

    #[test]
    fn wall_finish_is_inspector_editable_and_undoable() {
        let mut s = empty();
        draw_rect(&mut s);
        s.selection = vec![s.doc.walls[0].id];
        let view = s.inspector().expect("wall inspector");
        assert_eq!(view.wall_finish, Some(WallFinish::Paint));
        assert!(s.doc.walls[0].finish == WallFinish::Paint);
        s.apply_action(InspectorAction::SetWallFinish(WallFinish::WoodPanel));
        assert_eq!(s.doc.walls[0].finish, WallFinish::WoodPanel);
        assert!(
            s.mesh3d().iter().any(|v| v.pattern == 11.0),
            "wood finish reaches the 3D mesh"
        );
        s.key("z", true, false);
        assert_eq!(s.doc.walls[0].finish, WallFinish::Paint);
    }

    #[test]
    fn wall_finish_library_choice_is_undoable() {
        let mut s = empty();
        draw_rect(&mut s);
        s.selection = vec![s.doc.walls[0].id];
        let view = s.inspector().expect("wall inspector");
        assert_eq!(view.wall_finish_options.len(), 4);
        let tile_id = s
            .doc
            .materials
            .iter()
            .find(|m| m.surface == FinishSurface::Wall && m.kind == "tile")
            .map(|m| m.id)
            .expect("wall tile preset");
        s.apply_action(InspectorAction::SetWallFinishMaterial(tile_id));
        assert_eq!(s.doc.walls[0].finish_id, Some(tile_id));
        assert_eq!(s.doc.walls[0].finish, WallFinish::Tile);
        assert!(s.mesh3d().iter().any(|v| v.pattern == 6.0));
        s.key("z", true, false);
        assert_eq!(s.doc.walls[0].finish_id, None);
        assert_eq!(s.doc.walls[0].finish, WallFinish::Paint);
    }

    #[test]
    fn finish_can_be_applied_to_selection_and_floor() {
        let mut s = empty();
        draw_rect(&mut s);
        let tile_id = s
            .doc
            .materials
            .iter()
            .find(|m| m.surface == FinishSurface::Wall && m.kind == "tile")
            .map(|m| m.id)
            .expect("wall tile preset");

        let all: Vec<EntityId> = s.doc.walls.iter().map(|w| w.id).collect();
        s.selection = all.clone();
        s.apply_action(InspectorAction::ApplyWallFinishToSelection(tile_id));
        assert!(s.doc.walls.iter().all(|w| w.finish_id == Some(tile_id)));
        s.key("z", true, false);
        assert!(s.doc.walls.iter().all(|w| w.finish_id.is_none()));

        s.selection = vec![all[0]];
        s.apply_action(InspectorAction::SetWallFinishMaterial(tile_id));
        s.apply_action(InspectorAction::ApplyWallFinishToFloor);
        assert!(s.doc.walls.iter().all(|w| w.finish_id == Some(tile_id)));
        s.key("z", true, false);
        assert_eq!(s.doc.walls[0].finish_id, Some(tile_id));
        assert!(s.doc.walls[1..].iter().all(|w| w.finish_id.is_none()));
    }

    #[test]
    fn room_ceiling_finish_is_undoable_and_quoted() {
        let mut s = empty();
        draw_rect(&mut s);
        s.selected_room = Some(s.doc.rooms[0].id);
        let before = s.doc.rooms[0].ceiling_material_id;
        let view = s.inspector().expect("room inspector");
        assert_eq!(view.ceiling_options.len(), 2);
        let wood_id = s
            .doc
            .materials
            .iter()
            .find(|m| m.surface == FinishSurface::Ceiling && m.kind == "wood_panel")
            .map(|m| m.id)
            .expect("ceiling wood preset");
        s.apply_action(InspectorAction::SetCeilingMaterial(wood_id));
        assert_eq!(s.doc.rooms[0].ceiling_material_id, Some(wood_id));
        let quote = crate::quote::Quote::compute(&s.doc);
        assert!(quote.ceiling_material > 0.0);
        assert!(quote.finish_rows.iter().any(|r| r.surface == "顶面"));
        s.key("z", true, false);
        assert_eq!(s.doc.rooms[0].ceiling_material_id, before);
    }

    #[test]
    fn inspector_wall_exact_size_and_opening_exact_size() {
        let mut s = empty();
        draw_rect(&mut s);
        s.selection = vec![s.doc.walls[0].id];
        let before = (s.doc.walls[0].thickness_mm, s.doc.walls[0].height_mm);
        s.apply_action(InspectorAction::WallSetThickness(240));
        s.apply_action(InspectorAction::WallSetHeight(2800));
        assert_eq!(
            (s.doc.walls[0].thickness_mm, s.doc.walls[0].height_mm),
            (240, 2800)
        );
        let view = s.inspector().expect("wall inspector");
        assert!(
            view.fields
                .iter()
                .any(|field| field.edit == Some(InspectorEdit::WallThickness))
        );
        s.key("z", true, false);
        s.key("z", true, false);
        assert_eq!(
            (s.doc.walls[0].thickness_mm, s.doc.walls[0].height_mm),
            before
        );

        s.set_tool(Tool::Door);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert_eq!(s.doc.openings.len(), 1);
        s.set_tool(Tool::Select);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert!(s.selected_opening.is_some());
        let opening_before = (
            s.doc.openings[0].width_mm,
            s.doc.openings[0].height_mm,
            s.doc.openings[0].sill_mm,
        );
        s.apply_action(InspectorAction::OpeningSetWidth(1100));
        s.apply_action(InspectorAction::OpeningSetHeight(2300));
        s.apply_action(InspectorAction::OpeningSetSill(80));
        assert_eq!(
            (
                s.doc.openings[0].width_mm,
                s.doc.openings[0].height_mm,
                s.doc.openings[0].sill_mm
            ),
            (1100, 2300, 80)
        );
        s.key("z", true, false);
        s.key("z", true, false);
        s.key("z", true, false);
        assert_eq!(
            (
                s.doc.openings[0].width_mm,
                s.doc.openings[0].height_mm,
                s.doc.openings[0].sill_mm
            ),
            opening_before
        );
    }

    #[test]
    fn furniture_layer_order_and_clear_are_undoable() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        let first = s.doc.furniture[0].id;
        s.set_tool(Tool::Furniture);
        s.pointer_down([420.0, 260.0], PointerButton::Left, false);
        let second = s.doc.furniture[1].id;
        assert_eq!(s.doc.furniture[0].id, first);

        s.selected_furniture = Some(second);
        s.apply_action(InspectorAction::FurnitureSendBack);
        assert_eq!(s.doc.furniture[0].id, second, "second piece goes to back");
        s.key("z", true, false);
        assert_eq!(s.doc.furniture[0].id, first, "undo restores order");

        s.selected_furniture = Some(first);
        s.apply_action(InspectorAction::FurnitureBringFront);
        assert_eq!(s.doc.furniture[1].id, first, "first piece goes to front");
        s.key("z", true, false);
        assert_eq!(s.doc.furniture[1].id, second);

        s.clear_furniture();
        assert!(s.doc.furniture.is_empty());
        assert_eq!(s.selected_furniture, None);
        s.key("z", true, false);
        assert_eq!(s.doc.furniture.len(), 2);
        assert_eq!(s.doc.furniture[0].id, first);
        assert_eq!(s.doc.furniture[1].id, second);
    }

    #[test]
    fn deleting_furniture_undo_restores_its_layer() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        let first = s.doc.furniture[0].id;
        s.set_tool(Tool::Furniture);
        s.pointer_down([420.0, 260.0], PointerButton::Left, false);
        let second = s.doc.furniture[1].id;
        s.selected_furniture = Some(first);
        s.key("delete", false, false);
        assert_eq!(s.doc.furniture.len(), 1);
        assert_eq!(s.doc.furniture[0].id, second);
        s.key("z", true, false);
        assert_eq!(
            s.doc.furniture[0].id, first,
            "undo puts it back at the bottom"
        );
        assert_eq!(s.doc.furniture[1].id, second);
    }

    #[test]
    fn shift_click_builds_a_furniture_selection() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        s.set_tool(Tool::Furniture);
        s.pointer_down([420.0, 260.0], PointerButton::Left, false);
        s.set_tool(Tool::Select);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        s.pointer_down([420.0, 260.0], PointerButton::Left, true);
        assert_eq!(s.furniture_selection.len(), 2);
        assert!(s.furniture_selection.contains(&s.doc.furniture[0].id));
        assert!(s.furniture_selection.contains(&s.doc.furniture[1].id));
    }

    #[test]
    fn furniture_drag_keeps_the_grab_point_and_ignores_jitter() {
        let mut s = empty();
        s.place_furniture_at(0, [500.0, 400.0]);
        let before = s.doc.furniture[0].center;
        // Grab ten pixels left of the centre; a one-pixel wobble follows the
        // pointer live but is put back on release, so a click never moves it.
        s.pointer_down([490.0, 395.0], PointerButton::Left, false);
        // A viewport change mid-drag (focus transition, titlebar) used to shift
        // the world origin and teleport the piece. The drag delta must not care.
        s.set_view_size([1000.0, 700.0]);
        s.pointer_move([491.0, 395.0]);
        s.pointer_up([491.0, 395.0], PointerButton::Left, false);
        assert_eq!(s.doc.furniture[0].center, before, "jitter is ignored");
        let grab = s.cam.world_to_screen(before);
        let factor = s.cam.scale;
        s.pointer_down([grab[0] - 10.0, grab[1] - 5.0], PointerButton::Left, false);
        s.pointer_move([grab[0] + 10.0, grab[1] - 5.0]);
        let expected = Pt::new(before.x + (20.0 / factor).round() as i32, before.y);
        assert_eq!(
            s.doc.furniture[0].center, expected,
            "the same point of the piece stays under the pointer"
        );
    }

    #[test]
    fn furniture_cannot_be_dragged_on_top_of_another() {
        let mut s = empty();
        s.place_furniture_at(0, [500.0, 400.0]);
        s.place_furniture_at(0, [800.0, 400.0]);
        s.set_tool(Tool::Select);
        s.pointer_down([800.0, 400.0], PointerButton::Left, false);
        s.pointer_move([650.0, 400.0]);
        s.pointer_up([650.0, 400.0], PointerButton::Left, false);
        let gap = (s.doc.furniture[1].center.x - s.doc.furniture[0].center.x).abs();
        assert!(
            gap >= 2195,
            "the second sofa stops flush beside the first, gap {gap}"
        );
        assert_eq!(s.note.as_deref(), Some("碰撞阻挡 · 家具不能穿墙或重叠"));
    }

    #[test]
    fn furniture_cannot_be_dragged_through_a_wall() {
        let mut s = empty();
        draw_wall(&mut s);
        s.place_furniture_at(0, [500.0, 350.0]);
        let before = s.doc.furniture[0].center;
        s.set_tool(Tool::Select);
        s.pointer_down([500.0, 350.0], PointerButton::Left, false);
        s.pointer_move([500.0, 450.0]);
        s.pointer_up([500.0, 450.0], PointerButton::Left, false);
        assert_eq!(
            s.doc.furniture[0].center, before,
            "a solid wall blocks the whole sweep"
        );
        assert_eq!(s.note.as_deref(), Some("碰撞阻挡 · 家具不能穿墙或重叠"));
    }

    #[test]
    fn a_wall_blocks_placement_and_nudges_it_clear() {
        let mut s = empty();
        draw_wall(&mut s);
        // Dropping exactly on the wall cannot add a piece inside the wall.
        s.place_furniture_at(0, [500.0, 400.0]);
        let item = &s.doc.furniture[0];
        assert!(
            furniture_blocking_wall(item, &s.doc.walls).is_none(),
            "placement nudges to a clear spot instead of cutting the wall"
        );
    }

    #[test]
    fn group_drag_moves_every_selected_piece_and_undoes_once() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        s.set_tool(Tool::Furniture);
        s.pointer_down([520.0, 320.0], PointerButton::Left, false);
        let first = s.doc.furniture[0].id;
        let second = s.doc.furniture[1].id;
        let before0 = s.doc.furniture[0].center;
        let before1 = s.doc.furniture[1].center;
        s.furniture_selection = vec![first, second];
        s.selected_furniture = Some(first);
        s.set_tool(Tool::Select);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        s.pointer_move([400.0, 220.0]);
        s.pointer_up([400.0, 220.0], PointerButton::Left, false);
        let after0 = s.doc.furniture[0].center;
        let after1 = s.doc.furniture[1].center;
        assert_eq!(after0.x - before0.x, after1.x - before1.x);
        assert_eq!(after0.y - before0.y, after1.y - before1.y);
        s.key("z", true, false);
        assert_eq!(s.doc.furniture[0].center, before0);
        assert_eq!(s.doc.furniture[1].center, before1);
    }

    #[test]
    fn furniture_alignment_moves_the_group_and_is_undoable() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        s.set_tool(Tool::Furniture);
        s.pointer_down([420.0, 260.0], PointerButton::Left, false);
        let first = s.doc.furniture[0].id;
        let second = s.doc.furniture[1].id;
        for item in &mut s.doc.furniture {
            item.width_mm = 800;
            item.depth_mm = 600;
            item.rotation_deg = 0.0;
        }
        s.doc.furniture[0].center = Pt::new(1000, 1000);
        s.doc.furniture[1].center = Pt::new(3000, 2000);
        s.furniture_selection = vec![first, second];
        s.selected_furniture = Some(first);
        s.apply_action(InspectorAction::FurnitureAlignLeft);
        assert_eq!(s.doc.furniture[1].center.x, 1000);
        s.key("z", true, false);
        assert_eq!(s.doc.furniture[1].center.x, 3000);
    }

    #[test]
    fn door_hinge_side_is_undoable() {
        let mut s = empty();
        draw_wall(&mut s);
        s.set_tool(Tool::Door);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        s.set_tool(Tool::Select);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert!(!s.doc.openings[0].hinge_right);
        s.apply_action(InspectorAction::OpeningFlipHinge);
        assert!(s.doc.openings[0].hinge_right);
        s.key("z", true, false);
        assert!(!s.doc.openings[0].hinge_right);
    }

    #[test]
    fn project_info_updates_header_and_persists() {
        let mut s = empty();
        s.update_project_info("新名字", "客户甲", "上海", "备注")
            .unwrap();
        let (name, client, address, note) = s.project_info();
        assert_eq!(name, "新名字");
        assert_eq!(client, "客户甲");
        assert_eq!(address, "上海");
        assert_eq!(note, "备注");
        let text = std::fs::read_to_string(&s.saved_path).unwrap();
        assert!(text.contains("新名字"));
        assert!(text.contains("客户甲"));
    }

    #[test]
    fn door_swing_direction_is_undoable() {
        let mut s = empty();
        draw_wall(&mut s);
        s.set_tool(Tool::Door);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert_eq!(s.doc.openings[0].swing_deg, 90.0);
        s.set_tool(Tool::Select);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        s.apply_action(InspectorAction::OpeningFlipSwing);
        assert_eq!(s.doc.openings[0].swing_deg, -90.0);
        s.key("z", true, false);
        assert_eq!(s.doc.openings[0].swing_deg, 90.0);
    }

    #[test]
    fn layers_hide_furniture_grid_and_room_labels() {
        let mut s = empty();
        draw_rect(&mut s);
        s.set_tool(Tool::Furniture);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        let full = s.build_batch().vertices.len();
        assert!(s.labels().iter().any(|l| l.text.contains("m²")));
        s.toggle_layer(Layer::Furniture);
        s.toggle_layer(Layer::Grid);
        s.toggle_layer(Layer::RoomNames);
        let lean = s.build_batch().vertices.len();
        assert!(lean < full, "{lean} !< {full}");
        assert!(!s.layer_on(Layer::Furniture));
        assert!(!s.labels().iter().any(|l| l.text.contains("m²")));
    }

    #[test]
    fn bearing_wall_layer_falls_back_to_partition_colour() {
        let mut s = empty();
        let wall = Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(600, 0),
            120,
            fp_model::WallKind::Bearing,
        );
        s.doc.walls.push(wall);
        let on = s.build_batch();
        s.toggle_layer(Layer::Bearing);
        let off = s.build_batch();
        let bearing = [0.30, 0.30, 0.33, 1.0];
        let partition = [0.62, 0.61, 0.60, 1.0];
        assert!(on.vertices.iter().any(|v| v.color == bearing));
        assert!(off.vertices.iter().all(|v| v.color != bearing));
        assert!(off.vertices.iter().any(|v| v.color == partition));
    }

    #[test]
    fn rotation_handle_spins_the_selected_piece() {
        let mut s = empty();
        s.doc.furniture.push(Furniture {
            id: EntityId(7),
            asset_id: "sofa".into(),
            name: "沙发".into(),
            center: Pt::new(0, 0),
            width_mm: 2000,
            depth_mm: 900,
            rotation_deg: 0.0,
            color: String::new(),
            room_id: None,
            price: None,
            floor: 0,
        });
        s.selected_furniture = Some(EntityId(7));
        s.set_tool(Tool::Select);
        let item = s.doc.furniture[0].clone();
        let handle = s.furniture_handle(&item);
        s.pointer_down(handle, PointerButton::Left, false);
        let center = s.cam.world_to_screen(item.center);
        s.pointer_move([center[0] + 240.0, center[1]]);
        s.pointer_up([center[0] + 240.0, center[1]], PointerButton::Left, false);
        assert!(
            (s.doc.furniture[0].rotation_deg - 270.0).abs() < 1.0,
            "dragging right of the centre turns it a quarter turn: {}",
            s.doc.furniture[0].rotation_deg
        );
        s.history.undo(&mut s.doc);
        assert!(
            s.doc.furniture[0].rotation_deg.abs() < 1e-9,
            "undo restores the angle"
        );
    }

    #[test]
    fn all_category_keeps_the_current_pick_and_tabs_forward() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.set_catalog_filter(FurnitureCategory::Living);
        let first = s.catalog_index;
        s.set_catalog_filter(FurnitureCategory::All);
        assert_eq!(s.catalog_index, first, "all tab keeps the current item");
        s.key("tab", false, false);
        assert_eq!(
            s.catalog_index,
            (first + 1) % fp_model::catalog::CATALOG.len(),
            "tab walks the whole catalogue under All"
        );
    }

    #[test]
    fn walk_view_opens_the_nearest_door_then_closes_it() {
        let mut s = empty();
        s.doc.walls.push(Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            100,
            WallKind::Partition,
        ));
        s.doc.openings.push(Opening {
            id: EntityId(2),
            wall_id: EntityId(1),
            kind: OpeningKind::Door,
            offset_mm: 1500,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 90.0,
            hinge_right: false,
            price: None,
        });
        s.set_view(ViewMode::Walk);
        s.walk.pos = glam::Vec3::new(1.95, 1.6, 0.8);
        s.key("e", false, false);
        assert_eq!(s.doors_open.len(), 1, "door next to the player opens");
        s.key("e", false, false);
        assert!(s.doors_open.is_empty(), "second press closes it");

        s.walk.pos = glam::Vec3::new(20.0, 1.6, 20.0);
        s.key("e", false, false);
        assert!(s.doors_open.is_empty(), "nothing in reach stays closed");
    }

    #[test]
    fn clicking_a_visible_door_in_walk_toggles_it() {
        let mut s = empty();
        s.doc.walls.push(Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            100,
            WallKind::Partition,
        ));
        let opening = Opening {
            id: EntityId(2),
            wall_id: EntityId(1),
            kind: OpeningKind::Door,
            offset_mm: 1500,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 90.0,
            hinge_right: false,
            price: None,
        };
        s.doc.openings.push(opening.clone());
        s.set_view(ViewMode::Walk);
        s.walk.pos = glam::Vec3::new(1.95, 1.6, -1.0);
        s.walk.yaw = 0.0;
        s.walk.pitch = 0.0;
        let wall = s.doc.walls[0].clone();
        let world = s.door_midpoint(&opening, &wall).expect("door midpoint");
        let clip = s.walk.view_proj(1000.0 / 800.0) * world.extend(1.0);
        let ndc = [clip.x / clip.w, clip.y / clip.w];
        let screen = [
            (ndc[0] * 0.5 + 0.5) * 1000.0,
            (1.0 - (ndc[1] * 0.5 + 0.5)) * 800.0,
        ];
        assert!(
            s.door_at_screen(screen).is_some(),
            "door projects onto the click point"
        );
        s.pointer_down(screen, PointerButton::Left, false);
        assert_eq!(s.doors_open.len(), 1);
    }

    #[test]
    fn cutaway_hides_the_door_leaves() {
        let mut s = empty();
        s.doc.walls.push(Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            100,
            WallKind::Partition,
        ));
        s.doc.openings.push(Opening {
            id: EntityId(2),
            wall_id: EntityId(1),
            kind: OpeningKind::Door,
            offset_mm: 1500,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 90.0,
            hinge_right: false,
            price: None,
        });
        s.cutaway = false;
        let full = s.mesh3d().iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
        s.cutaway = true;
        let cut = s.mesh3d().iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
        assert!(full > 2.0, "full-height view keeps the 2.1 m door: {full}");
        assert!(cut <= 1.351, "cutaway caps everything: {cut}");
    }

    #[test]
    fn status_speaks_the_selected_language() {
        let mut s = empty();
        s.lang = crate::i18n::Lang::Zh;
        assert!(s.status().contains("面墙"), "{}", s.status());
        s.lang = crate::i18n::Lang::En;
        assert!(s.status().contains("walls"), "{}", s.status());
    }

    #[test]
    fn cutaway_caps_walls_and_walk_view_keeps_full_height() {
        let mut s = empty();
        draw_rect(&mut s);
        s.cutaway = false;
        let full = s.mesh3d().iter().map(|v| v.pos[1]).fold(0.0, f32::max);
        assert!(full > 2.0, "full walls reach 2.8m, got {full}");
        s.cutaway = true;
        let cut = s.mesh3d().iter().map(|v| v.pos[1]).fold(0.0, f32::max);
        assert!(cut <= 1.351, "cutaway caps at 1.35m, got {cut}");
        let walk = s.mesh3d_walk().iter().map(|v| v.pos[1]).fold(0.0, f32::max);
        assert!(walk > 2.0, "walk view stays full height, got {walk}");
    }

    #[test]
    fn toggling_cutaway_refreshes_the_mesh_and_epoch_without_moving_the_camera() {
        let mut s = empty();
        draw_rect(&mut s);
        s.set_cutaway(false);
        let (epoch_full, full) = s.mesh3d_parts(false);
        let full_h = full.vertices.iter().map(|v| v.pos[1]).fold(0.0, f32::max);
        assert!(full_h > 2.0);
        s.set_cutaway(true);
        let (epoch_cut, cut) = s.mesh3d_parts(false);
        let cut_h = cut.vertices.iter().map(|v| v.pos[1]).fold(0.0, f32::max);
        assert!(cut_h <= 1.351, "cut mesh rebuilt immediately: {cut_h}");
        assert_ne!(epoch_full, epoch_cut, "the renderer must see a new epoch");
    }

    #[test]
    fn space_left_drag_pans_without_starting_a_selection() {
        let mut s = empty();
        let before = s.cam.center;
        s.set_space_down(true);
        s.pointer_down([400.0, 300.0], PointerButton::Left, false);
        assert!(s.pan.is_some(), "space + left starts a pan");
        assert!(
            s.box_select.is_none(),
            "space + left must not marquee-select"
        );
        s.pointer_move([470.0, 350.0]);
        assert!(
            s.cam.center[0] < before[0],
            "dragging right pans the plan right"
        );
        assert!(
            s.cam.center[1] > before[1],
            "dragging down pans the plan down"
        );
        s.pointer_up([470.0, 350.0], PointerButton::Left, false);
        assert!(s.pan.is_none());
        s.set_space_down(false);
    }

    #[test]
    fn sun_sweep_and_night_change_the_lighting() {
        let mut s = empty();
        let noon = s.look3d();
        assert!(
            noon.tint[0] >= noon.tint[2],
            "daylight is warm: {:?}",
            noon.tint
        );
        s.sun = 0.0;
        let morning = s.look3d();
        assert!(morning.light[0] < -0.5, "morning sun leans one way");
        assert!(
            morning.light[1].abs() < noon.light[1].abs() * 0.2,
            "low morning sun casts much longer shadows"
        );
        s.sun = 1.0;
        let evening = s.look3d();
        assert!(evening.light[0] > 0.5, "evening sun leans the other way");
        s.night = true;
        let night = s.look3d();
        assert!(night.ambient < noon.ambient);
        assert!(night.tint[2] > night.tint[0], "night is cold/blue");
        assert!(night.clear[0] < noon.clear[0], "night background is darker");
    }

    #[test]
    fn dragging_furniture_in_3d_moves_it_on_the_floor() {
        let mut s = empty();
        draw_rect(&mut s);
        s.set_tool(Tool::Furniture);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        let id = s.selected_furniture.expect("placed");
        let before = s.doc.furniture.iter().find(|f| f.id == id).unwrap().center;

        s.view = ViewMode::Three;
        s.view_size = [1200.0, 800.0];
        s.cam3.target = Vec3::new(before.x as f32 * 0.001, 0.4, before.y as f32 * 0.001);
        s.cam3.distance = 5.5;
        s.cam3.pitch = 1.3;
        s.cam3.yaw = 0.3;

        let screen = project(
            &s.cam3,
            s.view_size,
            Vec3::new(before.x as f32 * 0.001, 0.3, before.y as f32 * 0.001),
        );
        s.pointer_down(screen, PointerButton::Left, false);
        assert!(s.furniture_drag_3d.is_some(), "press on furniture grabs it");

        s.pointer_move([screen[0] + 60.0, screen[1] + 10.0]);
        let after = s.doc.furniture.iter().find(|f| f.id == id).unwrap().center;
        assert_ne!(after, before, "drag moves the centre");

        s.pointer_up(
            [screen[0] + 60.0, screen[1] + 10.0],
            PointerButton::Left,
            false,
        );
        assert!(s.furniture_drag_3d.is_none());
        assert!(s.history.undo_len() >= 2, "the move is one undo step");
        s.history.undo(&mut s.doc);
        assert_eq!(
            s.doc.furniture.iter().find(|f| f.id == id).unwrap().center,
            before
        );
    }

    #[test]
    fn fine_rotation_steps_by_fifteen_degrees() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        let id = s.selected_furniture.expect("placed");
        s.apply_action(InspectorAction::FurnitureRotateFine(-15));
        let item = s.doc.furniture.iter().find(|f| f.id == id).unwrap();
        assert_eq!(item.rotation_deg, 345.0);
        for _ in 0..2 {
            s.apply_action(InspectorAction::FurnitureRotateFine(15));
        }
        assert_eq!(s.doc.furniture[0].rotation_deg, 15.0);
        assert_eq!(s.history.undo_len(), 4, "place + three nudges are undoable");
    }

    #[test]
    fn shortcuts_duplicate_zoom_toggle_and_fit() {
        let mut s = empty();
        draw_rect(&mut s);
        s.set_tool(Tool::Furniture);
        s.pointer_down([320.0, 220.0], PointerButton::Left, false);
        assert_eq!(s.doc.furniture.len(), 1);
        s.key("d", true, false);
        assert_eq!(s.doc.furniture.len(), 2, "cmd+D duplicates");
        let before = s.cam.scale;
        s.key("+", false, false);
        assert!(s.cam.scale > before, "plus zooms in");
        s.key("-", false, false);
        assert!(s.cam.scale <= before * 1.01);
        s.key("t", false, false);
        assert_eq!(s.view, ViewMode::Three);
        s.key("t", false, false);
        assert_eq!(s.view, ViewMode::Plan);
        s.key("v", false, false);
        assert_eq!(s.tool, Tool::Select);
        s.key("r", false, true);
        let selected = s.selected_furniture.expect("duplicate stays selected");
        let item = s.doc.furniture.iter().find(|f| f.id == selected).unwrap();
        assert_eq!(item.rotation_deg, 270.0, "shift+R rotates -90°");
        s.key("f", false, false);
        assert_eq!(s.note.as_deref(), Some("适应窗口"));
        s.key("g", false, false);
        assert_eq!(s.view, ViewMode::Walk);
        s.key("g", false, false);
        assert_eq!(s.view, ViewMode::Three);
    }

    #[test]
    fn presets_and_room_fly_move_the_orbit_camera() {
        let mut s = empty();
        draw_rect(&mut s);
        s.preset3d(Preset3d::Top);
        assert_eq!(s.view, ViewMode::Three);
        assert!(s.cam3.pitch > 1.3, "top view looks almost straight down");
        s.preset3d(Preset3d::Oblique);
        assert!(s.cam3.pitch < 0.5, "oblique view sits low");
        let (min, max) = plan_bounds(&s.doc);
        let centre = Vec3::new(
            (min.x + max.x) as f32 * 0.0005,
            0.0,
            (min.y + max.y) as f32 * 0.0005,
        );
        let room = s.room_list()[0].0;
        s.fly_to_room(room);
        assert_eq!(s.view, ViewMode::Three);
        assert_eq!(s.selected_room, Some(room));
        assert!((s.cam3.target - centre).length() < 1.0);
    }

    #[test]
    fn measure_two_points_and_escape_clears() {
        let mut s = empty();
        s.set_tool(Tool::Measure);
        s.pointer_down([200.0, 200.0], PointerButton::Left, false);
        let from = s.measure_from.expect("first click sets the start");
        s.pointer_down([420.0, 260.0], PointerButton::Left, false);
        assert_eq!(s.measurements.len(), 1);
        assert_eq!(
            s.doc.measures.len(),
            1,
            "measurement is part of the document"
        );
        assert_eq!(s.measurements[0].from, from);
        assert!(s.measurements[0].length_mm() >= 50.0);
        assert_eq!(s.measure_from, None);
        s.key("escape", false, false);
        assert!(s.measurements.is_empty());
        assert_eq!(s.tool, Tool::Select);
    }

    #[test]
    fn measurements_persist_and_undo() {
        let mut s = empty();
        s.set_tool(Tool::Measure);
        s.pointer_down([200.0, 200.0], PointerButton::Left, false);
        s.pointer_down([420.0, 260.0], PointerButton::Left, false);
        assert_eq!(s.doc.measures.len(), 1);

        let json = s.doc.to_json().expect("serialize");
        let doc = Document::from_json(&json).expect("parse");
        let loaded = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/x.json"));
        assert_eq!(loaded.measurements.len(), 1, "measure survives save/load");
        assert_eq!(loaded.measurements[0], s.measurements[0]);

        s.history.undo(&mut s.doc);
        s.sync_measurements();
        assert!(s.measurements.is_empty(), "undo removes the measurement");
        s.history.redo(&mut s.doc);
        s.sync_measurements();
        assert_eq!(s.measurements.len(), 1, "redo restores it");

        s.clear_measurements();
        assert!(s.measurements.is_empty());
        assert!(s.doc.measures.is_empty());
        s.history.undo(&mut s.doc);
        s.sync_measurements();
        assert_eq!(s.measurements.len(), 1, "clearing is undoable");
    }

    #[test]
    fn measure_snaps_to_wall_endpoint() {
        let mut s = empty();
        draw_rect(&mut s);
        let corner = s.doc.walls[0].start;
        let snapped = s.snap_measure(Pt::new(corner.x + 120, corner.y - 80));
        assert_eq!(snapped, corner);
        let far = Pt::new(corner.x + 3000, corner.y + 3000);
        assert!((s.snap_measure(far).x - far.x).abs() <= 10);
    }

    #[test]
    fn m_measures_and_shift_locks_axis() {
        let mut s = empty();
        s.key("m", false, false);
        assert_eq!(s.tool, Tool::Measure);
        s.pointer_down([200.0, 220.0], PointerButton::Left, false);
        let from = s.measure_from.expect("start");
        s.pointer_down([500.0, 260.0], PointerButton::Left, true);
        let m = s.measurements[0];
        assert_eq!(m.to.y, from.y, "shift keeps the run horizontal");
    }

    #[test]
    fn labels_show_wall_lengths_and_rooms() {
        let mut s = empty();
        draw_rect(&mut s);
        let labels = s.labels();
        assert_eq!(labels.len(), 5, "4 walls + 1 room");
        assert!(labels.iter().any(|l| l.text.starts_with("房间")));
        assert!(labels.iter().any(|l| l.text.contains("m²")));
        s.doc.settings.show_dimensions = false;
        let labels = s.labels();
        assert!(
            labels.iter().all(|l| l.text.contains("m²")),
            "only room labels"
        );
    }

    #[test]
    fn place_furniture_then_rotate_move_delete() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([400.0, 300.0], PointerButton::Left, false);
        assert_eq!(s.doc.furniture.len(), 1);
        assert_eq!(s.doc.furniture[0].name, CATALOG[0].name);
        let id = s.doc.furniture[0].id;
        assert_eq!(s.selected_furniture, Some(id));
        assert_eq!(s.tool, Tool::Select);

        s.key("r", false, false);
        assert_eq!(s.doc.furniture[0].rotation_deg, 90.0);
        s.key("z", true, false);
        assert_eq!(s.doc.furniture[0].rotation_deg, 0.0);

        s.pointer_down([400.0, 300.0], PointerButton::Left, false);
        s.pointer_move([460.0, 300.0]);
        s.pointer_up([460.0, 300.0], PointerButton::Left, false);
        assert_ne!(s.doc.furniture[0].center, Pt::new(0, 0));

        s.key("delete", false, false);
        assert!(s.doc.furniture.is_empty());
        s.key("z", true, false);
        assert_eq!(s.doc.furniture.len(), 1);
    }

    #[test]
    fn room_selection_and_material_cycle() {
        let mut s = empty();
        // Isolate this test from the standard palette so the ids below are the
        // only choices; this also exercises the unassigned initial state.
        s.doc.materials.clear();
        draw_rect(&mut s);
        assert_eq!(s.doc.rooms.len(), 1);
        s.doc.materials.push(Material {
            id: EntityId(2001),
            name: "橡木".into(),
            price_per_m2: 0.0,
            kind: "wood".into(),
            color: "#c49a6c".into(),
            surface: fp_model::FinishSurface::Floor,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
        });
        s.doc.materials.push(Material {
            id: EntityId(2002),
            name: "瓷砖".into(),
            price_per_m2: 0.0,
            kind: "tile".into(),
            color: "#a8b6c2".into(),
            surface: fp_model::FinishSurface::Floor,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
        });
        s.set_tool(Tool::Select);
        s.pointer_down([400.0, 250.0], PointerButton::Left, false);
        let room_id = s.doc.rooms[0].id;
        assert_eq!(s.selected_room, Some(room_id));
        s.key("m", false, true);
        assert_eq!(s.doc.rooms[0].material_id, Some(EntityId(2001)));
        s.key("m", false, true);
        assert_eq!(s.doc.rooms[0].material_id, Some(EntityId(2002)));
        s.key("z", true, false);
        assert_eq!(s.doc.rooms[0].material_id, Some(EntityId(2001)));
    }

    #[test]
    fn material_library_add_edit_delete_is_undoable() {
        let mut s = empty();
        s.doc.materials.clear();
        let id = s.add_material();
        let keep = s.add_material();
        assert_eq!(s.doc.materials.len(), 2);
        assert_eq!(s.doc.materials[0].name, "新材料");
        assert_eq!(s.material_usage(id), 0);
        let before = s.doc.materials[0].clone();
        let after = Material {
            id,
            name: "赤陶砖".into(),
            price_per_m2: 275.0,
            kind: "tile".into(),
            color: "#b4552d".into(),
            surface: fp_model::FinishSurface::Floor,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
        };
        s.update_material(after.clone());
        assert_eq!(s.doc.materials[0], after);
        s.remove_material(id);
        assert_eq!(s.doc.materials.len(), 1, "unused material can go");
        assert_eq!(s.doc.materials[0].id, keep);
        s.history.undo(&mut s.doc);
        assert_eq!(s.doc.materials.len(), 2);
        assert_eq!(s.doc.materials[0], after);
        s.history.undo(&mut s.doc);
        assert_eq!(s.doc.materials[0], before);
        s.history.undo(&mut s.doc);
        assert_eq!(s.doc.materials.len(), 1);
        s.history.undo(&mut s.doc);
        assert!(s.doc.materials.is_empty());
    }

    #[test]
    fn used_material_cannot_be_deleted() {
        let mut s = empty();
        s.doc.materials.clear();
        let id = s.add_material();
        s.doc.rooms.push(Room {
            id: EntityId(3001),
            polygon: vec![
                Pt::new(0, 0),
                Pt::new(2000, 0),
                Pt::new(2000, 2000),
                Pt::new(0, 2000),
            ],
            name: "测试房".into(),
            material_id: Some(id),
            counted: true,
            floor: 0,
            ceiling_material_id: None,
        });
        s.remove_material(id);
        assert_eq!(s.doc.materials.len(), 1);
        assert!(s.note.as_deref().unwrap_or("").contains("使用"));
    }

    #[test]
    fn rooms_recompute_after_walls() {
        let mut s = empty();
        s.set_tool(Tool::Wall);
        let corners = [
            ([200.0, 400.0], [600.0, 400.0]),
            ([600.0, 400.0], [600.0, 100.0]),
            ([600.0, 100.0], [200.0, 100.0]),
            ([200.0, 100.0], [200.0, 400.0]),
        ];
        for (a, b) in corners {
            s.pointer_down(a, PointerButton::Left, false);
            s.pointer_down(b, PointerButton::Left, false);
        }
        assert_eq!(s.doc.walls.len(), 4);
        assert_eq!(s.doc.rooms.len(), 1);
        assert_eq!(s.doc.rooms[0].polygon.len(), 4);
        s.key("z", true, false);
        assert_eq!(s.doc.walls.len(), 3);
        assert_eq!(s.doc.rooms.len(), 0, "opening the rectangle drops the room");
        s.key("z", true, true);
        assert_eq!(s.doc.rooms.len(), 1);
    }

    #[test]
    fn place_door_and_undo() {
        let mut s = empty();
        draw_wall(&mut s);
        s.set_tool(Tool::Door);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert_eq!(s.doc.openings.len(), 1);
        assert_eq!(s.doc.openings[0].width_mm, 900);
        s.key("z", true, false);
        assert_eq!(s.doc.openings.len(), 0);
    }

    #[test]
    fn opening_does_not_land_twice_on_the_same_span() {
        let mut s = empty();
        draw_wall(&mut s);
        s.set_tool(Tool::Window);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        s.pointer_down([400.0, 400.0], PointerButton::Left, false);
        assert_eq!(s.doc.openings.len(), 1, "overlapping window rejected");
    }

    #[test]
    fn box_select_picks_walls_inside() {
        let mut s = empty();
        draw_wall(&mut s);
        s.set_tool(Tool::Select);
        s.pointer_down([150.0, 300.0], PointerButton::Left, false);
        s.pointer_move([650.0, 500.0]);
        s.pointer_up([650.0, 500.0], PointerButton::Left, false);
        assert_eq!(s.selection.len(), 1);
    }
    #[test]
    fn walk_spawns_inside_the_largest_room() {
        let mut s = room_state();
        s.set_view(ViewMode::Walk);
        assert!(s.stand_ok(s.walk.pos.x, s.walk.pos.z));
        assert!((s.walk.pos.y - EYE_HEIGHT).abs() < 1e-5);
    }

    #[test]
    fn walk_moves_forward_and_stays_inside() {
        let mut s = room_state();
        s.set_view(ViewMode::Walk);
        let start = s.walk.pos;
        s.key("w", false, false);
        for _ in 0..120 {
            s.walk_step(1.0 / 60.0);
        }
        let moved = (s.walk.pos - start).length();
        assert!(moved > 0.5, "moved {moved}");
        s.key_up("w", false);
        assert!(s.stand_ok(s.walk.pos.x, s.walk.pos.z));
    }

    #[test]
    fn walk_cannot_pass_through_a_wall() {
        let mut s = room_state();
        s.set_view(ViewMode::Walk);
        // Face -Z (towards the wall at z = 0) and hold forward for two seconds.
        s.walk.yaw = std::f32::consts::PI;
        s.walk.pos = Vec3::new(3.0, EYE_HEIGHT, 1.0);
        s.key("w", false, false);
        for _ in 0..200 {
            assert!(s.stand_ok(s.walk.pos.x, s.walk.pos.z));
            s.walk_step(1.0 / 60.0);
        }
        assert!(s.walk.pos.z > 0.12, "z = {}", s.walk.pos.z);
    }

    #[test]
    fn walk_look_turns_and_drag_ends() {
        let mut s = room_state();
        s.set_view(ViewMode::Walk);
        let yaw = s.walk.yaw;
        s.pointer_down([400.0, 300.0], PointerButton::Left, false);
        s.pointer_move([520.0, 260.0]);
        assert!((s.walk.yaw - yaw).abs() > 0.05);
        s.pointer_up([520.0, 260.0], PointerButton::Left, false);
        assert!(s.walk_look.is_none());
    }
    #[test]
    fn copy_paste_duplicates_furniture() {
        let mut s = empty();
        s.set_tool(Tool::Furniture);
        s.pointer_down([500.0, 400.0], PointerButton::Left, false);
        assert_eq!(s.doc.furniture.len(), 1);
        s.key("c", true, false);
        s.key("v", true, false);
        assert_eq!(s.doc.furniture.len(), 2);
        let a = &s.doc.furniture[0];
        let b = &s.doc.furniture[1];
        assert_ne!(a.id, b.id);
        assert_eq!(b.center.x - a.center.x, 300);
        assert!(s.selected_furniture.is_some());
    }

    #[test]
    fn paste_walls_adds_copies_and_undo_restores() {
        let mut s = room_state();
        s.selection = s.doc.walls.iter().map(|w| w.id).collect();
        let walls = s.doc.walls.len();
        s.key("c", true, false);
        s.key("v", true, false);
        assert_eq!(s.doc.walls.len(), walls * 2);
        assert_eq!(s.selection.len(), walls);
        s.key("z", true, false);
        assert_eq!(s.doc.walls.len(), walls);
        assert_eq!(s.doc.rooms.len(), 1);
    }

    #[test]
    fn paste_into_another_level_retags_the_copies() {
        let mut s = room_state();
        s.add_level(false);
        s.set_level(0);
        s.selection = s.doc.walls.iter().map(|w| w.id).collect();
        let walls = s.doc.walls.len();
        s.key("c", true, false);
        s.set_level(1);
        s.key("v", true, false);
        assert_eq!(s.doc.walls.len(), walls * 2);
        let pasted: Vec<_> = s.doc.walls.iter().filter(|w| w.floor == 1).collect();
        assert_eq!(pasted.len(), walls);
        assert!(s.doc.walls.iter().filter(|w| w.floor == 0).count() == walls);
        s.key("z", true, false);
        assert_eq!(s.doc.walls.len(), walls);
        assert!(s.doc.walls.iter().all(|w| w.floor == 0));
    }

    #[test]
    fn paste_without_copy_is_a_no_op() {
        let mut s = room_state();
        let walls = s.doc.walls.len();
        s.key("v", true, false);
        assert_eq!(s.doc.walls.len(), walls);
        assert!(s.clipboard.is_none());
    }
    #[test]
    fn rename_room_is_undoable() {
        let mut s = room_state();
        let id = s.doc.rooms[0].id;
        let before = s.doc.rooms[0].name.clone();
        s.rename_room(id, "书房");
        assert_eq!(s.doc.rooms[0].name, "书房");
        s.key("z", true, false);
        assert_eq!(s.doc.rooms[0].name, before);
    }

    #[test]
    fn rename_room_prompt_returns_the_selected_room() {
        let mut s = room_state();
        s.selected_room = Some(s.doc.rooms[0].id);
        let (id, name) = s.selected_room_prompt().expect("prompt");
        assert_eq!(id, s.doc.rooms[0].id);
        assert!(!name.is_empty());
    }
    fn room_with_door() -> SceneState {
        let mut s = room_state();
        let wall_id = s.doc.walls[0].id;
        s.doc.openings.push(Opening {
            id: EntityId(900),
            wall_id,
            kind: OpeningKind::Door,
            offset_mm: 2000,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 0.0,
            hinge_right: false,
            price: None,
        });
        s
    }

    #[test]
    fn dragging_a_door_moves_it_along_the_wall() {
        let mut s = room_with_door();
        let from = s.cam.world_to_screen(Pt::new(2450, 0));
        s.pointer_down(from, PointerButton::Left, false);
        assert!(s.opening_drag.is_some(), "door should be grabbed");
        let to = s.cam.world_to_screen(Pt::new(3450, 0));
        s.pointer_move(to);
        s.pointer_up(to, PointerButton::Left, false);
        let offset = s.doc.openings[0].offset_mm;
        assert!((offset - 3000).abs() <= 60, "offset {offset}");
        s.key("z", true, false);
        assert_eq!(s.doc.openings[0].offset_mm, 2000);
    }

    #[test]
    fn dragging_a_door_past_the_wall_end_clamps() {
        let mut s = room_with_door();
        let from = s.cam.world_to_screen(Pt::new(2450, 0));
        s.pointer_down(from, PointerButton::Left, false);
        let far = s.cam.world_to_screen(Pt::new(20000, 0));
        s.pointer_move(far);
        s.pointer_up(far, PointerButton::Left, false);
        assert_eq!(s.doc.openings[0].offset_mm, 6000 - 900);
    }

    #[test]
    fn clicking_a_door_leaf_selects_the_opening_not_the_wall() {
        let mut s = room_with_door();
        // The leaf is drawn perpendicular to the wall, outside the 160 mm
        // centreline band. It used to fall through to wall selection.
        let leaf = s.cam.world_to_screen(Pt::new(2000, 450));
        s.pointer_down(leaf, PointerButton::Left, false);
        s.pointer_up(leaf, PointerButton::Left, false);
        assert_eq!(s.selected_opening, Some(EntityId(900)));
        assert!(
            s.selection.is_empty(),
            "clicking a door leaf must not also select the wall"
        );
    }

    #[test]
    fn a_click_on_a_door_does_not_change_its_offset() {
        let mut s = room_with_door();
        let at = s.cam.world_to_screen(Pt::new(2450, 0));
        s.pointer_down(at, PointerButton::Left, false);
        s.pointer_up(at, PointerButton::Left, false);
        assert_eq!(s.doc.openings[0].offset_mm, 2000);
        assert_eq!(s.history.undo_len(), 0);
    }
    #[test]
    fn copying_a_level_duplicates_walls_and_rooms() {
        let mut s = room_state();
        let base_walls = s.walls_for_floor(0).len();
        assert!(base_walls >= 4);
        s.add_level(true);
        assert_eq!(s.active_level, 1);
        assert_eq!(s.doc.level_count(), 2);
        assert_eq!(s.walls_for_floor(1).len(), base_walls);
        let unique: std::collections::HashSet<EntityId> =
            s.doc.walls.iter().map(|w| w.id).collect();
        assert_eq!(unique.len(), s.doc.walls.len(), "copied walls get new ids");
        assert!(s.doc.rooms.iter().any(|r| r.floor == 1));
        assert!((s.cam3.target.y - 4.0).abs() < 0.01, "camera rises to 2F");
        let top = s
            .mesh3d_with(false)
            .vertices
            .iter()
            .map(|v| v.pos[1])
            .fold(f32::MIN, f32::max);
        assert!(top > 5.0, "2F geometry is stacked above 1F, top = {top}");
        s.history.undo(&mut s.doc);
        assert_eq!(s.doc.level_count(), 1);
        assert!(s.doc.walls.iter().all(|w| w.floor == 0));
    }

    #[test]
    fn roof_form_and_eave_are_editable_and_undoable() {
        let mut s = room_state();
        assert_eq!(s.roof_settings().kind, RoofKind::Gable);
        assert_eq!(s.roof_settings().overhang_mm, 600);
        s.set_roof_kind(RoofKind::Flat);
        assert_eq!(s.doc.roof.kind, RoofKind::Flat);
        s.set_roof_kind(RoofKind::Gable);
        s.cycle_roof_pitch();
        assert!(s.roof_settings().pitch_deg > 25.0);
        s.cycle_roof_overhang();
        assert_eq!(s.roof_settings().overhang_mm, 900);
        s.history.undo(&mut s.doc);
        assert_eq!(s.doc.roof.overhang_mm, 600);
        s.history.undo(&mut s.doc);
        assert_eq!(s.doc.roof.kind, RoofKind::Gable);
        s.history.undo(&mut s.doc);
        assert_eq!(s.doc.roof.kind, RoofKind::Flat);
        s.set_roof(true);
        assert!(s.show_roof);
        assert!(s.mesh3d().len() > 0);
    }

    #[test]
    fn roof_is_drawn_only_when_toggled() {
        let mut s = room_state();
        s.add_level(true);
        let flat = s
            .mesh3d_with(false)
            .vertices
            .iter()
            .map(|v| v.pos[1])
            .fold(f32::MIN, f32::max);
        s.set_roof(true);
        let roofed = s
            .mesh3d_with(false)
            .vertices
            .iter()
            .map(|v| v.pos[1])
            .fold(f32::MIN, f32::max);
        assert!(roofed > flat + 0.05, "roof adds a slab: {flat} → {roofed}");
        s.set_roof(false);
        assert!(
            s.mesh3d_with(false)
                .vertices
                .iter()
                .map(|v| v.pos[1])
                .fold(f32::MIN, f32::max)
                < roofed
        );
    }

    #[test]
    fn deleting_a_level_removes_its_entities_and_is_undoable() {
        let mut s = room_state();
        s.add_level(true);
        let walls_before = s.doc.walls.len();
        s.remove_active_level();
        assert_eq!(s.doc.level_count(), 1);
        assert!(s.doc.walls.iter().all(|w| w.floor == 0));
        assert!(s.walls_for_floor(1).is_empty());
        assert_eq!(s.doc.walls.len() * 2, walls_before);
        s.history.undo(&mut s.doc);
        assert_eq!(s.doc.level_count(), 2);
        assert_eq!(s.doc.walls.len(), walls_before);
    }

    #[test]
    fn stair_is_only_placed_between_levels() {
        let mut s = room_state();
        s.add_level(true);
        s.set_level(0);
        s.commit_stair(Pt::new(1000, 1000), Pt::new(4000, 1000));
        assert_eq!(s.doc.stairs.len(), 1);
        let stair = &s.doc.stairs[0];
        assert_eq!(stair.floor, 0);
        assert_eq!(stair.steps, 12);
        s.set_level(1);
        s.commit_stair(Pt::new(1000, 1000), Pt::new(4000, 1000));
        assert_eq!(s.doc.stairs.len(), 1, "top floor has no stair target");
        assert!(s.note.as_deref().unwrap_or("").contains("顶层"));
        s.history.undo(&mut s.doc);
        assert!(s.doc.stairs.is_empty());
    }

    #[test]
    fn level_name_and_height_are_editable_and_undoable() {
        let mut s = room_state();
        s.add_level(true);
        s.set_level(1);
        assert_eq!(s.active_level_info(), ("2F".to_string(), 2800));
        s.set_active_level_info("主卧层", 3200).unwrap();
        assert_eq!(s.active_level_info(), ("主卧层".to_string(), 3200));
        assert_eq!(s.doc.levels[1].height_mm, 3200);
        assert!(s.set_active_level_info("", 3000).is_err());
        assert!(s.set_active_level_info("X", 900).is_err());
        s.history.undo(&mut s.doc);
        assert_eq!(s.doc.levels[1].height_mm, 2800);
        assert_eq!(s.doc.levels[1].name, "2F");
    }

    #[test]
    fn stair_size_and_direction_are_editable_and_undoable() {
        let mut s = room_state();
        s.add_level(true);
        s.set_level(0);
        s.commit_stair(Pt::new(1000, 1000), Pt::new(4000, 1000));
        let id = s.doc.stairs[0].id;
        s.selected_stair = Some(id);
        let before = s.selected_stair_info().unwrap();
        s.set_selected_stair_size(Some(1200), Some(14)).unwrap();
        let sized = s.selected_stair_info().unwrap();
        assert_eq!(sized.width_mm, 1200);
        assert_eq!(sized.steps, 14);
        assert!(s.set_selected_stair_size(Some(500), None).is_err());
        assert!(s.reverse_selected_stair());
        let reversed = s.selected_stair_info().unwrap();
        assert_eq!(reversed.start, before.end);
        assert_eq!(reversed.end, before.start);
        assert_eq!(reversed.width_mm, 1200);
        let view = s.inspector().expect("stair inspector");
        assert_eq!(view.kind, InspectorKind::Stair);
        assert!(
            view.actions
                .iter()
                .any(|(_, action)| matches!(action, InspectorAction::StairReverse))
        );
        s.history.undo(&mut s.doc);
        let undone = s.selected_stair_info().unwrap();
        assert_eq!(undone.start, before.start);
        assert_eq!(undone.width_mm, 1200);
    }

    #[test]
    fn walk_is_blocked_by_furniture() {
        let mut s = room_state();
        s.set_view(ViewMode::Walk);
        s.walk.pos = Vec3::new(3.0, EYE_HEIGHT, 1.0);
        s.walk.yaw = 0.0;
        s.doc.furniture.push(Furniture::from_spec(
            EntityId(700),
            &CATALOG[0],
            Pt::new(3000, 2000),
            0.0,
        ));
        s.key("w", false, false);
        for _ in 0..150 {
            assert!(!s.furniture_blocks(s.walk.pos.x, s.walk.pos.z));
            s.walk_step(1.0 / 60.0);
        }
        assert!(s.walk.pos.z < 1.55, "z = {}", s.walk.pos.z);
    }
}
