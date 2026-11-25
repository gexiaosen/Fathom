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
    pub settings: DocSettings,
    /// Optional imported floor-plan image drawn under the 2D sheet.
    #[serde(default)]
    pub underlay: Option<Underlay>,
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
            settings: DocSettings::default(),
            underlay: None,
        }
    }
}

impl Document {
    pub fn new(name: impl Into<String>) -> Self {
        let mut doc = Self::default();
        doc.meta.name = name.into();
        doc
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
            .chain(self.measures.iter().map(|m| m.id.0));
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub id: EntityId,
    pub polygon: Vec<Pt>,
    pub name: String,
    pub material_id: Option<EntityId>,
    /// Whether the room counts toward usable area (bay windows often do not).
    pub counted: bool,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub id: EntityId,
    pub name: String,
    pub price_per_m2: f64,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub id: EntityId,
    pub a: Pt,
    pub b: Pt,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocSettings {
    pub grid_mm: i32,
    pub snap: bool,
    pub show_dimensions: bool,
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
}

fn default_true() -> bool {
    true
}

impl Default for DocSettings {
    fn default() -> Self {
        Self {
            grid_mm: 10,
            snap: true,
            show_dimensions: true,
            show_room_names: true,
            show_furniture: true,
            show_grid: true,
            show_bearing: true,
            show_underlay: true,
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
    fn zero_length_wall_is_rejected() {
        let mut doc = Document::new("bad");
        doc.walls
            .push(wall(2, Pt::new(100, 100), Pt::new(100, 100)));
        let issues = doc.validate().expect_err("should fail");
        assert_eq!(issues[0].entity, "wall");
    }

    #[test]
    fn bearing_and_partition_demolition_rules() {
        assert!(!WallKind::Bearing.is_demolishable());
        assert!(WallKind::Partition.is_demolishable());
        assert!(WallKind::Low.is_demolishable());
    }
}
