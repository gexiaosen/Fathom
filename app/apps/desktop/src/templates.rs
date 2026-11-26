//! Starter plans for the new-project sheet.
//!
//! Templates are ordinary [`Document`] values: no wizard state, no generated
//! assets. Everything can be edited, undone and saved exactly like a hand-drawn
//! plan, and the demo remains the same fixture used by the Web importer.

use fp_core::{EntityId, Pt};
use fp_model::catalog::CATALOG;
use fp_model::{
    Document, FinishSurface, Furniture, Material, Opening, OpeningKind, Room, Wall, WallFinish,
    WallKind,
};

use crate::i18n::Lang;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateKind {
    Blank,
    Studio,
    TwoBed,
    Demo,
}

pub const KINDS: [TemplateKind; 4] = [
    TemplateKind::Blank,
    TemplateKind::Studio,
    TemplateKind::TwoBed,
    TemplateKind::Demo,
];

impl TemplateKind {
    pub fn title(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Blank, Lang::Zh) => "空白户型",
            (Self::Blank, Lang::En) => "Blank plan",
            (Self::Studio, Lang::Zh) => "一室一厅",
            (Self::Studio, Lang::En) => "1-bed apartment",
            (Self::TwoBed, Lang::Zh) => "两室两厅",
            (Self::TwoBed, Lang::En) => "2-bed apartment",
            (Self::Demo, Lang::Zh) => "示例公寓",
            (Self::Demo, Lang::En) => "Demo apartment",
        }
    }

    pub fn note(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Blank, Lang::Zh) => "从零开始画墙、门窗和房间",
            (Self::Blank, Lang::En) => "Start from an empty grid",
            (Self::Studio, Lang::Zh) => "7.2 × 5.2 m · 客厅 + 卧室",
            (Self::Studio, Lang::En) => "7.2 × 5.2 m · living + bedroom",
            (Self::TwoBed, Lang::Zh) => "10.2 × 7.2 m · 客厅 + 主卧 + 次卧",
            (Self::TwoBed, Lang::En) => "10.2 × 7.2 m · living + 2 bedrooms",
            (Self::Demo, Lang::Zh) => "完整的多房间样例，含家具与材质",
            (Self::Demo, Lang::En) => "Full multi-room sample with furniture",
        }
    }
}

pub fn build(kind: TemplateKind, lang: Lang) -> Document {
    match kind {
        TemplateKind::Blank => blank(lang),
        TemplateKind::Studio => studio(lang),
        TemplateKind::TwoBed => two_bed(lang),
        TemplateKind::Demo => demo(lang),
    }
}

fn blank(lang: Lang) -> Document {
    Document::new(match lang {
        Lang::Zh => "未命名户型",
        Lang::En => "Untitled plan",
    })
}

fn demo(lang: Lang) -> Document {
    let mut doc = Document::from_json(include_str!("../../../assets/samples/demo-apartment.json"))
        .unwrap_or_else(|_| studio(lang));
    if lang == Lang::En {
        doc.meta.name = "Demo apartment".into();
    }
    doc
}

fn studio(lang: Lang) -> Document {
    let mut doc = blank(lang);
    doc.meta.name = match lang {
        Lang::Zh => "一室一厅".into(),
        Lang::En => "1-bed apartment".into(),
    };
    doc.meta.note = "Fathom template".into();
    materials(&mut doc);

    wall(&mut doc, 1, (0, 0), (7200, 0), 240, WallKind::Exterior);
    wall(
        &mut doc,
        2,
        (7200, 0),
        (7200, 5200),
        240,
        WallKind::Exterior,
    );
    wall(
        &mut doc,
        3,
        (7200, 5200),
        (0, 5200),
        240,
        WallKind::Exterior,
    );
    wall(&mut doc, 4, (0, 5200), (0, 0), 240, WallKind::Exterior);
    wall(
        &mut doc,
        5,
        (4600, 0),
        (4600, 5200),
        120,
        WallKind::Partition,
    );

    opening(&mut doc, 11, 1, OpeningKind::Window, 1000, 1600, 1500, 900);
    opening(&mut doc, 12, 1, OpeningKind::Window, 5000, 1400, 1500, 900);
    opening(&mut doc, 13, 3, OpeningKind::Door, 1200, 900, 2100, 0);
    opening(&mut doc, 14, 5, OpeningKind::Door, 3600, 800, 2100, 0);

    room(
        &mut doc,
        101,
        &[(120, 120), (4540, 120), (4540, 5080), (120, 5080)],
        match lang {
            Lang::Zh => "客厅",
            Lang::En => "Living room",
        },
        true,
    );
    room(
        &mut doc,
        102,
        &[(4660, 120), (7080, 120), (7080, 5080), (4660, 5080)],
        match lang {
            Lang::Zh => "卧室",
            Lang::En => "Bedroom",
        },
        true,
    );

    furniture(&mut doc, 301, "sofa", 1500, 1200, 0.0, 101);
    furniture(&mut doc, 302, "coffee_table", 1500, 2300, 0.0, 101);
    furniture(&mut doc, 303, "tv", 1500, 4300, 0.0, 101);
    furniture(&mut doc, 304, "dining_table", 3400, 3800, 0.0, 101);
    furniture(&mut doc, 305, "bed", 5870, 1700, 0.0, 102);
    furniture(&mut doc, 306, "nightstand", 6820, 700, 0.0, 102);
    furniture(&mut doc, 307, "wardrobe", 6100, 3900, 0.0, 102);
    doc
}

fn two_bed(lang: Lang) -> Document {
    let mut doc = blank(lang);
    doc.meta.name = match lang {
        Lang::Zh => "两室两厅".into(),
        Lang::En => "2-bed apartment".into(),
    };
    doc.meta.note = "Fathom template".into();
    materials(&mut doc);

    wall(&mut doc, 1, (0, 0), (10200, 0), 240, WallKind::Exterior);
    wall(
        &mut doc,
        2,
        (10200, 0),
        (10200, 7200),
        240,
        WallKind::Exterior,
    );
    wall(
        &mut doc,
        3,
        (10200, 7200),
        (0, 7200),
        240,
        WallKind::Exterior,
    );
    wall(&mut doc, 4, (0, 7200), (0, 0), 240, WallKind::Exterior);
    wall(
        &mut doc,
        5,
        (5200, 0),
        (5200, 7200),
        120,
        WallKind::Partition,
    );
    wall(
        &mut doc,
        6,
        (5200, 3600),
        (10200, 3600),
        120,
        WallKind::Partition,
    );

    opening(&mut doc, 11, 1, OpeningKind::Window, 600, 2200, 1500, 900);
    opening(&mut doc, 12, 1, OpeningKind::Window, 6100, 1800, 1500, 900);
    opening(&mut doc, 13, 3, OpeningKind::Door, 1200, 900, 2100, 0);
    opening(&mut doc, 14, 3, OpeningKind::Window, 6100, 1400, 1500, 900);
    opening(&mut doc, 15, 5, OpeningKind::Door, 1500, 800, 2100, 0);
    opening(&mut doc, 16, 5, OpeningKind::Door, 5200, 800, 2100, 0);

    room(
        &mut doc,
        101,
        &[(120, 120), (5080, 120), (5080, 7080), (120, 7080)],
        match lang {
            Lang::Zh => "客厅",
            Lang::En => "Living room",
        },
        true,
    );
    room(
        &mut doc,
        102,
        &[(5320, 120), (10080, 120), (10080, 3480), (5320, 3480)],
        match lang {
            Lang::Zh => "主卧",
            Lang::En => "Primary bedroom",
        },
        true,
    );
    room(
        &mut doc,
        103,
        &[(5320, 3720), (10080, 3720), (10080, 7080), (5320, 7080)],
        match lang {
            Lang::Zh => "次卧",
            Lang::En => "Second bedroom",
        },
        true,
    );

    furniture(&mut doc, 301, "sofa", 1700, 1200, 0.0, 101);
    furniture(&mut doc, 302, "coffee_table", 1700, 2400, 0.0, 101);
    furniture(&mut doc, 303, "tv", 1700, 4300, 0.0, 101);
    furniture(&mut doc, 304, "dining_table", 3900, 5600, 0.0, 101);
    furniture(&mut doc, 305, "bed", 7600, 1700, 0.0, 102);
    furniture(&mut doc, 306, "nightstand", 8800, 700, 0.0, 102);
    furniture(&mut doc, 307, "wardrobe", 9000, 3000, 0.0, 102);
    furniture(&mut doc, 308, "bed_single", 7600, 5200, 0.0, 103);
    furniture(&mut doc, 309, "kids_desk", 9000, 6100, 0.0, 103);
    doc
}

/// The standard flooring palette. The web original has five swatches; Fathom
/// keeps six priced options and gives every route (blank, templates, imported
/// JSON) the same palette.
pub const MATERIAL_LIBRARY: [(&str, f64, &str, &str); 6] = [
    ("橡木地板", 320.0, "wood", "#c49a6c"),
    ("瓷砖", 180.0, "tile", "#a8b6c2"),
    ("大理石", 680.0, "marble", "#d6d4cd"),
    ("水磨石", 260.0, "terrazzo", "#cbc6b7"),
    ("地毯", 220.0, "carpet", "#9e8c80"),
    ("微水泥", 240.0, "stone", "#b8b4ac"),
];

/// Built-in wall and ceiling finishes added once to older plans. The `kind`
/// is the shader/catalogue key, so custom finishes can reuse it.
pub const FINISH_LIBRARY: [(FinishSurface, &str, f64, &str, &str); 6] = [
    (FinishSurface::Wall, "墙面乳胶漆", 45.0, "paint", "#d0c9bc"),
    (
        FinishSurface::Wall,
        "无纺布壁纸",
        80.0,
        "wallpaper",
        "#c9c0b1",
    ),
    (
        FinishSurface::Wall,
        "木饰面",
        380.0,
        "wood_panel",
        "#86664a",
    ),
    (FinishSurface::Wall, "墙砖", 220.0, "tile", "#b9c0bd"),
    (
        FinishSurface::Ceiling,
        "顶面乳胶漆",
        38.0,
        "paint",
        "#e8e6df",
    ),
    (
        FinishSurface::Ceiling,
        "顶面木饰面",
        320.0,
        "wood_panel",
        "#7a614b",
    ),
];

/// Seed the standard palette into a document that has no material library,
/// then add the wall/ceiling presets once. The version lives in the document
/// so a preset the user deletes is not recreated on the next open.
pub fn ensure_materials(doc: &mut Document) {
    if doc.materials.is_empty() {
        for (index, (name, price, kind, color)) in MATERIAL_LIBRARY.into_iter().enumerate() {
            // Keep the historical 201..206 material ids for starter plans, but
            // never reuse an id that a wall, room, opening or piece already owns.
            let preferred = 201 + index as u128;
            let id = if id_in_use(doc, preferred) {
                doc.next_id()
            } else {
                preferred
            };
            doc.materials.push(Material {
                id: EntityId(id),
                name: name.into(),
                price_per_m2: price,
                kind: kind.into(),
                color: color.into(),
                surface: FinishSurface::Floor,
                waste_rate: fp_model::default_waste_rate(),
                built_in: true,
            });
        }
    }
    ensure_finish_library(doc);
}

fn ensure_finish_library(doc: &mut Document) {
    if doc.settings.finish_library_version >= 1 {
        return;
    }
    for (index, (surface, name, price, kind, color)) in FINISH_LIBRARY.into_iter().enumerate() {
        let preferred = 211 + index as u128;
        let id = if id_in_use(doc, preferred) {
            doc.next_id()
        } else {
            preferred
        };
        doc.materials.push(Material {
            id: EntityId(id),
            name: name.into(),
            price_per_m2: price,
            kind: kind.into(),
            color: color.into(),
            surface,
            waste_rate: if surface == FinishSurface::Wall {
                0.08
            } else {
                0.03
            },
            built_in: true,
        });
    }

    // Old walls carried only the enum. Point them at the matching built-in so
    // the inspector, 3D view and quote all read the same finish record.
    let paint_wall = built_in_finish_id(doc, FinishSurface::Wall, "paint");
    let wallpaper = built_in_finish_id(doc, FinishSurface::Wall, "wallpaper");
    let wood_panel = built_in_finish_id(doc, FinishSurface::Wall, "wood_panel");
    let tile = built_in_finish_id(doc, FinishSurface::Wall, "tile");
    for wall in &mut doc.walls {
        if wall.finish_id.is_some() {
            continue;
        }
        wall.finish_id = match wall.finish {
            WallFinish::Paint => paint_wall,
            WallFinish::Wallpaper => wallpaper,
            WallFinish::WoodPanel => wood_panel,
            WallFinish::Tile => tile,
        };
    }
    // Every room starts with the standard ceiling paint. This is what makes
    // the ceiling line appear in the quote without asking users to click.
    let ceiling_paint = built_in_finish_id(doc, FinishSurface::Ceiling, "paint");
    for room in &mut doc.rooms {
        if room.ceiling_material_id.is_none() {
            room.ceiling_material_id = ceiling_paint;
        }
    }
    doc.settings.finish_library_version = 1;
}

pub fn built_in_finish_id(doc: &Document, surface: FinishSurface, kind: &str) -> Option<EntityId> {
    doc.materials
        .iter()
        .find(|m| m.built_in && m.surface == surface && m.kind == kind)
        .map(|m| m.id)
}

fn id_in_use(doc: &Document, id: u128) -> bool {
    doc.walls.iter().any(|w| w.id.0 == id)
        || doc.openings.iter().any(|o| o.id.0 == id)
        || doc.rooms.iter().any(|r| r.id.0 == id)
        || doc.furniture.iter().any(|f| f.id.0 == id)
        || doc.materials.iter().any(|m| m.id.0 == id)
        || doc.measures.iter().any(|m| m.id.0 == id)
        || doc.stairs.iter().any(|s| s.id.0 == id)
}

fn materials(doc: &mut Document) {
    ensure_materials(doc);
}

fn wall(
    doc: &mut Document,
    id: u128,
    a: (i32, i32),
    b: (i32, i32),
    thickness_mm: i32,
    kind: WallKind,
) {
    doc.walls.push(Wall::new(
        EntityId(id),
        Pt::new(a.0, a.1),
        Pt::new(b.0, b.1),
        thickness_mm,
        kind,
    ));
}

#[allow(clippy::too_many_arguments)]
fn opening(
    doc: &mut Document,
    id: u128,
    wall_id: u128,
    kind: OpeningKind,
    offset_mm: i32,
    width_mm: i32,
    height_mm: i32,
    sill_mm: i32,
) {
    doc.openings.push(Opening {
        id: EntityId(id),
        wall_id: EntityId(wall_id),
        kind,
        offset_mm,
        width_mm,
        height_mm,
        sill_mm,
        swing_deg: if kind == OpeningKind::Door { 90.0 } else { 0.0 },
        hinge_right: false,
        price: None,
    });
}

fn room(doc: &mut Document, id: u128, points: &[(i32, i32)], name: &str, counted: bool) {
    doc.rooms.push(Room {
        id: EntityId(id),
        polygon: points.iter().map(|&(x, y)| Pt::new(x, y)).collect(),
        name: name.into(),
        material_id: Some(EntityId(201)),
        counted,
        floor: 0,
        ceiling_material_id: None,
    });
}

fn furniture(
    doc: &mut Document,
    id: u128,
    asset_id: &str,
    x: i32,
    y: i32,
    rotation_deg: f64,
    room_id: u128,
) {
    let Some(spec) = CATALOG.iter().find(|spec| spec.asset_id == asset_id) else {
        return;
    };
    doc.furniture.push(Furniture {
        id: EntityId(id),
        asset_id: spec.asset_id.into(),
        name: spec.name.into(),
        center: Pt::new(x, y),
        width_mm: spec.width_mm,
        depth_mm: spec.depth_mm,
        rotation_deg,
        color: spec.color.to_owned(),
        room_id: Some(EntityId(room_id)),
        price: None,
        floor: 0,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_structurally_valid() {
        for kind in KINDS {
            for lang in [Lang::Zh, Lang::En] {
                let doc = build(kind, lang);
                assert!(doc.validate().is_ok(), "{kind:?} / {lang:?} validates");
                if kind != TemplateKind::Blank {
                    assert!(!doc.walls.is_empty());
                    assert!(!doc.rooms.is_empty());
                }
            }
        }
    }

    #[test]
    fn openings_point_at_existing_walls() {
        for kind in [
            TemplateKind::Studio,
            TemplateKind::TwoBed,
            TemplateKind::Demo,
        ] {
            let doc = build(kind, Lang::Zh);
            for opening in &doc.openings {
                assert!(
                    doc.walls.iter().any(|wall| wall.id == opening.wall_id),
                    "{kind:?} opening {} points at wall {}",
                    opening.id.0,
                    opening.wall_id.0
                );
            }
        }
    }

    #[test]
    fn finish_library_migrates_old_walls_and_ceilings_once() {
        let mut doc = Document::new("旧图");
        doc.materials.push(Material {
            id: EntityId(201),
            name: "旧橡木".into(),
            price_per_m2: 100.0,
            kind: "wood".into(),
            color: String::new(),
            surface: FinishSurface::Floor,
            waste_rate: 0.05,
            built_in: true,
        });
        let mut wall = Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(3000, 0),
            200,
            WallKind::Partition,
        );
        wall.finish = WallFinish::Wallpaper;
        doc.walls.push(wall);
        doc.rooms.push(Room {
            id: EntityId(2),
            polygon: vec![
                Pt::new(0, 0),
                Pt::new(3000, 0),
                Pt::new(3000, 3000),
                Pt::new(0, 3000),
            ],
            name: "客厅".into(),
            material_id: Some(EntityId(201)),
            counted: true,
            floor: 0,
            ceiling_material_id: None,
        });
        assert_eq!(doc.settings.finish_library_version, 0);
        ensure_materials(&mut doc);
        assert_eq!(doc.settings.finish_library_version, 1);
        let wall_finishes = doc
            .materials
            .iter()
            .filter(|m| m.surface == FinishSurface::Wall)
            .count();
        let ceiling_finishes = doc
            .materials
            .iter()
            .filter(|m| m.surface == FinishSurface::Ceiling)
            .count();
        assert_eq!(wall_finishes, 4);
        assert_eq!(ceiling_finishes, 2);
        let wallpaper_id = built_in_finish_id(&doc, FinishSurface::Wall, "wallpaper");
        assert_eq!(doc.walls[0].finish_id, wallpaper_id);
        let ceiling_id = built_in_finish_id(&doc, FinishSurface::Ceiling, "paint");
        assert_eq!(doc.rooms[0].ceiling_material_id, ceiling_id);

        // Once migrated, a manually deleted preset stays deleted.
        let before = doc.materials.len();
        doc.materials.retain(|m| m.kind != "wallpaper");
        ensure_materials(&mut doc);
        assert_eq!(doc.materials.len(), before - 1);
    }

    #[test]
    fn entity_ids_are_unique_within_a_template() {
        for kind in [TemplateKind::Studio, TemplateKind::TwoBed] {
            let doc = build(kind, Lang::Zh);
            let mut ids = Vec::new();
            ids.extend(doc.walls.iter().map(|w| w.id));
            ids.extend(doc.openings.iter().map(|o| o.id));
            ids.extend(doc.rooms.iter().map(|r| r.id));
            ids.extend(doc.furniture.iter().map(|f| f.id));
            ids.extend(doc.materials.iter().map(|m| m.id));
            for &id in &ids {
                assert_eq!(
                    ids.iter().filter(|&&other| other == id).count(),
                    1,
                    "{kind:?} id {} is unique",
                    id.0
                );
            }
        }
    }
}
