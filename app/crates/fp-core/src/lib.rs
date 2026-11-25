//! Shared primitives for Floorplan-3D: units, IDs and core error types.

use serde::{Deserialize, Serialize};

/// All internal geometry uses millimetres as signed integers.
pub type Mm = i32;

/// A point in document space, in millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pt {
    pub x: Mm,
    pub y: Mm,
}

impl Pt {
    pub const ZERO: Self = Self { x: 0, y: 0 };

    pub const fn new(x: Mm, y: Mm) -> Self {
        Self { x, y }
    }
}

/// A stable identifier for document entities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityId(pub u128);

impl EntityId {
    pub fn from_raw(raw: u128) -> Self {
        Self(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_is_millimetre_based() {
        let p = Pt::new(1200, -350);
        assert_eq!(p.x, 1200);
        assert_eq!(p.y, -350);
    }
}
