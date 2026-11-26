//! Document loading, saving and schema migration.
//!
//! `schema_version` is a monotonic integer. Loading a document with an older
//! version runs migrations in order; loading a newer one fails loudly instead of
//! silently dropping unknown fields.

use serde_json::{Value, json};

use crate::{Document, SCHEMA_VERSION, ValidationIssue};

#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("document schema {found} is newer than supported {supported}")]
    NewerSchema { found: u32, supported: u32 },
    #[error("document validation failed: {0:?}")]
    Invalid(Vec<ValidationIssue>),
}

pub fn from_json_str(text: &str) -> Result<Document, DocError> {
    let mut value: Value = serde_json::from_str(text)?;
    let found = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;

    if found > SCHEMA_VERSION {
        return Err(DocError::NewerSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }

    if found < SCHEMA_VERSION {
        migrate(&mut value, found);
    }

    let mut doc: Document = serde_json::from_value(value)?;
    doc.ensure_levels();
    if let Err(issues) = doc.validate() {
        return Err(DocError::Invalid(issues));
    }
    Ok(doc)
}

pub fn to_json_string(doc: &Document) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(doc)
}

/// Fill every missing field with the default document's value. This is how v0
/// (pre-versioned) documents become readable without bespoke per-field code.
fn migrate(value: &mut Value, from: u32) {
    if from < 1 {
        let defaults =
            serde_json::to_value(Document::default()).expect("default document serializes");
        merge_defaults(value, &defaults);
        value["schema_version"] = json!(SCHEMA_VERSION);
    }
}

fn merge_defaults(target: &mut Value, defaults: &Value) {
    if let (Value::Object(t), Value::Object(d)) = (target, defaults) {
        for (key, default_value) in d {
            match t.get_mut(key) {
                Some(existing) => merge_defaults(existing, default_value),
                None => {
                    t.insert(key.clone(), default_value.clone());
                }
            }
        }
    }
}

impl Document {
    pub fn from_json(text: &str) -> Result<Self, DocError> {
        from_json_str(text)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        to_json_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocMeta, DocSettings, EntityId, Pt, Site, Unit, Wall, WallKind};

    #[test]
    fn v0_document_is_migrated() {
        let legacy = r#"{
            "meta": {"name": "老文档"},
            "walls": [
                {"id": 7, "start": {"x": 0, "y": 0}, "end": {"x": 4000, "y": 0},
                 "thickness_mm": 120, "kind": "Partition", "height_mm": 2800}
            ]
        }"#;
        let doc = from_json_str(legacy).expect("v0 migrates");
        assert_eq!(doc.schema_version, SCHEMA_VERSION);
        assert_eq!(doc.meta.name, "老文档");
        assert_eq!(doc.walls.len(), 1);
        assert_eq!(doc.settings.grid_mm, DocSettings::default().grid_mm);
        assert_eq!(doc.site.unit, Unit::Millimetre);
    }

    #[test]
    fn newer_schema_is_rejected() {
        let future = r#"{"schema_version": 99, "meta": {"name": "x"}}"#;
        match from_json_str(future) {
            Err(DocError::NewerSchema { found, supported }) => {
                assert_eq!(found, 99);
                assert_eq!(supported, SCHEMA_VERSION);
            }
            other => panic!("expected NewerSchema, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_preserves_document() {
        let mut doc = Document::new("往返");
        doc.meta = DocMeta {
            name: "往返".into(),
            ..Default::default()
        };
        doc.site = Site {
            floor: 2,
            ..Default::default()
        };
        doc.walls.push(Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            120,
            WallKind::Partition,
        ));
        let json = to_json_string(&doc).expect("serialize");
        let back = from_json_str(&json).expect("deserialize");
        assert_eq!(doc, back);
    }

    #[test]
    fn missing_layer_flags_default_to_visible() {
        let json = to_json_string(&Document::default()).expect("serialize");
        let mut value: Value = serde_json::from_str(&json).expect("parse");
        let settings = value
            .get_mut("settings")
            .and_then(Value::as_object_mut)
            .expect("settings object");
        for key in [
            "show_room_names",
            "show_furniture",
            "show_grid",
            "show_bearing",
        ] {
            settings.remove(key);
        }
        let doc = from_json_str(&value.to_string()).expect("old settings still load");
        assert!(doc.settings.show_room_names);
        assert!(doc.settings.show_furniture);
        assert!(doc.settings.show_grid);
        assert!(doc.settings.show_bearing);
    }

    #[test]
    fn validation_failure_surfaces_as_error() {
        let bad = r#"{
            "schema_version": 1,
            "meta": {"name": "bad", "client": "", "address": "", "note": "",
                     "created_at": "", "updated_at": ""},
            "site": {"boundary": [], "floor": 1, "unit": "Millimetre"},
            "openings": [], "rooms": [], "furniture": [], "materials": [], "measures": [],
            "settings": {"grid_mm": 10, "snap": true, "show_dimensions": true},
            "walls": [
                {"id": 1, "start": {"x": 0, "y": 0}, "end": {"x": 0, "y": 0},
                 "thickness_mm": 120, "kind": "Partition", "height_mm": 2800}
            ]
        }"#;
        match from_json_str(bad) {
            Err(DocError::Invalid(issues)) => assert_eq!(issues[0].entity, "wall"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}
