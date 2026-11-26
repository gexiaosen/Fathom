//! Soft room fills tinted by the flooring material, drawn under the walls.

use fp_core::EntityId;
use fp_model::catalog::{material_color as swatch, material_kind_color};
use fp_model::{Material, Room};

use crate::batch::{Batch, Color, QuadVertex};
use crate::camera::Camera2D;

/// Base tint per flooring kind; unknown kinds fall back to neutral grey.
pub fn material_color(materials: &[Material], room: &Room) -> Color {
    let c = room
        .material_id
        .and_then(|id| materials.iter().find(|m| m.id == id))
        .map(swatch)
        .unwrap_or_else(|| material_kind_color(""));
    Color([c[0], c[1], c[2], 1.0])
}

pub const FILL_ALPHA: f32 = 0.30;
pub const SELECTED_ALPHA: f32 = 0.45;

/// Fill every room polygon and outline the selected one.
pub fn build_room_fills(
    rooms: &[Room],
    materials: &[Material],
    cam: &Camera2D,
    selected: Option<EntityId>,
) -> Batch {
    let mut batch = Batch::default();
    for room in rooms {
        let base = material_color(materials, room);
        let alpha = if selected == Some(room.id) {
            SELECTED_ALPHA
        } else {
            FILL_ALPHA
        };
        let color = base.with_alpha(alpha);
        for tri in fp_geom::triangulate(&room.polygon) {
            for p in tri {
                batch.vertices.push(QuadVertex {
                    pos: cam.world_to_screen(p),
                    color: color.0,
                });
            }
        }
        if selected == Some(room.id) {
            for w in room.polygon.windows(2) {
                batch.line(cam, w[0], w[1], 2.0, Color::SELECTED);
            }
            if let (Some(first), Some(last)) = (room.polygon.first(), room.polygon.last()) {
                batch.line(cam, *last, *first, 2.0, Color::SELECTED);
            }
        }
    }
    batch
}

#[cfg(test)]
mod tests {
    use super::*;
    use fp_core::Pt;

    fn square_room(material_id: Option<u128>) -> Room {
        Room {
            id: EntityId(7),
            polygon: vec![
                Pt::new(0, 0),
                Pt::new(4000, 0),
                Pt::new(4000, 3000),
                Pt::new(0, 3000),
            ],
            name: "客厅".into(),
            material_id: material_id.map(EntityId),
            counted: true,
            floor: 0,
            ceiling_material_id: None,
        }
    }

    fn material(id: u128, kind: &str) -> Material {
        Material {
            id: EntityId(id),
            name: kind.into(),
            price_per_m2: 0.0,
            kind: kind.into(),
            color: String::new(),
            surface: fp_model::FinishSurface::Floor,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
        }
    }

    #[test]
    fn square_room_fills_with_two_triangles() {
        let cam = Camera2D::new([1000.0, 800.0]);
        let batch = build_room_fills(&[square_room(None)], &[], &cam, None);
        assert_eq!(batch.vertices.len(), 6, "quad -> 2 triangles");
    }

    #[test]
    fn material_tint_follows_kind() {
        let mats = vec![material(1, "wood")];
        let wood = material_color(&mats, &square_room(Some(1)));
        let bare = material_color(&[], &square_room(None));
        assert!(wood.0[0] > bare.0[0], "wood is warmer than the fallback");
    }

    #[test]
    fn custom_swatch_overrides_kind_tint() {
        let mut mats = vec![material(1, "wood")];
        mats[0].color = "#b4552d".into();
        let custom = material_color(&mats, &square_room(Some(1)));
        assert!(custom.0[0] > custom.0[1] && custom.0[1] > custom.0[2]);
        mats[0].color.clear();
        let kind = material_color(&mats, &square_room(Some(1)));
        assert_ne!(custom.0, kind.0);
    }

    #[test]
    fn selected_room_gets_an_outline() {
        let cam = Camera2D::new([1000.0, 800.0]);
        let plain = build_room_fills(&[square_room(None)], &[], &cam, None);
        let picked = build_room_fills(&[square_room(None)], &[], &cam, Some(EntityId(7)));
        assert!(picked.vertices.len() > plain.vertices.len());
    }
}
