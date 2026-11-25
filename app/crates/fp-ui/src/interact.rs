//! Editing gestures: dragging a selection and drafting a wall.

use fp_core::{EntityId, Mm, Pt};
use fp_geom::snap_to_grid;
use fp_model::Wall;
use fp_render2d::Camera2D;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Wall,
    Door,
    Window,
    Furniture,
    Measure,
    /// Click a non-load-bearing wall to demolish it.
    Demolish,
}

/// Grid step used while drafting walls (10 mm keeps millimetre precision but
/// still feels snappy at plan zoom).
pub const DEFAULT_GRID_MM: Mm = 10;

/// A drag in progress. Wall origins are captured at pointer-down so the move is
/// absolute (never accumulates rounding).
#[derive(Clone, Debug)]
pub struct DragSession {
    start_screen: [f32; 2],
    origins: Vec<(EntityId, Pt, Pt)>,
}

impl DragSession {
    pub fn begin(walls: &[Wall], selection: &[EntityId], pointer: [f32; 2]) -> Option<Self> {
        if selection.is_empty() {
            return None;
        }
        let origins = walls
            .iter()
            .filter(|wall| selection.contains(&wall.id))
            .map(|wall| (wall.id, wall.start, wall.end))
            .collect::<Vec<_>>();
        if origins.is_empty() {
            return None;
        }
        Some(Self {
            start_screen: pointer,
            origins,
        })
    }

    pub fn delta_mm(&self, cam: &Camera2D, pointer: [f32; 2]) -> [Mm; 2] {
        let from = cam.screen_to_world_f64(self.start_screen);
        let to = cam.screen_to_world_f64(pointer);
        [
            (to[0] - from[0]).round() as Mm,
            (to[1] - from[1]).round() as Mm,
        ]
    }

    /// Move the captured walls to `origin + delta`. Returns `true` if the
    /// pointer travelled at least one millimetre (i.e. this was a drag, not a
    /// click).
    pub fn apply(&self, walls: &mut [Wall], cam: &Camera2D, pointer: [f32; 2]) -> bool {
        let delta = self.delta_mm(cam, pointer);
        for wall in walls.iter_mut() {
            if let Some((_, start, end)) = self.origins.iter().find(|(id, _, _)| *id == wall.id) {
                wall.start = Pt::new(start.x + delta[0], start.y + delta[1]);
                wall.end = Pt::new(end.x + delta[0], end.y + delta[1]);
            }
        }
        delta[0] != 0 || delta[1] != 0
    }

    pub fn ids(&self) -> Vec<EntityId> {
        self.origins.iter().map(|(id, _, _)| *id).collect()
    }

    pub fn origins(&self) -> &[(EntityId, Pt, Pt)] {
        &self.origins
    }
}

/// Default opening width for the active tool, in millimetres.
pub fn opening_width(tool: Tool) -> Mm {
    match tool {
        Tool::Door => 900,
        Tool::Window => 1200,
        _ => 0,
    }
}

/// Project a point onto the wall centreline, returning the distance from the
/// wall start in millimetres.
pub fn wall_offset_at(wall: &Wall, p: Pt) -> Mm {
    let dx = (wall.end.x - wall.start.x) as f64;
    let dy = (wall.end.y - wall.start.y) as f64;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1.0 {
        return 0;
    }
    let t = (((p.x - wall.start.x) as f64 * dx + (p.y - wall.start.y) as f64 * dy) / len_sq)
        .clamp(0.0, 1.0);
    (t * len_sq.sqrt()) as Mm
}

/// Clamp an opening so it stays inside the wall.
pub fn clamp_offset(wall: &Wall, offset: Mm, width: Mm) -> Mm {
    let len = ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64) as Mm;
    offset.clamp(0, (len - width).max(0))
}

/// Snap the wall end to the grid, optionally forcing it ortho to the start.
pub fn constrained_end(start: Pt, cursor: Pt, ortho: bool, grid: Mm) -> Pt {
    let mut end = Pt::new(snap_to_grid(cursor.x, grid), snap_to_grid(cursor.y, grid));
    if ortho {
        if (end.x - start.x).abs() >= (end.y - start.y).abs() {
            end.y = start.y;
        } else {
            end.x = start.x;
        }
    }
    end
}

/// A wall shorter than this is a mis-click, not a wall.
pub const MIN_WALL_LEN_MM: Mm = 50;

pub fn is_valid_wall(start: Pt, end: Pt) -> bool {
    ((end.x - start.x) as f64).hypot((end.y - start.y) as f64) >= MIN_WALL_LEN_MM as f64
}

#[cfg(test)]
mod tests {
    use fp_model::WallKind;

    use super::*;

    fn cam() -> Camera2D {
        let mut c = Camera2D::new([1000.0, 800.0]);
        c.scale = 0.05;
        c
    }

    fn walls() -> Vec<Wall> {
        vec![
            Wall::new(
                EntityId(1),
                Pt::new(0, 0),
                Pt::new(2000, 0),
                100,
                WallKind::Partition,
            ),
            Wall::new(
                EntityId(2),
                Pt::new(0, 1000),
                Pt::new(2000, 1000),
                100,
                WallKind::Partition,
            ),
        ]
    }

    #[test]
    fn drag_moves_only_the_selection() {
        let c = cam();
        let mut w = walls();
        let sel = [EntityId(1)];
        let s = DragSession::begin(&w, &sel, [100.0, 100.0]).unwrap();
        s.apply(&mut w, &c, [140.0, 120.0]);
        // 40 px → 800 mm, 20 px → -400 mm (screen y goes down)
        assert_eq!(w[0].start, Pt::new(800, -400));
        assert_eq!(w[0].end, Pt::new(2800, -400));
        assert_eq!(w[1].start, Pt::new(0, 1000), "unselected wall stays");
    }

    #[test]
    fn drag_is_absolute_not_cumulative() {
        let c = cam();
        let mut w = walls();
        let s = DragSession::begin(&w, &[EntityId(1)], [10.0, 10.0]).unwrap();
        s.apply(&mut w, &c, [30.0, 10.0]);
        s.apply(&mut w, &c, [50.0, 10.0]);
        s.apply(&mut w, &c, [30.0, 10.0]);
        assert_eq!(w[0].start, Pt::new(400, 0));
        assert_eq!(w[0].end, Pt::new(2400, 0));
    }

    #[test]
    fn click_without_movement_is_not_a_drag() {
        let c = cam();
        let mut w = walls();
        let s = DragSession::begin(&w, &[EntityId(1)], [0.0, 0.0]).unwrap();
        assert!(!s.apply(&mut w, &c, [0.0, 0.0]));
        assert!(s.apply(&mut w, &c, [2.0, 0.0]));
    }

    #[test]
    fn ortho_locks_the_dominant_axis() {
        let start = Pt::new(0, 0);
        let end = constrained_end(start, Pt::new(1234, 87), true, 10);
        assert_eq!(end, Pt::new(1230, 0));
        let end = constrained_end(start, Pt::new(42, 991), true, 10);
        assert_eq!(end, Pt::new(0, 990));
    }

    #[test]
    fn grid_snaps_without_ortho() {
        let start = Pt::new(0, 0);
        let end = constrained_end(start, Pt::new(1234, 87), false, 10);
        assert_eq!(end, Pt::new(1230, 90));
    }

    #[test]
    fn offset_projects_onto_the_wall() {
        let w = Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            100,
            WallKind::Partition,
        );
        assert_eq!(wall_offset_at(&w, Pt::new(2000, 500)), 2000);
        assert_eq!(wall_offset_at(&w, Pt::new(-100, 0)), 0);
        assert_eq!(wall_offset_at(&w, Pt::new(5000, 0)), 4000);
    }

    #[test]
    fn offset_is_clamped_inside_the_wall() {
        let w = Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            100,
            WallKind::Partition,
        );
        assert_eq!(clamp_offset(&w, 3500, 900), 3100);
        assert_eq!(clamp_offset(&w, -50, 900), 0);
    }

    #[test]
    fn tiny_drags_are_rejected_as_walls() {
        assert!(!is_valid_wall(Pt::new(0, 0), Pt::new(20, 0)));
        assert!(is_valid_wall(Pt::new(0, 0), Pt::new(1000, 0)));
    }
}
