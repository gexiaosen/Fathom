//! Adaptive plan grid: minor lines pick a step that stays legible at any zoom.

use fp_core::Pt;

use crate::camera::Camera2D;

/// Candidate grid steps in millimetres (10 mm … 50 m).
const STEPS: [f64; 13] = [
    10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2000.0, 2500.0, 5000.0, 10_000.0, 25_000.0,
    50_000.0,
];

/// Never emit more than this many lines per axis.
const MAX_LINES_PER_AXIS: i64 = 220;

#[derive(Clone, Debug, Default)]
pub struct GridLines {
    /// Grid step actually used, in millimetres.
    pub step_mm: f64,
    pub minor: Vec<[Pt; 2]>,
    pub major: Vec<[Pt; 2]>,
}

impl GridLines {
    pub fn total(&self) -> usize {
        self.minor.len() + self.major.len()
    }
}

/// Build grid lines covering the visible world box.
///
/// `target_minor_px` is the smallest on-screen spacing we accept for minor
/// lines; the step grows until it fits.
pub fn build_grid(cam: &Camera2D, target_minor_px: f64) -> GridLines {
    let [min_x, min_y, max_x, max_y] = cam.world_bounds();
    let mut step = *STEPS.last().expect("step table is non-empty");
    for candidate in STEPS {
        if candidate * cam.scale >= target_minor_px {
            step = candidate;
            break;
        }
    }
    // Push the step up until both axes fit inside the line budget.
    let mut guard = 0;
    while guard < STEPS.len() && spans(min_x, max_x, step) > MAX_LINES_PER_AXIS {
        step *= 2.0;
        guard += 1;
    }

    let mut out = GridLines {
        step_mm: step,
        ..GridLines::default()
    };

    let i0 = (min_x / step).floor() as i64;
    let i1 = (max_x / step).ceil() as i64;
    for i in i0..=i1 {
        let x = (i as f64 * step) as i32;
        let line = [Pt::new(x, min_y as i32), Pt::new(x, max_y as i32)];
        if i % 5 == 0 {
            out.major.push(line);
        } else {
            out.minor.push(line);
        }
    }

    let j0 = (min_y / step).floor() as i64;
    let j1 = (max_y / step).ceil() as i64;
    for j in j0..=j1 {
        let y = (j as f64 * step) as i32;
        let line = [Pt::new(min_x as i32, y), Pt::new(max_x as i32, y)];
        if j % 5 == 0 {
            out.major.push(line);
        } else {
            out.minor.push(line);
        }
    }

    out
}

fn spans(min: f64, max: f64, step: f64) -> i64 {
    ((max / step).ceil() - (min / step).floor()) as i64 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_grows_with_legibility_target() {
        let mut cam = Camera2D::new([1000.0, 800.0]);
        cam.scale = 0.05; // 1 px = 20 mm
        let g = build_grid(&cam, 8.0);
        assert!(
            g.step_mm * cam.scale >= 8.0,
            "step {} too small at scale {}",
            g.step_mm,
            cam.scale
        );
        assert_eq!(g.step_mm, 250.0);
    }

    #[test]
    fn lines_cover_the_view_and_stay_within_budget() {
        let mut cam = Camera2D::new([1600.0, 1000.0]);
        cam.center = [4000.0, 3000.0];
        cam.scale = 0.02;
        let g = build_grid(&cam, 8.0);
        let bounds = cam.world_bounds();
        let (min_x, max_x) = (bounds[0], bounds[2]);
        let xs: Vec<i32> = g
            .minor
            .iter()
            .chain(g.major.iter())
            .filter(|l| l[0].x == l[1].x)
            .map(|l| l[0].x)
            .collect();
        assert!(xs.iter().any(|&x| (x as f64) <= min_x));
        assert!(xs.iter().any(|&x| (x as f64) >= max_x));
        assert!(
            g.total() <= (MAX_LINES_PER_AXIS * 2) as usize,
            "{}",
            g.total()
        );
    }

    #[test]
    fn major_every_fifth_line() {
        let mut cam = Camera2D::new([1000.0, 800.0]);
        cam.scale = 0.05;
        let g = build_grid(&cam, 8.0);
        assert!(!g.major.is_empty());
        for line in &g.minor {
            let v = if line[0].x == line[1].x {
                line[0].x
            } else {
                line[0].y
            };
            assert_ne!(v as f64 % (g.step_mm * 5.0), 0.0);
        }
        for line in &g.major {
            let v = if line[0].x == line[1].x {
                line[0].x
            } else {
                line[0].y
            };
            assert_eq!(v as f64 % (g.step_mm * 5.0), 0.0);
        }
    }
}
