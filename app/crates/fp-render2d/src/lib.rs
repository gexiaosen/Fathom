//! 2D plan rendering: camera, grid and screen-space geometry batches.
//!
//! Everything here is pure CPU so it can be unit tested and reused by both the
//! GPUI view and the wgpu renderer.

mod batch;
pub mod camera;
mod furniture;
pub mod grid;
mod openings;
mod rooms;

pub use batch::{
    Batch, Color, QuadVertex, build_wall_batch, build_wall_batch_visible, build_wall_finish_batch,
};
pub use camera::Camera2D;
pub use furniture::{build_furniture_batch, corners, hit};
pub use grid::{GridLines, build_grid};
pub use openings::{DOOR_COLOR, WINDOW_COLOR, build_opening_batch};
pub use rooms::{FILL_ALPHA, SELECTED_ALPHA, build_room_fills, material_color};
