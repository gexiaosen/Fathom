//! Floor-plan image import: decode, detect straight wall bands and build a
//! normal editable document. Recognition is intentionally conservative:
//! only thick horizontal/vertical strokes become walls, so dimension chains,
//! text and door arcs are ignored. The image stays visible as an underlay for
//! correction by hand.

use std::path::{Path, PathBuf};

use fp_core::{EntityId, Pt};
use fp_geom::detect_rooms;
use fp_model::{Document, Underlay, Wall, WallKind};

pub struct ImportOutcome {
    pub doc: Document,
    pub recognized_walls: usize,
}

#[derive(Clone, Copy, Debug)]
struct Bounds {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

#[derive(Clone, Copy, Debug)]
struct Band {
    /// True = horizontal wall (primary axis is image row).
    horizontal: bool,
    /// Primary axis: image row for horizontal walls, column for vertical.
    a0: usize,
    a1: usize,
    /// Along-wall span.
    b0: usize,
    b1: usize,
}

pub fn import_floorplan(
    path: &Path,
    width_mm: i32,
    name: &str,
    recognize: bool,
) -> Result<ImportOutcome, String> {
    if !(1_000..=100_000).contains(&width_mm) {
        return Err("图纸宽度需在 1–100 米之间".into());
    }
    let decoded = image::open(path)
        .map_err(|e| format!("读取图片失败: {e}"))?
        .to_rgba8();
    let (w, h) = decoded.dimensions();
    if w < 40 || h < 40 {
        return Err("图片分辨率过低（至少 40×40）".into());
    }
    let rgba = decoded.into_raw();
    let threshold = otsu_threshold(&rgba);
    let full_bounds = content_bounds(&rgba, w, h).ok_or("图片里没有可识别的线条")?;
    // Crop the white paper margin before scaling so the underlay and detected
    // walls share one coordinate system and the plan fills the canvas.
    let cw = full_bounds.x1 - full_bounds.x0 + 1;
    let ch = full_bounds.y1 - full_bounds.y0 + 1;
    let mut cropped = Vec::with_capacity(cw * ch * 4);
    for y in full_bounds.y0..=full_bounds.y1 {
        let row = ((y * w as usize + full_bounds.x0) * 4) as usize;
        let len = cw * 4;
        cropped.extend_from_slice(&rgba[row..row + len]);
    }
    let bounds = Bounds {
        x0: 0,
        y0: 0,
        x1: cw - 1,
        y1: ch - 1,
    };
    let scale = width_mm as f32 / cw as f32;
    let walls = if recognize {
        detect_walls(&cropped, cw as u32, ch as u32, bounds, threshold, scale)
    } else {
        Vec::new()
    };

    let asset =
        persist_image(path, &cropped, cw as u32, ch as u32).unwrap_or_else(|_| path.to_path_buf());
    let underlay = Underlay::new(
        asset.to_string_lossy().to_string(),
        0,
        0,
        width_mm,
        (ch as f32 * scale).round().max(1.0) as i32,
    );

    let mut doc = Document::new(name);
    doc.walls = walls;
    let wall_segments: Vec<(Pt, Pt)> = doc.walls.iter().map(|w| (w.start, w.end)).collect();
    for (i, polygon) in detect_rooms(&wall_segments, 10, 0.3)
        .into_iter()
        .enumerate()
    {
        let id = EntityId(doc.next_id());
        doc.rooms.push(fp_model::Room {
            id,
            polygon,
            name: format!("房间 {}", i + 1),
            material_id: None,
            counted: true,
        });
    }
    let recognized_walls = doc.walls.len();
    doc.underlay = Some(underlay);
    Ok(ImportOutcome {
        doc,
        recognized_walls,
    })
}

/// Copy the reference image into the user's asset directory as PNG, so the
/// document keeps working even if the original download is moved.
fn persist_image(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "找不到用户目录".to_string())?;
    let dir = PathBuf::from(home).join("Documents/Fathom/Assets");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem: String = path
        .file_stem()
        .map(|s| s.to_string_lossy().replace(['/', ' '], "-"))
        .unwrap_or_else(|| "plan".into());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let out = dir.join(format!("{stem}-{stamp}.png"));
    let image = image::RgbaImage::from_raw(w, h, rgba.to_vec())
        .ok_or_else(|| "图片缓冲区大小不匹配".to_string())?;
    image.save(&out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Otsu threshold keeps scans and grey pencil drawings usable without a fixed
/// cutoff tuned only for clean vector exports.
fn otsu_threshold(rgba: &[u8]) -> u8 {
    let mut hist = [0u32; 256];
    for px in rgba.as_chunks::<4>().0 {
        let l = luminance(px) as usize;
        hist[l] += 1;
    }
    let total: u32 = rgba.len() as u32 / 4;
    let mut sum = 0f64;
    for (i, count) in hist.iter().enumerate() {
        sum += i as f64 * *count as f64;
    }
    let mut sum_b = 0f64;
    let mut w_b = 0u32;
    let mut best = 0u8;
    let mut best_var = -1.0;
    for (i, count) in hist.iter().enumerate() {
        w_b += count;
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += i as f64 * *count as f64;
        let m_b = sum_b / w_b as f64;
        let m_f = (sum - sum_b) / w_f as f64;
        let var = w_b as f64 * w_f as f64 * (m_b - m_f).powi(2);
        if var > best_var {
            best_var = var;
            best = i as u8;
        }
    }
    best.clamp(30, 200)
}

fn luminance(px: &[u8]) -> u8 {
    ((0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32) * 1.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn content_bounds(rgba: &[u8], w: u32, h: u32) -> Option<Bounds> {
    let (mut x0, mut y0, mut x1, mut y1) = (w as usize, h as usize, 0usize, 0usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let px = &rgba[(y * w as usize + x) * 4..][..4];
            let l = luminance(px);
            if l < 235 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some(Bounds { x0, y0, x1, y1 })
    }
}

fn detect_walls(
    rgba: &[u8],
    w: u32,
    h: u32,
    bounds: Bounds,
    threshold: u8,
    scale: f32,
) -> Vec<Wall> {
    let content_w = (bounds.x1 - bounds.x0 + 1) as f32;
    let content_h = (bounds.y1 - bounds.y0 + 1) as f32;
    let min_len = (content_w.max(content_h) * 0.08).max(14.0) as usize;
    let horizontal = scan_bands(rgba, w, h, true, threshold, min_len);
    let vertical = scan_bands(rgba, w, h, false, threshold, min_len);
    let mut bands = merge_double_lines(horizontal);
    bands.extend(merge_double_lines(vertical));

    let mut walls = Vec::new();
    let mut next = 1u128;
    for band in bands {
        let a_center = (band.a0 + band.a1) as f32 * 0.5;
        let b0 = band.b0 as f32;
        let b1 = (band.b1 + 1) as f32;
        let (start, end, thickness_px) = if band.is_horizontal() {
            (
                Pt::new(snap(b0 * scale), snap(a_center * scale)),
                Pt::new(snap(b1 * scale), snap(a_center * scale)),
                (band.a1 - band.a0 + 1) as f32,
            )
        } else {
            (
                Pt::new(snap(a_center * scale), snap(b0 * scale)),
                Pt::new(snap(a_center * scale), snap(b1 * scale)),
                (band.a1 - band.a0 + 1) as f32,
            )
        };
        let dx = (end.x - start.x) as f64;
        let dy = (end.y - start.y) as f64;
        if dx.hypot(dy) < 250.0 {
            continue;
        }
        let touches_edge = band.b0 as f32 <= bounds_along_start(bounds, band)
            || band.b1 as f32 >= bounds_along_end(bounds, band);
        let kind = if touches_edge {
            WallKind::Exterior
        } else {
            WallKind::Partition
        };
        let detected = (thickness_px * scale).round() as i32;
        let thickness = if kind == WallKind::Exterior {
            detected.clamp(160, 240)
        } else {
            detected.clamp(80, 200)
        };
        let id = EntityId(next);
        next += 1;
        walls.push(Wall::new(id, start, end, thickness, kind));
    }
    walls
}

fn bounds_along_start(bounds: Bounds, band: Band) -> f32 {
    if band.is_horizontal() {
        bounds.x0 as f32
    } else {
        bounds.y0 as f32
    }
}

fn bounds_along_end(bounds: Bounds, band: Band) -> f32 {
    if band.is_horizontal() {
        bounds.x1 as f32
    } else {
        bounds.y1 as f32
    }
}

fn snap(v: f32) -> i32 {
    ((v / 10.0).round() * 10.0) as i32
}

impl Band {
    fn is_horizontal(&self) -> bool {
        self.horizontal
    }
}

fn scan_bands(
    rgba: &[u8],
    w: u32,
    h: u32,
    horizontal: bool,
    threshold: u8,
    min_len: usize,
) -> Vec<Band> {
    let (a_max, _b_max) = if horizontal {
        (h as usize, w as usize)
    } else {
        (w as usize, h as usize)
    };
    let mut finished = Vec::new();
    let mut active: Vec<Band> = Vec::new();
    for a in 0..a_max {
        let runs = line_runs(rgba, w, h, horizontal, a, threshold, min_len);
        for (b0, b1) in runs {
            let hit = active.iter_mut().find(|band| {
                band.a1 + 1 >= a
                    && overlap(b0, b1, band.b0, band.b1)
                        >= (b1 - b0 + 1).min(band.b1 - band.b0 + 1) / 2
            });
            match hit {
                Some(band) => {
                    band.a1 = a;
                    band.b0 = band.b0.min(b0);
                    band.b1 = band.b1.max(b1);
                }
                None => active.push(Band {
                    horizontal,
                    a0: a,
                    a1: a,
                    b0,
                    b1,
                }),
            }
        }
        let mut keep = Vec::new();
        for band in active {
            if band.a1 + 1 >= a {
                keep.push(band);
            } else if band.a1 + 1 >= band.a0 && band.a1 > band.a0 {
                finished.push(band);
            }
        }
        active = keep;
    }
    finished.extend(active.into_iter().filter(|b| b.a1 > b.a0));
    finished
}

fn line_runs(
    rgba: &[u8],
    w: u32,
    h: u32,
    horizontal: bool,
    primary: usize,
    threshold: u8,
    min_len: usize,
) -> Vec<(usize, usize)> {
    let secondary_max = if horizontal { w as usize } else { h as usize };
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    let mut last_dark = 0usize;
    for b in 0..secondary_max {
        let (x, y) = if horizontal {
            (b, primary)
        } else {
            (primary, b)
        };
        let px = &rgba[(y * w as usize + x) * 4..][..4];
        let dark = luminance(px) <= threshold;
        if dark {
            if start.is_none() {
                start = Some(b);
            }
            last_dark = b;
        } else if let Some(s) = start {
            // Let 2 px of anti-aliasing/scan noise pass, but not a real gap.
            if b > last_dark + 2 {
                if last_dark + 1 >= s + min_len {
                    runs.push((s, last_dark));
                }
                start = None;
            }
        }
    }
    if let Some(s) = start {
        let long_enough = last_dark + 1 >= s + min_len;
        if long_enough {
            runs.push((s, last_dark));
        }
    }
    runs
}

fn overlap(a0: usize, a1: usize, b0: usize, b1: usize) -> usize {
    let lo = a0.max(b0);
    let hi = a1.min(b1);
    hi.saturating_sub(lo)
}

/// Hollow double-line wall drawings produce two parallel bands a few pixels
/// apart. Merge overlapping neighbours into one centreline.
fn merge_double_lines(bands: Vec<Band>) -> Vec<Band> {
    let mut out: Vec<Band> = Vec::new();
    for band in bands {
        if let Some(existing) = out.iter_mut().find(|other| {
            other.horizontal == band.horizontal
                && {
                    let center_gap = ((band.a0 + band.a1) as f32 * 0.5
                        - (other.a0 + other.a1) as f32 * 0.5)
                        .abs();
                    center_gap <= 14.0
                }
                && overlap(band.b0, band.b1, other.b0, other.b1)
                    >= (band.b1 - band.b0 + 1).min(other.b1 - other.b0 + 1) * 2 / 3
        }) {
            existing.a0 = existing.a0.min(band.a0);
            existing.a1 = existing.a1.max(band.a1);
            existing.b0 = existing.b0.min(band.b0);
            existing.b1 = existing.b1.max(band.b1);
        } else {
            out.push(band);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: u32, h: u32) -> Vec<u8> {
        vec![255u8; (w * h * 4) as usize]
    }

    fn line(img: &mut [u8], w: u32, x0: u32, y0: u32, x1: u32, y1: u32, thick: u32) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for dy in 0..thick {
                    for dx in 0..thick {
                        let px = ((y + dy) * w + x + dx) as usize * 4;
                        if px + 3 < img.len() {
                            img[px..px + 3].fill(0);
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "manual check against a real plan image"]
    fn real_plan_preview_recognizes_walls() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/plan-sample-pdf.png");
        let img = image::open(&path).expect("plan image").to_rgba8();
        let (w, h) = img.dimensions();
        let rgba = img.into_raw();
        let threshold = otsu_threshold(&rgba);
        let bounds = content_bounds(&rgba, w, h).expect("content");
        let scale = 12000.0 / w as f32;
        let walls = detect_walls(&rgba, w, h, bounds, threshold, scale);
        eprintln!(
            "{w}x{h} threshold={threshold} bounds={bounds:?} walls={}",
            walls.len()
        );
        for w in walls.iter().take(40) {
            eprintln!("{:?} -> {:?} t={}", w.start, w.end, w.thickness_mm);
        }
        assert!(!walls.is_empty(), "real plan should yield walls");
    }

    #[test]
    fn finds_two_thick_walls_and_ignores_text() {
        let (w, h) = (120u32, 90u32);
        let mut img = canvas(w, h);
        line(&mut img, w, 10, 20, 100, 20, 5);
        line(&mut img, w, 55, 10, 55, 80, 5);
        // A dimension line and a small text blob must not become walls.
        line(&mut img, w, 10, 50, 100, 50, 1);
        line(&mut img, w, 20, 70, 28, 70, 2);
        let bounds = content_bounds(&img, w, h).unwrap();
        let walls = detect_walls(&img, w, h, bounds, 128, 100.0);
        assert_eq!(walls.len(), 2, "{walls:#?}");
        let lengths: Vec<f64> = walls
            .iter()
            .map(|w| ((w.end.x - w.start.x) as f64).hypot((w.end.y - w.start.y) as f64))
            .collect();
        assert!(lengths.iter().any(|l| (*l - 9450.0).abs() < 100.0));
    }

    #[test]
    fn merges_a_hollow_double_line() {
        let (w, h) = (80u32, 60u32);
        let mut img = canvas(w, h);
        line(&mut img, w, 10, 20, 70, 20, 2);
        line(&mut img, w, 10, 28, 70, 28, 2);
        let bounds = content_bounds(&img, w, h).unwrap();
        let walls = detect_walls(&img, w, h, bounds, 200, 100.0);
        assert_eq!(walls.len(), 1, "double outline becomes one centreline");
        let y = walls[0].start.y;
        assert!((y - 2400).abs() < 100, "centre y {y}");
    }
}
