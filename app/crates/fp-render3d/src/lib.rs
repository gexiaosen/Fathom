//! 3D scene construction: wall boxes, room floors and an orbit camera.

mod camera3d;
mod furniture;
mod mesh;

pub use camera3d::{EYE_HEIGHT, MAX_DISTANCE, MIN_DISTANCE, OrbitCamera, WalkCamera};
pub use mesh::{
    CEILING_COLOR, FLOOR_FALLBACK, FURNITURE_FALLBACK, MeshParts, MeshVertex, WALL_COLOR,
    ceiling_mesh, ceiling_mesh_with_materials, door_leaf_mesh, floor_mesh, furniture_mesh,
    furniture_mesh_parts, ground_mesh, opening_glass_mesh, roof_mesh, roof_mesh_for, scene_mesh,
    scene_mesh_full, scene_mesh_with_openings, stair_mesh, wall_mesh, wall_mesh_with_finishes,
    wall_mesh_with_openings,
};
