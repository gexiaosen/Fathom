//! Floor-plan image import: decode, detect straight wall bands and build a
//! normal editable document. Recognition is intentionally conservative:
//! only thick horizontal/vertical strokes become walls, so dimension chains,
//! text and door arcs are ignored. The image stays visible as an underlay for
//! correction by hand.

use std::fs;
use std::path::{Path, PathBuf};

use fp_core::{EntityId, Pt};
use fp_geom::{ROOM_JOIN_TOLERANCE_MM, detect_rooms};
use fp_model::{Document, Opening, OpeningKind, Underlay, Wall, WallKind};

pub struct ImportOutcome {
    pub doc: Document,
    pub recognized_walls: usize,
    pub recognized_rooms: usize,
    pub recognized_doors: usize,
    pub preview_png: PathBuf,
    pub vector: bool,
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
    let recognized_doors = if recognize {
        merge_door_gaps(&mut doc)
    } else {
        0
    };
    let wall_segments: Vec<(Pt, Pt)> = doc.walls.iter().map(|w| (w.start, w.end)).collect();
    let default_material = doc
        .materials
        .iter()
        .find(|m| m.surface == fp_model::FinishSurface::Floor)
        .map(|m| m.id);
    let default_ceiling =
        crate::templates::built_in_finish_id(&doc, fp_model::FinishSurface::Ceiling, "paint");
    for (i, polygon) in detect_rooms(&wall_segments, ROOM_JOIN_TOLERANCE_MM, 0.3)
        .into_iter()
        .enumerate()
    {
        let id = EntityId(doc.next_id());
        doc.rooms.push(fp_model::Room {
            id,
            polygon,
            name: format!("房间 {}", i + 1),
            material_id: default_material,
            counted: true,
            floor: 0,
            ceiling_material_id: default_ceiling,
        });
    }
    let recognized_walls = doc.walls.len();
    let recognized_rooms = doc.rooms.len();
    doc.underlay = Some(underlay);
    let preview_png = save_raster_preview(&cropped, cw as u32, ch as u32, &doc, width_mm)
        .unwrap_or_else(|_| asset.clone());
    Ok(ImportOutcome {
        doc,
        recognized_walls,
        recognized_rooms,
        recognized_doors,
        preview_png,
        vector: false,
    })
}

/// True when the path is an ASCII DXF we can parse directly.
pub fn is_dxf(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("dxf"))
        .unwrap_or(false)
}

/// Import wall centrelines from an ASCII DXF. `LINE` and `LWPOLYLINE`
/// entities on wall layers become walls; dimension/text/furniture layers are
/// ignored. Units follow `$INSUNITS` (mm when the file is unitless).
pub fn import_dxf(path: &Path, name: &str) -> Result<ImportOutcome, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("读取 DXF 失败: {e}"))?;
    if text.starts_with("AutoCAD Binary DXF") {
        return Err("暂不支持二进制 DXF，请在 CAD 里另存为 ASCII DXF".into());
    }
    let pairs = dxf_pairs(&text);
    let mut unit_scale = dxf_unit_scale(&pairs);
    let segments = dxf_segments(&pairs, unit_scale);
    if segments.is_empty() {
        return Err("DXF 里没有可用的墙线（LINE / LWPOLYLINE）".into());
    }

    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for (a, b, _) in &segments {
        min_x = min_x.min(a.0).min(b.0);
        min_y = min_y.min(a.1).min(b.1);
        max_x = max_x.max(a.0).max(b.0);
        max_y = max_y.max(a.1).max(b.1);
    }
    let span_x = (max_x - min_x).max(1.0);
    let span_y = (max_y - min_y).max(1.0);
    // Many exports leave $INSUNITS unset while drawing in metres. A whole
    // plan that is only a few units across is metres, not millimetres.
    if unit_scale == 1.0 && span_x.max(span_y) < 100.0 {
        unit_scale = 1000.0;
    }

    let mut doc = Document::new(name);
    let mut next = 1u128;
    for (a, b, layer) in segments {
        if !layer.is_empty() {
            let lower = layer.to_ascii_lowercase();
            if [
                "dim", "text", "furn", "axis", "标注", "尺寸", "文字", "家具", "轴线",
            ]
            .iter()
            .any(|needle| lower.contains(needle))
            {
                continue;
            }
        }
        let start = Pt::new(
            ((a.0 - min_x) * unit_scale).round() as i32,
            ((max_y - a.1) * unit_scale).round() as i32,
        );
        let end = Pt::new(
            ((b.0 - min_x) * unit_scale).round() as i32,
            ((max_y - b.1) * unit_scale).round() as i32,
        );
        let length = ((end.x - start.x) as f64).hypot((end.y - start.y) as f64);
        if length < 500.0 {
            continue;
        }
        let edge = start.x.min(end.x) < 100
            || start.y.min(end.y) < 100
            || (span_x * unit_scale - start.x.max(end.x) as f64) < 100.0
            || (span_y * unit_scale - start.y.max(end.y) as f64) < 100.0;
        let thickness = if edge { 200 } else { 120 };
        let kind = if edge {
            WallKind::Exterior
        } else {
            WallKind::Partition
        };
        doc.walls
            .push(Wall::new(EntityId(next), start, end, thickness, kind));
        next += 1;
    }
    if doc.walls.is_empty() {
        return Err("DXF 里的线条都太短，未生成墙体".into());
    }
    let recognized_doors = merge_door_gaps(&mut doc);
    let wall_segments: Vec<(Pt, Pt)> = doc.walls.iter().map(|w| (w.start, w.end)).collect();
    let default_material = doc
        .materials
        .iter()
        .find(|m| m.surface == fp_model::FinishSurface::Floor)
        .map(|m| m.id);
    let default_ceiling =
        crate::templates::built_in_finish_id(&doc, fp_model::FinishSurface::Ceiling, "paint");
    for (i, polygon) in detect_rooms(&wall_segments, ROOM_JOIN_TOLERANCE_MM, 0.3)
        .into_iter()
        .enumerate()
    {
        let id = EntityId(doc.next_id());
        doc.rooms.push(fp_model::Room {
            id,
            polygon,
            name: format!("房间 {}", i + 1),
            material_id: default_material,
            counted: true,
            floor: 0,
            ceiling_material_id: default_ceiling,
        });
    }
    let recognized_walls = doc.walls.len();
    let recognized_rooms = doc.rooms.len();
    let preview_png = save_vector_preview(&doc).map_err(|e| format!("预览生成失败: {e}"))?;
    Ok(ImportOutcome {
        doc,
        recognized_walls,
        recognized_rooms,
        recognized_doors,
        preview_png,
        vector: true,
    })
}

type DxfPair = (i32, String);

fn dxf_pairs(text: &str) -> Vec<DxfPair> {
    let mut lines = text.lines();
    let mut pairs = Vec::new();
    while let Some(code_line) = lines.next() {
        let Ok(code) = code_line.trim().parse::<i32>() else {
            // Tolerate blank or malformed lines between code/value pairs.
            continue;
        };
        let Some(value_line) = lines.next() else {
            break;
        };
        pairs.push((code, value_line.trim().to_string()));
    }
    pairs
}

fn dxf_unit_scale(pairs: &[DxfPair]) -> f64 {
    let mut units = 0i32;
    for window in pairs.windows(2) {
        if window[0].0 == 9 && window[0].1.eq_ignore_ascii_case("$INSUNITS") {
            units = window[1].1.parse().unwrap_or(0);
            break;
        }
    }
    match units {
        1 => 25.4,   // inches
        2 => 304.8,  // feet
        4 => 1.0,    // millimetres
        5 => 10.0,   // centimetres
        6 => 1000.0, // metres
        _ => 1.0,    // unitless: assume millimetres
    }
}

/// Collect LINE and LWPOLYLINE segments. Coordinates stay in DXF units here;
/// the caller converts to millimetres.
fn dxf_segments(pairs: &[DxfPair], _unit_scale: f64) -> Vec<((f64, f64), (f64, f64), String)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < pairs.len() {
        if pairs[i].0 != 0 {
            i += 1;
            continue;
        }
        let entity = pairs[i].1.to_ascii_uppercase();
        if entity == "LINE" {
            let mut layer = String::new();
            let mut x1 = None;
            let mut y1 = None;
            let mut x2 = None;
            let mut y2 = None;
            i += 1;
            while i < pairs.len() && pairs[i].0 != 0 {
                match pairs[i].0 {
                    8 => layer = pairs[i].1.clone(),
                    10 => x1 = pairs[i].1.parse().ok(),
                    20 => y1 = pairs[i].1.parse().ok(),
                    11 => x2 = pairs[i].1.parse().ok(),
                    21 => y2 = pairs[i].1.parse().ok(),
                    _ => {}
                }
                i += 1;
            }
            if let (Some(x1), Some(y1), Some(x2), Some(y2)) = (x1, y1, x2, y2) {
                out.push(((x1, y1), (x2, y2), layer));
            }
            continue;
        }
        if entity == "LWPOLYLINE" {
            let mut layer = String::new();
            let mut vertices: Vec<(f64, f64)> = Vec::new();
            let mut last_x: Option<f64> = None;
            let mut closed = false;
            i += 1;
            while i < pairs.len() && pairs[i].0 != 0 {
                match pairs[i].0 {
                    8 => layer = pairs[i].1.clone(),
                    70 => closed = pairs[i].1.parse::<i32>().unwrap_or(0) & 1 == 1,
                    10 => last_x = pairs[i].1.parse().ok(),
                    20 => {
                        if let (Some(x), Ok(y)) = (last_x, pairs[i].1.parse::<f64>()) {
                            vertices.push((x, y));
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            for pair in vertices.windows(2) {
                out.push((pair[0], pair[1], layer.clone()));
            }
            if closed && vertices.len() > 2 {
                out.push((vertices[vertices.len() - 1], vertices[0], layer));
            }
            continue;
        }
        i += 1;
    }
    // Prefer explicit wall layers; otherwise keep all plausible linework.
    let has_wall_layer = out.iter().any(|(_, _, layer)| {
        let l = layer.to_ascii_uppercase();
        l.contains("WALL") || layer.contains('墙')
    });
    if has_wall_layer {
        out.retain(|(_, _, layer)| {
            let l = layer.to_ascii_uppercase();
            l.contains("WALL") || layer.contains('墙')
        });
    }
    // Deduplicate exact segments (doors often repeat a jamb line).
    let mut seen: Vec<((i64, i64), (i64, i64))> = Vec::new();
    out.retain(|(a, b, _)| {
        let key = (
            (a.0.round() as i64, a.1.round() as i64),
            (b.0.round() as i64, b.1.round() as i64),
        );
        let reverse = (key.1, key.0);
        if seen.contains(&key) || seen.contains(&reverse) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    out
}

/// Copy the reference image into the user's asset directory as PNG, so the
/// document keeps working even if the original download is moved.
fn unique_stamp() -> u128 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    nanos + seq as u128
}

fn persist_image(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "找不到用户目录".to_string())?;
    let dir = PathBuf::from(home).join("Documents/Fathom/Assets");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem: String = path
        .file_stem()
        .map(|s| s.to_string_lossy().replace(['/', ' '], "-"))
        .unwrap_or_else(|| "plan".into());
    let stamp = unique_stamp();
    let out = dir.join(format!("{stem}-{stamp}.png"));
    let image = image::RgbaImage::from_raw(w, h, rgba.to_vec())
        .ok_or_else(|| "图片缓冲区大小不匹配".to_string())?;
    image.save(&out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn assets_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "找不到用户目录".to_string())?;
    let dir = PathBuf::from(home).join("Documents/Fathom/Assets");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Raster preview: faded source drawing + detected wall centrelines in red.
fn save_raster_preview(
    rgba: &[u8],
    w: u32,
    h: u32,
    doc: &Document,
    width_mm: i32,
) -> Result<PathBuf, String> {
    let src = image::RgbaImage::from_raw(w, h, rgba.to_vec())
        .ok_or_else(|| "图片缓冲区大小不匹配".to_string())?;
    let preview_w = 640u32;
    let preview_h = ((h as f32 / w as f32) * preview_w as f32)
        .round()
        .clamp(240.0, 540.0) as u32;
    let resized = image::imageops::resize(
        &src,
        preview_w,
        preview_h,
        image::imageops::FilterType::Triangle,
    );
    let mut out = image::RgbaImage::new(preview_w, preview_h);
    for (x, y, pixel) in resized.enumerate_pixels() {
        let faded = |c: u8| ((c as f32 * 0.38) + 255.0 * 0.62).round() as u8;
        out.put_pixel(
            x,
            y,
            image::Rgba([faded(pixel[0]), faded(pixel[1]), faded(pixel[2]), 255]),
        );
    }
    let height_mm = doc.underlay.as_ref().map(|u| u.height_mm).unwrap_or(1);
    let sx = preview_w as f32 / width_mm.max(1) as f32;
    let sy = preview_h as f32 / height_mm.max(1) as f32;
    for wall in &doc.walls {
        let a = (wall.start.x as f32 * sx, wall.start.y as f32 * sy);
        let b = (wall.end.x as f32 * sx, wall.end.y as f32 * sy);
        let thickness = (wall.thickness_mm as f32 * sx).clamp(2.0, 7.0);
        draw_thick_line(&mut out, a, b, thickness, image::Rgba([229, 72, 77, 255]));
    }
    draw_openings(&mut out, doc, |p| (p.x as f32 * sx, p.y as f32 * sy));
    for room in &doc.rooms {
        for pair in room.polygon.windows(2) {
            let a = (pair[0].x as f32 * sx, pair[0].y as f32 * sy);
            let b = (pair[1].x as f32 * sx, pair[1].y as f32 * sy);
            draw_thick_line(&mut out, a, b, 1.0, image::Rgba([47, 111, 237, 255]));
        }
    }
    save_preview(&out)
}

/// Vector preview: white sheet + wall centrelines, fitted from wall bounds.
fn save_vector_preview(doc: &Document) -> Result<PathBuf, String> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for wall in &doc.walls {
        min_x = min_x.min(wall.start.x).min(wall.end.x);
        min_y = min_y.min(wall.start.y).min(wall.end.y);
        max_x = max_x.max(wall.start.x).max(wall.end.x);
        max_y = max_y.max(wall.start.y).max(wall.end.y);
    }
    let span_x = (max_x - min_x).max(1) as f32;
    let span_y = (max_y - min_y).max(1) as f32;
    let preview_w = 640u32;
    let preview_h = ((span_y / span_x) * preview_w as f32)
        .round()
        .clamp(240.0, 540.0) as u32;
    let pad = 24.0f32;
    let scale =
        ((preview_w as f32 - pad * 2.0) / span_x).min((preview_h as f32 - pad * 2.0) / span_y);
    let mut out =
        image::RgbaImage::from_pixel(preview_w, preview_h, image::Rgba([253, 252, 249, 255]));
    for wall in &doc.walls {
        let a = (
            pad + (wall.start.x - min_x) as f32 * scale,
            pad + (wall.start.y - min_y) as f32 * scale,
        );
        let b = (
            pad + (wall.end.x - min_x) as f32 * scale,
            pad + (wall.end.y - min_y) as f32 * scale,
        );
        draw_thick_line(&mut out, a, b, 3.0, image::Rgba([229, 72, 77, 255]));
    }
    draw_openings(&mut out, doc, |p| {
        (
            pad + (p.x - min_x) as f32 * scale,
            pad + (p.y - min_y) as f32 * scale,
        )
    });
    for room in &doc.rooms {
        for pair in room.polygon.windows(2) {
            let a = (
                pad + (pair[0].x - min_x) as f32 * scale,
                pad + (pair[0].y - min_y) as f32 * scale,
            );
            let b = (
                pad + (pair[1].x - min_x) as f32 * scale,
                pad + (pair[1].y - min_y) as f32 * scale,
            );
            draw_thick_line(&mut out, a, b, 1.0, image::Rgba([47, 111, 237, 255]));
        }
    }
    save_preview(&out)
}

fn draw_openings<F: Fn(Pt) -> (f32, f32)>(image: &mut image::RgbaImage, doc: &Document, map: F) {
    for opening in &doc.openings {
        let Some(wall) = doc.walls.iter().find(|w| w.id == opening.wall_id) else {
            continue;
        };
        let dx = (wall.end.x - wall.start.x) as f32;
        let dy = (wall.end.y - wall.start.y) as f32;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let ux = dx / len;
        let uy = dy / len;
        let start = Pt::new(
            wall.start.x + (opening.offset_mm as f32 * ux) as i32,
            wall.start.y + (opening.offset_mm as f32 * uy) as i32,
        );
        let end = Pt::new(
            wall.start.x + ((opening.offset_mm + opening.width_mm) as f32 * ux) as i32,
            wall.start.y + ((opening.offset_mm + opening.width_mm) as f32 * uy) as i32,
        );
        draw_thick_line(
            image,
            map(start),
            map(end),
            2.0,
            image::Rgba([47, 160, 90, 255]),
        );
        // Door leaf, drawn into the room on the hinge side.
        let leaf = Pt::new(
            start.x + (-uy * opening.width_mm as f32) as i32,
            start.y + (ux * opening.width_mm as f32) as i32,
        );
        draw_thick_line(
            image,
            map(start),
            map(leaf),
            1.2,
            image::Rgba([47, 160, 90, 255]),
        );
    }
}

fn save_preview(image: &image::RgbaImage) -> Result<PathBuf, String> {
    let dir = assets_dir()?;
    let stamp = unique_stamp();
    let out = dir.join(format!("import-preview-{stamp}.png"));
    image.save(&out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn draw_thick_line(
    image: &mut image::RgbaImage,
    a: (f32, f32),
    b: (f32, f32),
    thickness: f32,
    color: image::Rgba<u8>,
) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let steps = dx.abs().max(dy.abs()).max(1.0).round() as usize;
    let radius = (thickness * 0.5).max(0.6);
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let cx = a.0 + dx * t;
        let cy = a.1 + dy * t;
        let r = radius.ceil() as i32;
        for py in -r..=r {
            for px in -r..=r {
                if ((px * px + py * py) as f32).sqrt() <= radius {
                    let x = (cx.round() as i32 + px).max(0) as u32;
                    let y = (cy.round() as i32 + py).max(0) as u32;
                    if x < image.width() && y < image.height() {
                        image.put_pixel(x, y, color);
                    }
                }
            }
        }
    }
}

/// Merge collinear walls separated by a 600–1100 mm gap into one wall and put
/// a door opening in the gap. This turns the common "wall run interrupted by a
/// door swing" drawing into an editable wall + door instead of two loose ends.
fn merge_door_gaps(doc: &mut Document) -> usize {
    let mut added = 0usize;
    loop {
        let walls = doc.walls.clone();
        let mut merged = false;
        'outer: for i in 0..walls.len() {
            for j in (i + 1)..walls.len() {
                let a = &walls[i];
                let b = &walls[j];
                let horizontal = (a.start.y - a.end.y).abs() <= 20
                    && (b.start.y - b.end.y).abs() <= 20
                    && (a.start.y - b.start.y).abs() <= 20;
                let vertical = (a.start.x - a.end.x).abs() <= 20
                    && (b.start.x - b.end.x).abs() <= 20
                    && (a.start.x - b.start.x).abs() <= 20;
                if !horizontal && !vertical {
                    continue;
                }
                let (a_lo, a_hi, b_lo, b_hi) = if horizontal {
                    (
                        a.start.x.min(a.end.x),
                        a.start.x.max(a.end.x),
                        b.start.x.min(b.end.x),
                        b.start.x.max(b.end.x),
                    )
                } else {
                    (
                        a.start.y.min(a.end.y),
                        a.start.y.max(a.end.y),
                        b.start.y.min(b.end.y),
                        b.start.y.max(b.end.y),
                    )
                };
                let (gap_start, gap_end) = if b_lo >= a_hi {
                    (a_hi, b_lo)
                } else if a_lo >= b_hi {
                    (b_hi, a_lo)
                } else {
                    continue;
                };
                let gap = gap_end - gap_start;
                if !(600..=1_100).contains(&gap) {
                    continue;
                }
                if doc.openings.iter().any(|o| {
                    o.wall_id == a.id
                        && o.offset_mm < gap_end - a_lo
                        && o.offset_mm + o.width_mm > gap_start - a_lo
                }) {
                    continue;
                }
                let lo = a_lo.min(b_lo);
                let hi = a_hi.max(b_hi);
                let coord = if horizontal {
                    ((a.start.y + a.end.y + b.start.y + b.end.y) as f64 / 4.0) as i32
                } else {
                    ((a.start.x + a.end.x + b.start.x + b.end.x) as f64 / 4.0) as i32
                };
                let (start, end) = if horizontal {
                    (Pt::new(lo, coord), Pt::new(hi, coord))
                } else {
                    (Pt::new(coord, lo), Pt::new(coord, hi))
                };
                let thickness = a.thickness_mm.max(b.thickness_mm);
                let kind = if a.kind == WallKind::Exterior || b.kind == WallKind::Exterior {
                    WallKind::Exterior
                } else {
                    WallKind::Partition
                };
                let wall_id = a.id;
                if let Some(wall) = doc.walls.iter_mut().find(|w| w.id == wall_id) {
                    wall.start = start;
                    wall.end = end;
                    wall.thickness_mm = thickness;
                    wall.kind = kind;
                }
                doc.walls.retain(|w| w.id != b.id);
                doc.openings.push(Opening {
                    id: EntityId(doc.next_id()),
                    wall_id,
                    kind: OpeningKind::Door,
                    offset_mm: gap_start - lo,
                    width_mm: gap,
                    height_mm: 2_100,
                    sill_mm: 0,
                    swing_deg: 90.0,
                    hinge_right: false,
                    price: None,
                });
                added += 1;
                merged = true;
                break 'outer;
            }
        }
        if !merged {
            break;
        }
    }
    added
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

    fn dxf_line(x1: f64, y1: f64, x2: f64, y2: f64) -> String {
        format!("0\nLINE\n8\nWALL\n10\n{x1}\n20\n{y1}\n11\n{x2}\n21\n{y2}\n")
    }

    #[test]
    fn dxf_reads_walls_merges_door_gap_and_builds_room() {
        let mut dxf = String::from(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n",
        );
        dxf.push_str(&dxf_line(0.0, 8000.0, 10000.0, 8000.0));
        dxf.push_str(&dxf_line(0.0, 0.0, 0.0, 8000.0));
        dxf.push_str(&dxf_line(10000.0, 0.0, 10000.0, 8000.0));
        dxf.push_str(&dxf_line(0.0, 0.0, 4000.0, 0.0));
        dxf.push_str(&dxf_line(4900.0, 0.0, 10000.0, 0.0));
        dxf.push_str("0\nENDSEC\n0\nEOF\n");
        let path = std::path::PathBuf::from("/tmp/fp-import-test.dxf");
        std::fs::write(&path, dxf).unwrap();
        let outcome = import_dxf(&path, "DXF 测试").expect("import");
        assert_eq!(outcome.recognized_walls, 4, "4 merged wall runs");
        assert_eq!(outcome.recognized_rooms, 1, "one closed room");
        assert_eq!(outcome.recognized_doors, 1, "the 900 mm gap becomes a door");
        assert!(outcome.preview_png.exists());
        let _ = std::fs::remove_file(&outcome.preview_png);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dxf_units_are_converted_to_millimetres() {
        let mut dxf = String::from(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n",
        );
        // 4 m × 3 m room in metres.
        dxf.push_str(&dxf_line(0.0, 0.0, 4.0, 0.0));
        dxf.push_str(&dxf_line(4.0, 0.0, 4.0, 3.0));
        dxf.push_str(&dxf_line(4.0, 3.0, 0.0, 3.0));
        dxf.push_str(&dxf_line(0.0, 3.0, 0.0, 0.0));
        dxf.push_str("0\nENDSEC\n0\nEOF\n");
        let path = std::path::PathBuf::from("/tmp/fp-import-metres.dxf");
        std::fs::write(&path, dxf).unwrap();
        // Rewrite INSUNITS to 6 (metres) for this file.
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("$INSUNITS\n70\n4", "$INSUNITS\n70\n6");
        std::fs::write(&path, text).unwrap();
        let outcome = import_dxf(&path, "米制").expect("import");
        let wall = &outcome.doc.walls[0];
        let length = ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
        assert!(length > 3_900.0 && length < 4_100.0, "length {length}");
        let _ = std::fs::remove_file(&outcome.preview_png);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dxf_unitless_metres_fall_back_to_metres() {
        let mut dxf = String::from(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n0\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n",
        );
        dxf.push_str(&dxf_line(0.0, 0.0, 4.0, 0.0));
        dxf.push_str(&dxf_line(4.0, 0.0, 4.0, 3.0));
        dxf.push_str(&dxf_line(4.0, 3.0, 0.0, 3.0));
        dxf.push_str(&dxf_line(0.0, 3.0, 0.0, 0.0));
        dxf.push_str("0\nENDSEC\n0\nEOF\n");
        let path = std::path::PathBuf::from("/tmp/fp-import-unitless.dxf");
        std::fs::write(&path, dxf).unwrap();
        let outcome = import_dxf(&path, "无单位").expect("import");
        let wall = &outcome.doc.walls[0];
        let length = ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
        assert!(length > 3_900.0 && length < 4_100.0, "length {length}");
        let _ = std::fs::remove_file(&outcome.preview_png);
        let _ = std::fs::remove_file(&path);
    }

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
    fn raster_import_returns_a_visual_preview_and_rooms() {
        let (w, h) = (800u32, 600u32);
        let mut img = canvas(w, h);
        // 10 px thick rectangle, 50 px inset.
        line(&mut img, w, 50, 50, 750, 60, 10);
        line(&mut img, w, 50, 540, 750, 550, 10);
        line(&mut img, w, 50, 50, 60, 550, 10);
        line(&mut img, w, 740, 50, 750, 550, 10);
        let src = std::path::PathBuf::from("/tmp/fp-raster-plan.png");
        image::RgbaImage::from_raw(w, h, img)
            .unwrap()
            .save(&src)
            .unwrap();
        let outcome = import_floorplan(&src, 10_000, "栅格测试", true).expect("import");
        assert!(
            outcome.recognized_walls >= 4,
            "walls {}",
            outcome.recognized_walls
        );
        assert!(
            outcome.recognized_rooms >= 1,
            "rooms {}",
            outcome.recognized_rooms
        );
        assert!(outcome.preview_png.exists());
        let _ = std::fs::remove_file(&outcome.preview_png);
        let _ = std::fs::remove_file(&src);
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
