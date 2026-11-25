//! Editor interaction logic: picking, selection and gestures.
//!
//! Pure functions so the same code drives the GPUI view and unit tests.

mod collision;
mod interact;
mod pick;
mod snap;

pub use collision::{
    WALL_CLEARANCE_MM, blocked_along_move, furniture_blocking_other, furniture_blocking_wall,
    furniture_hits_furniture, furniture_hits_wall, furniture_path_blocked, quads_penetrate,
    wall_corners,
};
pub use interact::{
    DEFAULT_GRID_MM, DragSession, MIN_WALL_LEN_MM, Tool, clamp_offset, constrained_end,
    is_valid_wall, opening_width, wall_offset_at,
};
pub use pick::{PICK_TOLERANCE_PX, hit_wall, walls_in_rect};
pub use snap::{FURNITURE_SNAP_MM, furniture_bounds, snap_furniture, snap_furniture_to_others};
