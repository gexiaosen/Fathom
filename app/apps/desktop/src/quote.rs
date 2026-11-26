//! Whole-home quote: one place that turns the document into priced lines.
//!
//! Flooring keeps the 5% cutting allowance used by the original BOM. Furniture
//! and openings use their explicit unit price when set, otherwise the catalogue
//! guide price. Labour and auxiliary materials are rate × counted floor area.

use fp_geom::area;
use fp_model::{Document, OpeningKind};

pub const WASTE: f64 = 0.05;

#[derive(Clone, Debug)]
pub struct FinishRow {
    /// 地面 / 墙面 / 顶面.
    pub surface: String,
    pub name: String,
    pub area_m2: f64,
    pub billed_area_m2: f64,
    pub unit_price: f64,
    pub amount: f64,
}

#[derive(Clone, Debug)]
pub struct RoomQuote {
    pub floor: u32,
    pub name: String,
    pub area_m2: f64,
    pub billed_area_m2: f64,
    pub material_name: String,
    pub material_rate: f64,
    pub floor_cost: f64,
    pub ceiling_cost: f64,
    pub furniture_count: usize,
    pub furniture_cost: f64,
}

impl RoomQuote {
    pub fn total(&self) -> f64 {
        self.floor_cost + self.ceiling_cost + self.furniture_cost
    }
}

#[derive(Clone, Debug)]
pub struct ItemRow {
    pub name: String,
    pub count: usize,
    pub unit_price: f64,
    pub amount: f64,
}

/// Direct-cost roll-up for one storey. Whole-home labour, auxiliary and
/// tax are intentionally excluded: they are not attributable to a floor.
#[derive(Clone, Debug)]
pub struct LevelQuote {
    pub floor: u32,
    pub name: String,
    pub room_count: usize,
    pub net_area_m2: f64,
    pub billed_area_m2: f64,
    pub floor_cost: f64,
    pub ceiling_cost: f64,
    pub finish_cost: f64,
    pub furniture_count: usize,
    pub furniture_cost: f64,
    pub opening_total: f64,
    pub total: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Quote {
    pub rooms: Vec<RoomQuote>,
    pub levels: Vec<LevelQuote>,
    pub floor_net_area: f64,
    pub floor_billed_area: f64,
    pub floor_material: f64,
    pub floor_waste: f64,
    /// Wall finishes after deducting door and window openings.
    pub wall_net_area: f64,
    pub wall_billed_area: f64,
    pub wall_material: f64,
    /// Ceiling finishes, one room polygon at a time.
    pub ceiling_net_area: f64,
    pub ceiling_billed_area: f64,
    pub ceiling_material: f64,
    /// Wall + ceiling.
    pub finish_material: f64,
    /// Per-finish line items for the quote document.
    pub finish_rows: Vec<FinishRow>,
    pub furniture: Vec<ItemRow>,
    pub furniture_count: usize,
    pub furniture_total: f64,
    pub door_count: usize,
    pub door_total: f64,
    pub window_count: usize,
    pub window_total: f64,
    pub opening_total: f64,
    pub labor_area: f64,
    pub labor: f64,
    pub auxiliary: f64,
    pub subtotal: f64,
    pub discount: f64,
    pub taxable: f64,
    pub tax: f64,
    pub total: f64,
}

impl Quote {
    pub fn compute(doc: &Document) -> Self {
        let mut quote = Self::default();
        let mut rows: Vec<ItemRow> = Vec::new();

        for room in &doc.rooms {
            let area_m2 = area(&room.polygon) / 1_000_000.0;
            let material = room
                .material_id
                .and_then(|id| doc.materials.iter().find(|m| m.id == id))
                // Imported/older rooms may not name a material; fall back to
                // the document's primary flooring instead of quoting ¥0.
                .or_else(|| {
                    doc.materials
                        .iter()
                        .find(|m| m.surface == fp_model::FinishSurface::Floor)
                });
            let material_name = material.map(|m| m.name.clone()).unwrap_or_default();
            let material_rate = material.map(|m| m.price_per_m2).unwrap_or(0.0);
            let waste = material.map(|m| m.waste_rate).unwrap_or(WASTE);
            let billed = if room.counted {
                area_m2 * (1.0 + waste)
            } else {
                area_m2
            };
            let floor_cost = billed * material_rate;
            if room.counted {
                quote.floor_waste += billed * material_rate - area_m2 * material_rate;
            }

            // Ceiling finish: polygon area × finish rate. This is deliberately
            // separate from the floor line even when both use the same kind.
            let ceiling = room
                .ceiling_material_id
                .and_then(|id| doc.materials.iter().find(|m| m.id == id));
            let ceiling_rate = ceiling.map(|m| m.price_per_m2).unwrap_or(0.0);
            let ceiling_waste = ceiling.map(|m| m.waste_rate).unwrap_or(WASTE);
            let ceiling_billed = area_m2 * (1.0 + ceiling_waste);
            let ceiling_cost = ceiling_billed * ceiling_rate;

            if room.counted {
                quote.floor_net_area += area_m2;
                quote.floor_billed_area += billed;
                quote.floor_material += floor_cost;
                quote.ceiling_net_area += area_m2;
                quote.ceiling_billed_area += ceiling_billed;
                quote.ceiling_material += ceiling_cost;
                quote.labor_area += area_m2;
                if let Some(m) = ceiling {
                    push_finish_row(
                        &mut quote.finish_rows,
                        "顶面",
                        &m.name,
                        area_m2,
                        ceiling_billed,
                        m.price_per_m2,
                    );
                }
            }

            let mut furniture_count = 0usize;
            let mut furniture_cost = 0.0f64;
            for item in &doc.furniture {
                if item.room_id != Some(room.id) {
                    continue;
                }
                furniture_count += 1;
                let price = item.effective_price();
                furniture_cost += price;
                push_row(&mut rows, &item.name, price);
            }

            let display_name = if doc.level_count() > 1 {
                format!("{} · {}", doc.level_name(room.floor as usize), room.name)
            } else {
                room.name.clone()
            };
            quote.rooms.push(RoomQuote {
                floor: room.floor,
                name: display_name,
                area_m2,
                billed_area_m2: billed,
                material_name,
                material_rate,
                floor_cost,
                ceiling_cost,
                furniture_count,
                furniture_cost,
            });
        }

        // Furniture outside a room still belongs in the quote.
        for item in &doc.furniture {
            if item
                .room_id
                .is_some_and(|id| doc.rooms.iter().any(|r| r.id == id))
            {
                continue;
            }
            quote.furniture_count += 1;
            quote.furniture_total += item.effective_price();
            push_row(&mut rows, &item.name, item.effective_price());
        }
        // Items assigned to a room were already added to rows; add their totals.
        for room in &quote.rooms {
            quote.furniture_count += room.furniture_count;
            quote.furniture_total += room.furniture_cost;
        }
        quote.furniture = rows;

        for opening in &doc.openings {
            let price = opening.effective_price();
            match opening.kind {
                OpeningKind::Door => {
                    quote.door_count += 1;
                    quote.door_total += price;
                }
                OpeningKind::Window | OpeningKind::Sliding | OpeningKind::Bay => {
                    quote.window_count += 1;
                    quote.window_total += price;
                }
            }
        }
        quote.opening_total = quote.door_total + quote.window_total;

        // Wall finishes: gross wall face minus every door/window opening on
        // that wall, then the finish's own waste allowance. Walls without a
        // finish record fall back to a matching built-in wall material.
        for wall in &doc.walls {
            let material = wall
                .finish_id
                .and_then(|id| doc.materials.iter().find(|m| m.id == id))
                .or_else(|| {
                    let kind = legacy_wall_finish_kind(wall.finish);
                    doc.materials.iter().find(|m| {
                        m.surface == fp_model::FinishSurface::Wall && m.built_in && m.kind == kind
                    })
                });
            let Some(material) = material else {
                continue;
            };
            let len_m = ((wall.end.x - wall.start.x) as f64)
                .hypot((wall.end.y - wall.start.y) as f64)
                / 1000.0;
            let gross = len_m * wall.height_mm as f64 / 1000.0;
            let holes: f64 = doc
                .openings
                .iter()
                .filter(|o| o.wall_id == wall.id)
                .map(|o| o.width_mm.max(0) as f64 * o.height_mm.max(0) as f64 / 1_000_000.0)
                .sum();
            let net = (gross - holes).max(0.0);
            let billed = net * (1.0 + material.waste_rate);
            let amount = billed * material.price_per_m2;
            quote.wall_net_area += net;
            quote.wall_billed_area += billed;
            quote.wall_material += amount;
            push_finish_row(
                &mut quote.finish_rows,
                "墙面",
                &material.name,
                net,
                billed,
                material.price_per_m2,
            );
        }
        quote.finish_material = quote.wall_material + quote.ceiling_material;

        let wall_floors: Vec<(fp_core::EntityId, u32)> =
            doc.walls.iter().map(|wall| (wall.id, wall.floor)).collect();
        let mut level_openings = vec![0.0f64; doc.level_count()];
        let mut level_wall_finish = vec![0.0f64; doc.level_count()];
        for opening in &doc.openings {
            if let Some((_, floor)) = wall_floors.iter().find(|(id, _)| *id == opening.wall_id) {
                if let Some(slot) = level_openings.get_mut(*floor as usize) {
                    *slot += opening.effective_price();
                }
            }
        }
        for wall in &doc.walls {
            let material = wall
                .finish_id
                .and_then(|id| doc.materials.iter().find(|m| m.id == id))
                .or_else(|| {
                    let kind = legacy_wall_finish_kind(wall.finish);
                    doc.materials.iter().find(|m| {
                        m.surface == fp_model::FinishSurface::Wall && m.built_in && m.kind == kind
                    })
                });
            let Some(material) = material else {
                continue;
            };
            let len_m = ((wall.end.x - wall.start.x) as f64)
                .hypot((wall.end.y - wall.start.y) as f64)
                / 1000.0;
            let gross = len_m * wall.height_mm as f64 / 1000.0;
            let holes: f64 = doc
                .openings
                .iter()
                .filter(|o| o.wall_id == wall.id)
                .map(|o| o.width_mm.max(0) as f64 * o.height_mm.max(0) as f64 / 1_000_000.0)
                .sum();
            let net = (gross - holes).max(0.0);
            if let Some(slot) = level_wall_finish.get_mut(wall.floor as usize) {
                *slot += net * (1.0 + material.waste_rate) * material.price_per_m2;
            }
        }

        for index in 0..doc.level_count() {
            let mut level = LevelQuote {
                floor: index as u32,
                name: doc.level_name(index),
                room_count: 0,
                net_area_m2: 0.0,
                billed_area_m2: 0.0,
                floor_cost: 0.0,
                ceiling_cost: 0.0,
                finish_cost: 0.0,
                furniture_count: 0,
                furniture_cost: 0.0,
                opening_total: level_openings[index],
                total: 0.0,
            };
            for room in &quote.rooms {
                if room.floor as usize != index {
                    continue;
                }
                level.room_count += 1;
                level.net_area_m2 += room.area_m2;
                level.billed_area_m2 += room.billed_area_m2;
                level.floor_cost += room.floor_cost;
                level.ceiling_cost += room.ceiling_cost;
                level.furniture_count += room.furniture_count;
                level.furniture_cost += room.furniture_cost;
            }
            level.finish_cost =
                level.ceiling_cost + level_wall_finish.get(index).copied().unwrap_or(0.0);
            level.total =
                level.floor_cost + level.finish_cost + level.furniture_cost + level.opening_total;
            quote.levels.push(level);
        }

        quote.labor = quote.labor_area * doc.quote.labor_per_m2;
        quote.auxiliary = quote.labor_area * doc.quote.auxiliary_per_m2;
        quote.subtotal = quote.floor_material
            + quote.finish_material
            + quote.furniture_total
            + quote.opening_total
            + quote.labor
            + quote.auxiliary;
        quote.discount = quote.subtotal * doc.quote.discount_rate.clamp(0.0, 1.0);
        quote.taxable = quote.subtotal - quote.discount;
        quote.tax = quote.taxable * doc.quote.tax_rate.clamp(0.0, 1.0);
        quote.total = quote.taxable + quote.tax;
        quote
    }
}

fn legacy_wall_finish_kind(finish: fp_model::WallFinish) -> &'static str {
    match finish {
        fp_model::WallFinish::Paint => "paint",
        fp_model::WallFinish::Wallpaper => "wallpaper",
        fp_model::WallFinish::WoodPanel => "wood_panel",
        fp_model::WallFinish::Tile => "tile",
    }
}

#[allow(clippy::too_many_arguments)]
fn push_finish_row(
    rows: &mut Vec<FinishRow>,
    surface: &str,
    name: &str,
    area_m2: f64,
    billed_area_m2: f64,
    unit_price: f64,
) {
    if let Some(row) = rows
        .iter_mut()
        .find(|row| row.surface == surface && row.name == name && (row.unit_price - unit_price).abs() < 0.01)
    {
        row.area_m2 += area_m2;
        row.billed_area_m2 += billed_area_m2;
        row.amount += billed_area_m2 * unit_price;
    } else {
        rows.push(FinishRow {
            surface: surface.to_string(),
            name: name.to_string(),
            area_m2,
            billed_area_m2,
            unit_price,
            amount: billed_area_m2 * unit_price,
        });
    }
}

fn push_row(rows: &mut Vec<ItemRow>, name: &str, price: f64) {
    if let Some(row) = rows
        .iter_mut()
        .find(|row| row.name == name && (row.unit_price - price).abs() < 0.01)
    {
        row.count += 1;
        row.amount += price;
    } else {
        rows.push(ItemRow {
            name: name.to_string(),
            count: 1,
            unit_price: price,
            amount: price,
        });
    }
}

/// `¥12,345` with no decimals; quotes are customer-facing summaries.
pub fn money(value: f64) -> String {
    let rounded = value.round() as i64;
    let sign = if rounded < 0 { "-" } else { "" };
    let digits = rounded.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 2);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("¥{sign}{out}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fp_core::{EntityId, Pt};
    use fp_model::{Furniture, Level, Material, Opening, Room, Wall, WallKind};

    fn doc() -> Document {
        let mut doc = Document::new("quote");
        doc.materials.push(Material {
            id: EntityId(10),
            name: "橡木地板".into(),
            price_per_m2: 320.0,
            kind: "wood".into(),
            color: String::new(),
            surface: fp_model::FinishSurface::Floor,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
        });
        doc.rooms.push(Room {
            id: EntityId(1),
            polygon: vec![
                Pt::new(0, 0),
                Pt::new(4000, 0),
                Pt::new(4000, 3000),
                Pt::new(0, 3000),
            ],
            name: "客厅".into(),
            material_id: Some(EntityId(10)),
            counted: true,
            floor: 0,
            ceiling_material_id: None,
        });
        doc.furniture.push(Furniture {
            id: EntityId(2),
            asset_id: "sofa".into(),
            name: "三人沙发".into(),
            center: Pt::new(2000, 1500),
            width_mm: 2200,
            depth_mm: 950,
            rotation_deg: 0.0,
            color: "#8B6F55".into(),
            room_id: Some(EntityId(1)),
            price: None,
            floor: 0,
        });
        doc.walls.push(Wall::new(
            EntityId(3),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            200,
            WallKind::Exterior,
        ));
        doc.openings.push(Opening {
            id: EntityId(4),
            wall_id: EntityId(3),
            kind: OpeningKind::Door,
            offset_mm: 0,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 90.0,
            hinge_right: false,
            price: None,
        });
        doc
    }

    #[test]
    fn totals_include_every_category() {
        let quote = Quote::compute(&doc());
        assert!((quote.floor_net_area - 12.0).abs() < 1e-6);
        assert!((quote.floor_billed_area - 12.6).abs() < 1e-6);
        assert!((quote.floor_material - 4032.0).abs() < 1e-6);
        assert_eq!(quote.furniture_count, 1);
        assert!((quote.furniture_total - 3800.0).abs() < 1e-6);
        assert_eq!(quote.door_count, 1);
        assert!((quote.door_total - 1800.0).abs() < 1e-6);
        assert!((quote.labor - 1440.0).abs() < 1e-6);
        assert!((quote.auxiliary - 720.0).abs() < 1e-6);
        let expected = 4032.0 + 3800.0 + 1800.0 + 1440.0 + 720.0;
        assert!((quote.subtotal - expected).abs() < 1e-6);
        assert!((quote.total - expected * 1.09).abs() < 1e-6);
    }

    #[test]
    fn explicit_price_overrides_suggestion() {
        let mut d = doc();
        d.furniture[0].price = Some(0.0);
        let quote = Quote::compute(&d);
        assert!((quote.furniture_total - 0.0).abs() < 1e-6);
    }

    #[test]
    fn rooms_without_a_material_use_the_primary_flooring() {
        let mut d = doc();
        d.rooms[0].material_id = None;
        let quote = Quote::compute(&d);
        assert!((quote.floor_material - 4032.0).abs() < 1e-6);
    }

    #[test]
    fn discount_reduces_the_taxable_subtotal() {
        let mut d = doc();
        d.quote.discount_rate = 0.1;
        let quote = Quote::compute(&d);
        let subtotal = quote.subtotal;
        assert!((quote.discount - subtotal * 0.1).abs() < 1e-6);
        assert!((quote.taxable - subtotal * 0.9).abs() < 1e-6);
        assert!((quote.total - subtotal * 0.9 * 1.09).abs() < 1e-6);
    }

    #[test]
    fn levels_roll_up_rooms_furniture_and_openings_by_floor() {
        let mut d = doc();
        d.levels.push(Level {
            name: "2F".into(),
            height_mm: 2800,
        });
        d.rooms.push(Room {
            id: EntityId(5),
            polygon: vec![
                Pt::new(0, 5000),
                Pt::new(4000, 5000),
                Pt::new(4000, 7500),
                Pt::new(0, 7500),
            ],
            floor: 1,
            counted: true,
            name: "二层卧室".into(),
            material_id: Some(EntityId(10)),
            ceiling_material_id: None,
        });
        let mut extra = d.furniture[0].clone();
        extra.id = EntityId(6);
        extra.center = Pt::new(2000, 6000);
        extra.room_id = Some(EntityId(5));
        extra.floor = 1;
        d.furniture.push(extra);
        let quote = Quote::compute(&d);
        assert_eq!(quote.levels.len(), 2);
        assert!((quote.levels[0].net_area_m2 - 12.0).abs() < 1e-6);
        assert!((quote.levels[0].floor_cost - 4032.0).abs() < 1e-6);
        assert!((quote.levels[0].opening_total - 1800.0).abs() < 1e-6);
        assert!((quote.levels[1].net_area_m2 - 10.0).abs() < 1e-6);
        assert_eq!(quote.levels[1].room_count, 1);
        assert!((quote.levels[1].opening_total - 0.0).abs() < 1e-6);
        assert_eq!(quote.levels[1].furniture_count, 1);
        let direct = quote.floor_material + quote.furniture_total + quote.opening_total;
        let by_level: f64 = quote.levels.iter().map(|l| l.total).sum();
        assert!((by_level - direct).abs() < 1e-6);
    }

    #[test]
    fn wall_and_ceiling_finishes_are_billed() {
        let mut d = doc();
        d.materials.push(Material {
            id: EntityId(20),
            name: "墙面乳胶漆".into(),
            price_per_m2: 45.0,
            kind: "paint".into(),
            color: String::new(),
            surface: fp_model::FinishSurface::Wall,
            waste_rate: 0.08,
            built_in: true,
        });
        d.materials.push(Material {
            id: EntityId(21),
            name: "顶面乳胶漆".into(),
            price_per_m2: 38.0,
            kind: "paint".into(),
            color: String::new(),
            surface: fp_model::FinishSurface::Ceiling,
            waste_rate: 0.03,
            built_in: true,
        });
        d.walls[0].finish_id = Some(EntityId(20));
        d.rooms[0].ceiling_material_id = Some(EntityId(21));
        let quote = Quote::compute(&d);

        // 4.0 × 2.8 m wall, minus the 0.9 × 2.1 m door.
        let net = 4.0 * 2.8 - 0.9 * 2.1;
        assert!((quote.wall_net_area - net).abs() < 1e-6);
        assert!((quote.wall_billed_area - net * 1.08).abs() < 1e-6);
        assert!((quote.wall_material - net * 1.08 * 45.0).abs() < 1e-6);
        // 12 m² ceiling × 1.03 × ¥38.
        assert!((quote.ceiling_material - 12.0 * 1.03 * 38.0).abs() < 1e-6);
        assert!((quote.finish_material - (quote.wall_material + quote.ceiling_material)).abs() < 1e-6);
        assert!(quote.finish_rows.iter().any(|r| r.surface == "墙面" && r.name == "墙面乳胶漆"));
        assert!(quote.finish_rows.iter().any(|r| r.surface == "顶面"));
        let expected = quote.floor_material
            + quote.finish_material
            + quote.furniture_total
            + quote.opening_total
            + quote.labor
            + quote.auxiliary;
        assert!((quote.subtotal - expected).abs() < 1e-6);
    }

    #[test]
    fn money_groups_thousands() {
        assert_eq!(money(12345.4), "¥12,345");
        assert_eq!(money(0.0), "¥0");
    }
}
