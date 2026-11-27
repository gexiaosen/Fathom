//! Document model for Floorplan-3D.
//!
//! All lengths are millimetres (see [`fp_core::Mm`]). Entity IDs are stable
//! across undo/redo and future sync, so commands never re-create objects.

use fp_core::{EntityId, Pt};
use serde::{Deserialize, Serialize};

pub mod catalog;
mod schema;
pub use schema::{DocError, from_json_str, to_json_string};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub meta: DocMeta,
    pub site: Site,
    pub walls: Vec<Wall>,
    pub openings: Vec<Opening>,
    pub rooms: Vec<Room>,
    pub furniture: Vec<Furniture>,
    pub materials: Vec<Material>,
    pub measures: Vec<Measure>,
    #[serde(default)]
    pub stairs: Vec<Stair>,
    pub settings: DocSettings,
    /// Whole-home quote rates. Missing blocks default to a residential fit-out
    /// baseline so older documents open with a usable quote instead of zeros.
    #[serde(default)]
    pub quote: QuoteSettings,
    /// Optional imported floor-plan image drawn under the 2D sheet.
    #[serde(default)]
    pub underlay: Option<Underlay>,
    /// Storey metadata. Old single-storey documents get one 1F level.
    #[serde(default = "default_levels")]
    pub levels: Vec<Level>,
    /// Roof form and eave, used when the 3D roof is switched on.
    #[serde(default)]
    pub roof: RoofSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoofKind {
    Flat,
    #[default]
    Gable,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoofSettings {
    pub kind: RoofKind,
    /// Slope of a pitched roof in degrees. Ignored for flat roofs.
    pub pitch_deg: f32,
    /// Horizontal eave projection beyond the outer wall, in millimetres.
    pub overhang_mm: i32,
}

impl Default for RoofSettings {
    fn default() -> Self {
        Self {
            kind: RoofKind::Gable,
            pitch_deg: 25.0,
            overhang_mm: 600,
        }
    }
}

impl RoofSettings {
    pub fn normalized(mut self) -> Self {
        self.pitch_deg = self.pitch_deg.clamp(10.0, 60.0);
        self.overhang_mm = self.overhang_mm.clamp(0, 1_500);
        self
    }
}

/// A reference image under the plan, positioned in plan millimetres.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Underlay {
    pub path: String,
    pub x_mm: i32,
    pub y_mm: i32,
    pub width_mm: i32,
    pub height_mm: i32,
    #[serde(default = "default_underlay_opacity")]
    pub opacity: f32,
    /// Storey this image belongs to.
    #[serde(default)]
    pub floor: u32,
}

fn default_underlay_opacity() -> f32 {
    0.50
}

impl Underlay {
    pub fn new(path: String, x_mm: i32, y_mm: i32, width_mm: i32, height_mm: i32) -> Self {
        Self {
            path,
            x_mm,
            y_mm,
            width_mm,
            height_mm,
            opacity: default_underlay_opacity(),
            floor: 0,
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            meta: DocMeta::default(),
            site: Site::default(),
            walls: Vec::new(),
            openings: Vec::new(),
            rooms: Vec::new(),
            furniture: Vec::new(),
            materials: Vec::new(),
            measures: Vec::new(),
            stairs: Vec::new(),
            settings: DocSettings::default(),
            quote: QuoteSettings::default(),
            underlay: None,
            levels: default_levels(),
            roof: RoofSettings::default(),
        }
    }
}

impl Document {
    pub fn new(name: impl Into<String>) -> Self {
        let mut doc = Self::default();
        doc.meta.name = name.into();
        doc
    }

    /// Storey names are one-based in the UI but zero-based here.
    pub fn level_count(&self) -> usize {
        self.levels.len().max(1)
    }

    pub fn level_height_mm(&self, level: usize) -> i32 {
        self.levels
            .get(level)
            .map(|l| l.height_mm.max(1000))
            .unwrap_or(2800)
    }

    /// Height of a storey's finished floor above level 0.
    pub fn level_elevation_mm(&self, level: usize) -> i32 {
        (0..level.min(self.level_count()))
            .map(|i| self.level_height_mm(i))
            .sum()
    }

    pub fn level_name(&self, level: usize) -> String {
        self.levels
            .get(level)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("F{}", level + 1))
    }

    pub fn ensure_levels(&mut self) {
        if self.levels.is_empty() {
            self.levels.push(Level::default());
        }
    }

    /// Next free entity id (max existing + 1).
    pub fn next_id(&self) -> u128 {
        let ids = self
            .walls
            .iter()
            .map(|w| w.id.0)
            .chain(self.openings.iter().map(|o| o.id.0))
            .chain(self.rooms.iter().map(|r| r.id.0))
            .chain(self.furniture.iter().map(|f| f.id.0))
            .chain(self.materials.iter().map(|m| m.id.0))
            .chain(self.measures.iter().map(|m| m.id.0))
            .chain(self.stairs.iter().map(|m| m.id.0));
        ids.max().unwrap_or(0) + 1
    }

    /// Lightweight structural validation, intended for load/save boundaries.
    pub fn validate(&self) -> Result<(), Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        if self.schema_version > SCHEMA_VERSION {
            issues.push(ValidationIssue::new("schema", "newer schema version"));
        }
        for wall in &self.walls {
            if wall.thickness_mm <= 0 {
                issues.push(ValidationIssue::new("wall", "thickness must be positive"));
            }
            if wall.start == wall.end {
                issues.push(ValidationIssue::new("wall", "zero-length wall"));
            }
        }
        for room in &self.rooms {
            if room.polygon.len() < 3 {
                issues.push(ValidationIssue::new(
                    "room",
                    "polygon needs at least 3 points",
                ));
            }
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocMeta {
    pub name: String,
    pub client: String,
    pub address: String,
    pub note: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Site {
    pub boundary: Vec<Pt>,
    pub floor: u32,
    pub unit: Unit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    #[default]
    Millimetre,
    Metre,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WallKind {
    Bearing,
    Exterior,
    #[default]
    Partition,
    Low,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallFinish {
    /// Matt latex paint on plaster.
    #[default]
    Paint,
    /// Subtle vertical paper with a small repeating motif.
    Wallpaper,
    /// Timber panels with a visible vertical grain.
    WoodPanel,
    /// Large-format wall tile with grout joints.
    Tile,
}

/// Which architectural surface a finish belongs to. Floors reuse the
/// historical `Room::material_id`, walls use `Wall::finish_id` and ceilings
/// use `Room::ceiling_material_id`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishSurface {
    #[default]
    Floor,
    Wall,
    Ceiling,
}

impl FinishSurface {
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Floor => "地面",
            Self::Wall => "墙面",
            Self::Ceiling => "顶面",
        }
    }

    pub fn label_en(self) -> &'static str {
        match self {
            Self::Floor => "Floor",
            Self::Wall => "Wall",
            Self::Ceiling => "Ceiling",
        }
    }

    /// Accepts the values typed into the material manager.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "floor" | "地面" | "地板" => Some(Self::Floor),
            "wall" | "墙面" | "墙" => Some(Self::Wall),
            "ceiling" | "顶面" | "天花" | "天花板" => Some(Self::Ceiling),
            _ => None,
        }
    }
}

impl WallKind {
    pub fn is_demolishable(self) -> bool {
        matches!(self, Self::Partition | Self::Low)
    }

    pub fn display_height_mm(self) -> i32 {
        match self {
            Self::Low => 1000,
            _ => 2800,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wall {
    pub id: EntityId,
    pub start: Pt,
    pub end: Pt,
    pub thickness_mm: i32,
    pub kind: WallKind,
    pub height_mm: i32,
    #[serde(default)]
    pub floor: u32,
    #[serde(default)]
    pub finish: WallFinish,
    /// Reference to a wall finish in `Document::materials`. `None` keeps the
    /// legacy enum above, so old plans render exactly as before.
    #[serde(default)]
    pub finish_id: Option<EntityId>,
}

impl Wall {
    pub fn new(id: EntityId, start: Pt, end: Pt, thickness_mm: i32, kind: WallKind) -> Self {
        Self {
            id,
            start,
            end,
            thickness_mm,
            kind,
            height_mm: kind.display_height_mm(),
            floor: 0,
            finish: WallFinish::Paint,
            finish_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpeningKind {
    #[default]
    Door,
    Window,
    Sliding,
    Bay,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opening {
    pub id: EntityId,
    pub wall_id: EntityId,
    pub kind: OpeningKind,
    /// Distance from the wall start to the opening start, in mm.
    pub offset_mm: i32,
    pub width_mm: i32,
    pub height_mm: i32,
    pub sill_mm: i32,
    pub swing_deg: f64,
    /// Hinge sits at the right jamb (offset + width) instead of the left one.
    #[serde(default)]
    pub hinge_right: bool,
    /// Unit price in yuan. `None` means "use the catalogue suggestion".
    #[serde(default)]
    pub price: Option<f64>,
}

impl Opening {
    /// Price shown in the quote: explicit override, otherwise a market guide
    /// price for the opening type.
    pub fn effective_price(&self) -> f64 {
        self.price
            .unwrap_or_else(|| suggested_opening_price(self.kind))
    }
}

pub fn suggested_opening_price(kind: OpeningKind) -> f64 {
    match kind {
        OpeningKind::Door => 1_800.0,
        OpeningKind::Window => 1_200.0,
        OpeningKind::Sliding => 2_200.0,
        OpeningKind::Bay => 3_000.0,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub id: EntityId,
    pub polygon: Vec<Pt>,
    pub name: String,
    pub material_id: Option<EntityId>,
    /// Whether the room counts toward usable area (bay windows often do not).
    pub counted: bool,
    #[serde(default)]
    pub floor: u32,
    /// Ceiling finish reference; `None` means the default ceiling colour.
    #[serde(default)]
    pub ceiling_material_id: Option<EntityId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Furniture {
    pub id: EntityId,
    pub asset_id: String,
    pub name: String,
    pub center: Pt,
    pub width_mm: i32,
    pub depth_mm: i32,
    pub rotation_deg: f64,
    pub color: String,
    pub room_id: Option<EntityId>,
    /// Unit price in yuan. `None` means "use the catalogue suggestion".
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub floor: u32,
}

impl Furniture {
    /// Price shown in the quote: explicit override, otherwise a catalogue
    /// guide price for the asset.
    pub fn effective_price(&self) -> f64 {
        self.price
            .unwrap_or_else(|| catalog::suggested_price(&self.asset_id))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub id: EntityId,
    pub name: String,
    pub price_per_m2: f64,
    pub kind: String,
    /// Optional custom sRGB hex colour (`#RRGGBB`). Empty keeps the standard
    /// palette tint for `kind`.
    #[serde(default)]
    pub color: String,
    /// Architectural surface this finish can be applied to.
    #[serde(default)]
    pub surface: FinishSurface,
    /// Cutting / pattern allowance as a fraction, e.g. 0.05 for 5%.
    #[serde(default = "default_waste_rate")]
    pub waste_rate: f64,
    /// True for the palette entries the app seeds itself. Used by the
    /// one-time finish-library migration; users can still edit or delete them.
    #[serde(default)]
    pub built_in: bool,
}

pub fn default_waste_rate() -> f64 {
    0.05
}

impl Material {
    pub fn surface_label_zh(&self) -> &'static str {
        self.surface.label_zh()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub id: EntityId,
    pub a: Pt,
    pub b: Pt,
    #[serde(default)]
    pub floor: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Level {
    /// Display name, e.g. "1F".
    pub name: String,
    /// Floor-to-floor height.
    #[serde(default = "default_level_height")]
    pub height_mm: i32,
}

fn default_level_height() -> i32 {
    2800
}

fn default_levels() -> Vec<Level> {
    vec![Level::default()]
}

impl Default for Level {
    fn default() -> Self {
        Self {
            name: "1F".into(),
            height_mm: default_level_height(),
        }
    }
}

/// A straight stair run. It connects `floor` to `floor + 1`; `start` is the
/// lower end and `end` the upper end, in plan millimetres.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stair {
    pub id: EntityId,
    #[serde(default)]
    pub floor: u32,
    pub start: Pt,
    pub end: Pt,
    pub width_mm: i32,
    #[serde(default = "default_steps")]
    pub steps: u32,
}

fn default_steps() -> u32 {
    16
}

impl Stair {
    pub fn new(id: EntityId, start: Pt, end: Pt, width_mm: i32, steps: u32) -> Self {
        Self {
            id,
            floor: 0,
            start,
            end,
            width_mm,
            steps,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocSettings {
    pub grid_mm: i32,
    pub snap: bool,
    pub show_dimensions: bool,
    /// Per-side dimension chains outside the plan outline. Independent of
    /// `show_dimensions`, which controls the small length label on each wall.
    #[serde(default = "default_true")]
    pub show_dimension_chains: bool,
    #[serde(default = "default_true")]
    pub show_room_names: bool,
    #[serde(default = "default_true")]
    pub show_furniture: bool,
    #[serde(default = "default_true")]
    pub show_grid: bool,
    #[serde(default = "default_true")]
    pub show_bearing: bool,
    #[serde(default = "default_true")]
    pub show_underlay: bool,
    /// Imported plans can carry authored room polygons while their wall
    /// strokes are too fragmented for `detect_rooms`. Keep those rooms when a
    /// wall edit cannot rebuild the graph instead of clearing the whole plan.
    #[serde(default)]
    pub keep_authored_rooms: bool,
    /// 1 once the built-in wall/ceiling finishes have been added to an old
    /// document. Kept in the document so opening a plan does not resurrect
    /// finishes the user deleted later.
    #[serde(default)]
    pub finish_library_version: u32,
}

fn default_true() -> bool {
    true
}

/// Rates used by the whole-home quote. Amounts are in yuan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuoteSettings {
    /// Labour cost per square metre of counted floor area.
    pub labor_per_m2: f64,
    /// Auxiliary materials (glue, screed, edge strips, …) per square metre.
    pub auxiliary_per_m2: f64,
    /// Tax rate as a fraction, e.g. 0.09 for 9%.
    pub tax_rate: f64,
    /// Discount as a fraction of the pre-tax subtotal, e.g. 0.05 for 5%.
    pub discount_rate: f64,
}

impl Default for QuoteSettings {
    fn default() -> Self {
        Self {
            labor_per_m2: 120.0,
            auxiliary_per_m2: 60.0,
            tax_rate: 0.09,
            discount_rate: 0.0,
        }
    }
}

impl Default for DocSettings {
    fn default() -> Self {
        Self {
            grid_mm: 10,
            snap: true,
            show_dimensions: true,
            show_dimension_chains: true,
            show_room_names: true,
            show_furniture: true,
            show_grid: true,
            show_bearing: true,
            show_underlay: true,
            keep_authored_rooms: false,
            finish_library_version: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    pub entity: &'static str,
    pub message: &'static str,
}

impl ValidationIssue {
    pub fn new(entity: &'static str, message: &'static str) -> Self {
        Self { entity, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(id: u128, start: Pt, end: Pt) -> Wall {
        Wall::new(EntityId(id), start, end, 120, WallKind::Partition)
    }

    #[test]
    fn default_document_is_valid_and_roundtrips() {
        let mut doc = Document::new("三室两厅");
        doc.walls.push(wall(1, Pt::new(0, 0), Pt::new(4000, 0)));
        assert!(doc.validate().is_ok());

        let json = serde_json::to_string(&doc).expect("serialize");
        let decoded: Document = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(doc, decoded);
    }

    #[test]
    fn old_walls_default_to_paint_finish() {
        let w = wall(3, Pt::new(0, 0), Pt::new(2000, 0));
        let mut value = serde_json::to_value(&w).expect("serialize wall");
        value.as_object_mut().expect("wall object").remove("finish");
        let decoded: Wall = serde_json::from_value(value).expect("deserialize wall");
        assert_eq!(decoded.finish, WallFinish::Paint);
    }

    #[test]
    fn zero_length_wall_is_rejected() {
        let mut doc = Document::new("bad");
        doc.walls
            .push(wall(2, Pt::new(100, 100), Pt::new(100, 100)));
        let issues = doc.validate().expect_err("should fail");
        assert_eq!(issues[0].entity, "wall");
    }

    #[test]
    fn levels_stack_and_report_elevation() {
        let mut doc = Document::new("多层");
        assert_eq!(doc.level_count(), 1);
        assert_eq!(doc.level_name(0), "1F");
        assert_eq!(doc.level_height_mm(0), 2800);
        assert_eq!(doc.level_elevation_mm(0), 0);
        doc.levels.push(Level {
            name: "2F".into(),
            height_mm: 3000,
        });
        assert_eq!(doc.level_count(), 2);
        assert_eq!(doc.level_height_mm(1), 3000);
        assert_eq!(doc.level_elevation_mm(1), 2800);
        assert_eq!(doc.level_name(1), "2F");
        doc.levels.clear();
        doc.ensure_levels();
        assert_eq!(doc.level_count(), 1, "an empty level list self-heals");
    }

    #[test]
    fn roof_defaults_when_missing_and_clamps() {
        let doc = Document::new("roof");
        let mut value: serde_json::Value = serde_json::from_str(&doc.to_json().unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("roof");
        let json = serde_json::to_string(&value).unwrap();
        let decoded = Document::from_json(&json).unwrap();
        assert_eq!(decoded.roof.kind, RoofKind::Gable);
        assert!((decoded.roof.pitch_deg - 25.0).abs() < 1e-6);
        assert_eq!(decoded.roof.overhang_mm, 600);
        let clamped = RoofSettings {
            kind: RoofKind::Flat,
            pitch_deg: 90.0,
            overhang_mm: 5_000,
        }
        .normalized();
        assert_eq!(clamped.pitch_deg, 60.0);
        assert_eq!(clamped.overhang_mm, 1_500);
    }

    #[test]
    fn stair_roundtrip_keeps_floor() {
        let mut doc = Document::new("楼梯");
        doc.levels.push(Level::default());
        let stair = Stair::new(EntityId(9), Pt::new(0, 0), Pt::new(3000, 0), 1000, 12);
        let mut on_upper = stair.clone();
        on_upper.floor = 1;
        doc.stairs.push(on_upper);
        let json = doc.to_json().expect("serialize");
        let decoded = Document::from_json(&json).expect("deserialize");
        assert_eq!(decoded.stairs.len(), 1);
        assert_eq!(decoded.stairs[0].floor, 1);
        assert_eq!(decoded.stairs[0].steps, 12);
    }

    #[test]
    fn bearing_and_partition_demolition_rules() {
        assert!(!WallKind::Bearing.is_demolishable());
        assert!(WallKind::Partition.is_demolishable());
        assert!(WallKind::Low.is_demolishable());
    }

    #[test]
    fn old_finish_fields_load_with_safe_defaults() {
        let mut doc = Document::new("旧户型");
        doc.materials.push(Material {
            id: EntityId(10),
            name: "旧地板".into(),
            price_per_m2: 100.0,
            kind: "wood".into(),
            color: String::new(),
            surface: FinishSurface::Floor,
            waste_rate: default_waste_rate(),
            built_in: false,
        });
        doc.walls.push(Wall::new(
            EntityId(11),
            Pt::new(0, 0),
            Pt::new(3000, 0),
            200,
            WallKind::Partition,
        ));
        doc.rooms.push(Room {
            id: EntityId(12),
            polygon: vec![
                Pt::new(0, 0),
                Pt::new(3000, 0),
                Pt::new(3000, 3000),
                Pt::new(0, 3000),
            ],
            name: "客厅".into(),
            material_id: Some(EntityId(10)),
            counted: true,
            floor: 0,
            ceiling_material_id: None,
        });
        let mut value: serde_json::Value = serde_json::from_str(&doc.to_json().unwrap()).unwrap();
        value["walls"][0]
            .as_object_mut()
            .unwrap()
            .remove("finish_id");
        value["rooms"][0]
            .as_object_mut()
            .unwrap()
            .remove("ceiling_material_id");
        let material = value["materials"][0].as_object_mut().unwrap();
        material.remove("surface");
        material.remove("waste_rate");
        material.remove("built_in");
        let decoded = Document::from_json(&serde_json::to_string(&value).unwrap()).unwrap();
        assert_eq!(decoded.walls[0].finish_id, None);
        assert_eq!(decoded.walls[0].finish, WallFinish::Paint);
        assert_eq!(decoded.rooms[0].ceiling_material_id, None);
        assert_eq!(decoded.materials[0].surface, FinishSurface::Floor);
        assert!((decoded.materials[0].waste_rate - 0.05).abs() < 1e-9);
        assert!(!decoded.materials[0].built_in);
        assert_eq!(FinishSurface::parse("墙面"), Some(FinishSurface::Wall));
    }
}
