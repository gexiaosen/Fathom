//! PNG export: render the plan offscreen into a texture and save it.

use std::path::Path;

use crate::render3d::Renderer3d;
use fp_model::{OpeningKind, WallKind};
use fp_render2d::{Batch, Camera2D};
use fp_render3d::scene_mesh_full;
use wgpu::util::DeviceExt;

use crate::scene::{SceneState, plan_bounds};

const MAX_SPAN: f32 = 3200.0;
const MIN_SPAN: u32 = 900;
const PAD: f32 = 120.0;
const BYTES_PER_PIXEL: u32 = 4;
const COPY_ALIGN: u32 = 256;

/// Canvas size that matches the plan's aspect ratio, fitted to MAX_SPAN.
fn canvas_size(min: fp_core::Pt, max: fp_core::Pt) -> (u32, u32) {
    let span_x = (max.x - min.x).max(1) as f32;
    let span_y = (max.y - min.y).max(1) as f32;
    let usable = MAX_SPAN - 2.0 * PAD;
    let scale = (usable / span_x).min(usable / span_y);
    let width = (span_x * scale + 2.0 * PAD).round().max(MIN_SPAN as f32) as u32;
    let height = (span_y * scale + 2.0 * PAD).round().max(MIN_SPAN as f32) as u32;
    (width, height)
}

fn plan_rgba(state: &SceneState) -> Result<(u32, u32, Vec<u8>, bool), String> {
    let (min, max) = plan_bounds(&state.doc);
    let (width, height) = canvas_size(min, max);
    let mut cam = Camera2D::new([width as f32, height as f32]);
    cam.fit(min, max, PAD);
    let batch = state.render_batch(&cam, &[], &[], None);
    let mut rgba = render_to_rgba(&batch, width, height)?;
    let labelled = overlay_labels(&mut rgba, width, height, state, &cam);
    Ok((width, height, rgba, labelled))
}

/// Render the whole plan fit into a PNG that mirrors the plan's aspect ratio.
pub fn export_png(state: &SceneState, path: &Path) -> Result<String, String> {
    let (width, height, rgba, labelled) = plan_rgba(state)?;
    write_png(path, width, height, &rgba)?;
    Ok(if labelled {
        format!("已导出 PNG {width}×{height}（含标注）")
    } else {
        format!("已导出 PNG {width}×{height}（未找到中文字体，标注已跳过）")
    })
}

/// A4 PDF with the labelled plan rasterised at print resolution.
pub fn export_pdf(state: &SceneState, path: &Path) -> Result<String, String> {
    let (width, height, rgba, _) = plan_rgba(state)?;
    let pdf = pdf_from_rgba(width, height, &rgba)?;
    std::fs::write(path, pdf).map_err(|e| format!("PDF 写入失败: {e}"))?;
    Ok(format!("已导出 PDF · A4 · {width}×{height}px"))
}

/// Print the current plan to the system default printer via the PDF spool file.
pub fn print_plan(state: &SceneState) -> String {
    let path = std::env::temp_dir().join("Fathom-print.pdf");
    if let Err(err) = export_pdf(state, &path) {
        return format!("打印准备失败 · {err}");
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("lp")
            .args(["-o", "media=A4", "-o", "fit-to-page"])
            .arg(&path)
            .output();
        match output {
            Ok(out) if out.status.success() => "已发送到默认打印机".to_string(),
            Ok(out) => {
                let msg = String::from_utf8_lossy(&out.stderr);
                let _ = std::process::Command::new("open").arg(&path).spawn();
                format!(
                    "打印失败，已打开预览 · {}",
                    msg.lines().next().unwrap_or("没有默认打印机")
                )
            }
            Err(err) => {
                let _ = std::process::Command::new("open").arg(&path).spawn();
                format!("打印命令不可用，已打开预览 · {err}")
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = std::process::Command::new("open").arg(&path).spawn();
        format!("已生成打印 PDF · {}", path.display())
    }
}

#[allow(clippy::chunks_exact_to_as_chunks)]
fn pdf_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for pixel in rgba.chunks_exact(4) {
        let a = pixel[3] as u16;
        let blend = |c: u8| -> u8 { ((c as u16 * a + 255 * (255 - a)) / 255) as u8 };
        rgb.push(blend(pixel[0]));
        rgb.push(blend(pixel[1]));
        rgb.push(blend(pixel[2]));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&rgb).map_err(|e| e.to_string())?;
    let compressed = encoder.finish().map_err(|e| e.to_string())?;

    let landscape = width >= height;
    let (page_w, page_h) = if landscape {
        (842.0f32, 595.0f32)
    } else {
        (595.0f32, 842.0f32)
    };
    let margin = 28.0f32;
    let scale =
        ((page_w - margin * 2.0) / width as f32).min((page_h - margin * 2.0) / height as f32);
    let draw_w = width as f32 * scale;
    let draw_h = height as f32 * scale;
    let x = (page_w - draw_w) / 2.0;
    let y = (page_h - draw_h) / 2.0;
    let content = format!("q\n{draw_w:.2} 0 0 {draw_h:.2} {x:.2} {y:.2} cm\n/Im0 Do\nQ\n");

    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets = Vec::new();
    let push_obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        let number = offsets.len();
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    push_obj(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    push_obj(
        &mut out,
        &mut offsets,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    );
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_w:.0} {page_h:.0}] \
         /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>"
    );
    push_obj(&mut out, &mut offsets, page.as_bytes());
    let mut image = format!(
        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
        compressed.len()
    )
    .into_bytes();
    image.extend_from_slice(&compressed);
    image.extend_from_slice(b"\nendstream");
    push_obj(&mut out, &mut offsets, &image);
    let stream = format!(
        "<< /Length {} >>\nstream\n{}endstream",
        content.len(),
        content
    );
    push_obj(&mut out, &mut offsets, stream.as_bytes());

    let xref_at = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", offsets.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    Ok(out)
}

// ---------------------------------------------------------------- annotations

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

const LABEL_DARK: [f32; 3] = [0.13, 0.15, 0.20];
const LABEL_GRAY: [f32; 3] = [0.34, 0.36, 0.42];

/// CPU glyph rasterisation for on-image annotations (room names, dimensions).
struct TextPainter {
    font: FontVec,
}

impl TextPainter {
    fn load() -> Option<Self> {
        let mut candidates: Vec<(String, u32)> = Vec::new();
        if cfg!(target_os = "macos") {
            candidates.push((
                "/System/Library/Fonts/Supplemental/Arial Unicode.ttf".into(),
                0,
            ));
            candidates.push(("/System/Library/Fonts/PingFang.ttc".into(), 0));
        }
        if cfg!(target_os = "windows") {
            candidates.push(("C:/Windows/Fonts/msyh.ttc".into(), 0));
            candidates.push(("C:/Windows/Fonts/simhei.ttf".into(), 0));
        }
        if cfg!(target_os = "linux") {
            candidates.push((
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into(),
                0,
            ));
            candidates.push((
                "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc".into(),
                0,
            ));
            candidates.push((
                "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf".into(),
                0,
            ));
        }
        candidates
            .into_iter()
            .find_map(|(path, index)| {
                let bytes = std::fs::read(path).ok()?;
                FontVec::try_from_vec_and_index(bytes, index).ok()
            })
            .map(|font| Self { font })
    }

    fn text_width(&self, text: &str, size: f32) -> f32 {
        let scaled = self.font.as_scaled(PxScale::from(size));
        text.chars()
            .map(|ch| scaled.h_advance(scaled.glyph_id(ch)))
            .sum()
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_centered(
        &self,
        rgba: &mut [u8],
        width: u32,
        height: u32,
        text: &str,
        cx: f32,
        cy: f32,
        size: f32,
        color: [f32; 3],
    ) {
        let scaled = self.font.as_scaled(PxScale::from(size));
        let text_w = self.text_width(text, size);
        let left = cx - text_w / 2.0;
        let baseline = cy + size * 0.36;
        let pad = size * 0.34;
        fill_rect(
            rgba,
            width,
            height,
            left - pad,
            cy - size * 0.66,
            text_w + pad * 2.0,
            size * 1.34,
            [1.0, 0.998, 0.992],
            0.52,
        );
        let mut pen = left;
        for ch in text.chars() {
            let glyph = scaled.scaled_glyph(ch);
            if let Some(outlined) = self.font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let px = (pen + bounds.min.x + gx as f32).floor() as i32;
                    let py = (baseline + bounds.min.y + gy as f32).floor() as i32;
                    blend_px(rgba, width, height, px, py, color, coverage);
                });
            }
            pen += scaled.h_advance(scaled.glyph_id(ch));
        }
    }
}

fn blend_px(rgba: &mut [u8], width: u32, height: u32, x: i32, y: i32, color: [f32; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let index = ((y as u32 * width + x as u32) * 4) as usize;
    let a = alpha.clamp(0.0, 1.0);
    for channel in 0..3 {
        let dst = rgba[index + channel] as f32 / 255.0;
        rgba[index + channel] = ((color[channel] * a + dst * (1.0 - a)) * 255.0).round() as u8;
    }
    rgba[index + 3] = 255;
}

#[allow(clippy::too_many_arguments)]
fn fill_rect(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 3],
    alpha: f32,
) {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x + w).ceil() as i32;
    let y1 = (y + h).ceil() as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            blend_px(rgba, width, height, px, py, color, alpha);
        }
    }
}

/// Area-weighted polygon centroid (falls back to the bbox centre).
fn polygon_centroid(points: &[fp_core::Pt]) -> fp_core::Pt {
    let mut area2 = 0.0f64;
    let mut cx = 0.0f64;
    let mut cy = 0.0f64;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        let cross = a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
        area2 += cross;
        cx += (a.x as f64 + b.x as f64) * cross;
        cy += (a.y as f64 + b.y as f64) * cross;
    }
    if area2.abs() < 1e-6 {
        let (min, max) = polygon_bounds(points);
        return fp_core::Pt::new((min.x + max.x) / 2, (min.y + max.y) / 2);
    }
    fp_core::Pt::new(
        (cx / (3.0 * area2)).round() as i32,
        (cy / (3.0 * area2)).round() as i32,
    )
}

fn polygon_bounds(points: &[fp_core::Pt]) -> (fp_core::Pt, fp_core::Pt) {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for p in points {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    (
        fp_core::Pt::new(min_x, min_y),
        fp_core::Pt::new(max_x, max_y),
    )
}

/// Draw room names/areas and wall lengths onto the bitmap; returns false when
/// no CJK-capable system font was found.
fn overlay_labels(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    state: &SceneState,
    cam: &Camera2D,
) -> bool {
    let Some(painter) = TextPainter::load() else {
        return false;
    };
    let base = (width as f32 * 0.015).clamp(22.0, 40.0);

    if state.doc.settings.show_room_names {
        for room in &state.doc.rooms {
            if room.polygon.len() < 3 {
                continue;
            }
            let centre = polygon_centroid(&room.polygon);
            let [sx, sy] = cam.world_to_screen(centre);
            let area = fp_geom::area(&room.polygon) / 1_000_000.0;
            let text = format!("{}  {:.1}㎡", room.name, area);
            painter.draw_centered(rgba, width, height, &text, sx, sy, base, LABEL_DARK);
        }
    }

    if state.doc.settings.show_dimensions {
        for wall in &state.doc.walls {
            let dx = (wall.end.x - wall.start.x) as f32;
            let dy = (wall.end.y - wall.start.y) as f32;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1200.0 {
                continue;
            }
            let mid = fp_core::Pt::new(
                (wall.start.x + wall.end.x) / 2,
                (wall.start.y + wall.end.y) / 2,
            );
            let [sx, sy] = cam.world_to_screen(mid);
            let text = format!("{:.2}m", len / 1000.0);
            painter.draw_centered(rgba, width, height, &text, sx, sy, base * 0.55, LABEL_GRAY);
        }
    }
    true
}

/// Quote a CSV cell; doubles embedded quotes.
fn cell(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

/// Material/quote list as CSV (Excel friendly, UTF-8 BOM added on save).
pub fn bom_csv(state: &SceneState) -> String {
    let doc = &state.doc;
    let mut out = String::new();
    out.push_str("户型装修报价清单\n\n");

    const WASTE: f64 = 0.05;
    out.push_str("房间,净面积(㎡),计损耗面积(㎡),地面材质,单价(元/㎡),小计(元)\n");
    let mut area_total = 0.0f64;
    let mut billed_total = 0.0f64;
    let mut material_total = 0.0f64;
    for room in &doc.rooms {
        if !room.counted {
            continue;
        }
        let area_m2 = fp_geom::area(&room.polygon) / 1_000_000.0;
        let billed = area_m2 * (1.0 + WASTE);
        let material = room
            .material_id
            .and_then(|id| doc.materials.iter().find(|m| m.id == id));
        let name = material.map(|m| m.name.clone()).unwrap_or_default();
        let price = material.map(|m| m.price_per_m2).unwrap_or(0.0);
        let subtotal = billed * price;
        area_total += area_m2;
        billed_total += billed;
        material_total += subtotal;
        out.push_str(&format!(
            "{},{:.2},{:.2},{},{:.1},{:.2}\n",
            cell(&room.name),
            area_m2,
            billed,
            cell(&name),
            price,
            subtotal
        ));
    }
    out.push_str(&format!(
        "地面小计,{:.2},{:.2},, ,{:.2}\n\n",
        area_total, billed_total, material_total
    ));

    out.push_str("门窗,樘数,洞口面积(㎡)\n");
    let (mut doors, mut windows, mut opening_area) = (0usize, 0usize, 0.0f64);
    for o in &doc.openings {
        match o.kind {
            OpeningKind::Door => doors += 1,
            OpeningKind::Window | OpeningKind::Sliding | OpeningKind::Bay => windows += 1,
        }
        opening_area += (o.width_mm as f64 * o.height_mm as f64) / 1_000_000.0;
    }
    out.push_str(&format!(
        "门,{},-,窗,{},-,洞口合计,{:.3}\n\n",
        doors, windows, opening_area
    ));

    out.push_str("墙体类型,总长(m),平均厚度(mm),段数\n");
    let kinds = [
        WallKind::Bearing,
        WallKind::Exterior,
        WallKind::Partition,
        WallKind::Low,
    ];
    for kind in kinds {
        let walls: Vec<_> = doc.walls.iter().filter(|w| w.kind == kind).collect();
        if walls.is_empty() {
            continue;
        }
        let total: f64 = walls
            .iter()
            .map(|w| {
                let dx = (w.end.x - w.start.x) as f64;
                let dy = (w.end.y - w.start.y) as f64;
                (dx * dx + dy * dy).sqrt() / 1000.0
            })
            .sum();
        let avg_thickness =
            walls.iter().map(|w| w.thickness_mm as f64).sum::<f64>() / walls.len() as f64;
        out.push_str(&format!(
            "{},{:.2},{:.0},{}\n",
            cell(kind_name(kind)),
            total,
            avg_thickness,
            walls.len()
        ));
    }

    out.push_str("\n家具,数量,单件尺寸(宽×深 mm)\n");
    let mut seen: Vec<(String, usize, i32, i32)> = Vec::new();
    for item in &doc.furniture {
        if let Some(row) = seen.iter_mut().find(|r| r.0 == item.name) {
            row.1 += 1;
        } else {
            seen.push((item.name.clone(), 1, item.width_mm, item.depth_mm));
        }
    }
    for (name, count, width, depth) in &seen {
        out.push_str(&format!(
            "{},{},{}\u{00d7}{}\n",
            cell(name),
            count,
            width,
            depth
        ));
    }

    out.push_str(&format!(
        "\n总建筑面积(㎡),{:.2}\n地面计价面积(含5%损耗),{:.2}\n地面材料合计(元),{:.2}\n材料损耗(元),{:.2}\n门窗数量,{}\n家具件数,{}\n",
        area_total,
        billed_total,
        material_total,
        material_total - material_total / (1.0 + WASTE),
        doors + windows,
        doc.furniture.len()
    ));
    out
}

fn kind_name(kind: WallKind) -> &'static str {
    match kind {
        WallKind::Bearing => "承重墙",
        WallKind::Exterior => "外墙",
        WallKind::Partition => "隔墙",
        WallKind::Low => "矮墙",
    }
}

/// Save the quote list as a BOM-prefixed CSV (so Excel opens UTF-8 correctly).
pub fn export_bom(state: &SceneState, path: &Path) -> Result<String, String> {
    let csv = bom_csv(state);
    let mut text = String::with_capacity(csv.len() + 1);
    text.push('\u{feff}');
    text.push_str(&csv);
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok("已导出报价清单 CSV".to_string())
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    Ok(())
}

fn create_gpu() -> Result<(wgpu::Device, wgpu::Queue), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .map_err(|e| format!("没有可用的 GPU 适配器: {e}"))?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("fp export"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .map_err(|e| format!("创建设备失败: {e}"))
}

fn render_to_rgba(batch: &Batch, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let (device, queue) = create_gpu()?;

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fp export target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fp export shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("fp export bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let viewport = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fp export viewport"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &viewport,
        0,
        bytemuck::bytes_of(&[width as f32, height as f32, 0.0, 0.0]),
    );
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("fp export bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: viewport.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fp export layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fp export pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<fp_render2d::QuadVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 8,
                        shader_location: 1,
                    },
                ],
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("fp export vertices"),
        contents: if batch.vertices.is_empty() {
            &[0u8; 64]
        } else {
            bytemuck::cast_slice(&batch.vertices)
        },
        usage: wgpu::BufferUsages::VERTEX,
    });

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("fp export enc"),
    });
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fp export pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.965,
                        g: 0.957,
                        b: 0.941,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.draw(0..batch.vertices.len() as u32, 0..1);
    }
    queue.submit(Some(enc.finish()));
    read_texture(&device, &queue, &target, width, height)
}

fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let padded_row = (width * BYTES_PER_PIXEL).div_ceil(COPY_ALIGN) * COPY_ALIGN;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fp export readback"),
        size: (padded_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("fp export readback enc"),
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(enc.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(std::time::Duration::from_secs(10)),
    });
    let mapped: Result<(), wgpu::BufferAsyncError> =
        rx.recv().map_err(|_| "读取位图失败".to_string())?;
    mapped.map_err(|e| format!("映射失败: {e}"))?;

    let data = slice
        .get_mapped_range()
        .map_err(|e| format!("读取位图失败: {e}"))?;
    let row_bytes = (width * BYTES_PER_PIXEL) as usize;
    let mut out = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * padded_row as usize;
        out.extend_from_slice(&data[start..start + row_bytes]);
    }
    drop(data);
    readback.unmap();
    Ok(out)
}

/// Render the extruded 3D view at 1920×1200 from the current orbit camera.
pub fn export_png_3d(state: &SceneState, path: &Path) -> Result<String, String> {
    const W: u32 = 1920;
    const H: u32 = 1200;
    // Presentation camera: 3/4 aerial view that shows floors, not just walls.
    let (min, max) = plan_bounds(&state.doc);
    let min3 = glam::Vec3::new(min.x as f32 / 1000.0, 0.0, min.y as f32 / 1000.0);
    let max3 = glam::Vec3::new(max.x as f32 / 1000.0, 3.2, max.y as f32 / 1000.0);
    let mut cam = fp_render3d::OrbitCamera::new(glam::Vec3::ZERO);
    cam.fit(min3, max3);
    cam.yaw = 0.72;
    cam.pitch = 0.92;
    cam.distance *= 1.02;
    let view_proj = cam.view_proj(W as f32 / H as f32);
    let mesh = scene_mesh_full(
        &state.doc.walls,
        &state.doc.openings,
        &state.doc.rooms,
        &state.doc.materials,
        &state.doc.furniture,
    );
    render_3d_frame(
        &mesh.vertices,
        path,
        W,
        H,
        view_proj,
        cam.eye().to_array(),
        state.look3d(),
    )
}

/// Render one 3D frame off-screen with an arbitrary camera and write it to `path`.
pub fn render_3d_frame(
    mesh: &[fp_render3d::MeshVertex],
    path: &Path,
    width: u32,
    height: u32,
    view_proj: glam::Mat4,
    eye: [f32; 3],
    look: crate::render3d::Look,
) -> Result<String, String> {
    let (device, queue) = create_gpu()?;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fp export 3d target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut renderer = Renderer3d::new(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        width,
        height,
    );
    if mesh.is_empty() {
        return Err("场景为空，先画几面墙再导出".to_string());
    }
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("fp export 3d enc"),
    });
    let scene = crate::render3d::Scene3dView {
        view_proj,
        eye,
        look,
    };
    renderer.prime_shadow(&queue, mesh, 0, scene);
    renderer.draw(&queue, &mut enc, &view, mesh, 0, scene);
    queue.submit(Some(enc.finish()));

    let rgba = read_texture(&device, &queue, &target, width, height)?;
    write_png(path, width, height, &rgba)?;
    Ok(format!("已导出 3D 截图 {width}×{height}"))
}

// ------------------------------------------------------------------ svg out

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn hex(color: fp_render2d::Color) -> String {
    let c = color.0;
    format!(
        "#{:02X}{:02X}{:02X}",
        (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[2].clamp(0.0, 1.0) * 255.0).round() as u8
    )
}

fn kind_colour(kind: fp_model::WallKind) -> &'static str {
    match kind {
        fp_model::WallKind::Bearing => "#8A8F96",
        fp_model::WallKind::Exterior => "#9AA1AA",
        fp_model::WallKind::Partition => "#B9BEC7",
        fp_model::WallKind::Low => "#C9CED5",
    }
}

/// Wall outline as four corners (thickness applied along the normal).
fn wall_quad(w: &fp_model::Wall) -> [fp_core::Pt; 4] {
    let dx = (w.end.x - w.start.x) as f64;
    let dy = (w.end.y - w.start.y) as f64;
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let half = w.thickness_mm as f64 / 2.0;
    let nx = (-dy / len * half).round() as i32;
    let ny = (dx / len * half).round() as i32;
    [
        fp_core::Pt::new(w.start.x + nx, w.start.y + ny),
        fp_core::Pt::new(w.end.x + nx, w.end.y + ny),
        fp_core::Pt::new(w.end.x - nx, w.end.y - ny),
        fp_core::Pt::new(w.start.x - nx, w.start.y - ny),
    ]
}

fn polygon_points(points: &[fp_core::Pt]) -> String {
    points
        .iter()
        .map(|p| format!("{},{}", p.x, p.y))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Editable vector plan: rooms, walls, door/window symbols, furniture, labels.
pub fn plan_svg(state: &SceneState) -> String {
    use std::fmt::Write as _;

    let doc = &state.doc;
    let (min, max) = plan_bounds(doc);
    let pad = 500;
    let vb_x = min.x - pad;
    let vb_y = min.y - pad;
    let vb_w = (max.x - min.x) + pad * 2;
    let vb_h = (max.y - min.y) + pad * 2;
    let mut out = String::new();

    let _ = writeln!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}mm\" height=\"{}mm\" viewBox=\"{} {} {} {}\">",
        (vb_w as f64 / 10.0).round(),
        (vb_h as f64 / 10.0).round(),
        vb_x,
        vb_y,
        vb_w,
        vb_h
    );
    let _ = writeln!(
        out,
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#F6F4F0\"/>",
        vb_x, vb_y, vb_w, vb_h
    );

    // rooms
    let _ = writeln!(out, "<g id=\"rooms\">");
    for room in &doc.rooms {
        if room.polygon.len() < 3 {
            continue;
        }
        let fill = fp_render2d::material_color(&doc.materials, room);
        let _ = writeln!(
            out,
            "  <polygon points=\"{}\" fill=\"{}\" fill-opacity=\"0.30\" stroke=\"#D8D4CC\" stroke-width=\"1\"/>",
            polygon_points(&room.polygon),
            hex(fill)
        );
    }
    let _ = writeln!(out, "</g>");

    // walls
    let _ = writeln!(out, "<g id=\"walls\">");
    for wall in &doc.walls {
        let _ = writeln!(
            out,
            "  <polygon points=\"{}\" fill=\"{}\"/>",
            polygon_points(&wall_quad(wall)),
            kind_colour(wall.kind)
        );
    }
    let _ = writeln!(out, "</g>");

    // openings
    let _ = writeln!(out, "<g id=\"openings\">");
    for opening in &doc.openings {
        let Some(wall) = doc.walls.iter().find(|w| w.id == opening.wall_id) else {
            continue;
        };
        let len = ((wall.end.x - wall.start.x) as f64)
            .hypot((wall.end.y - wall.start.y) as f64)
            .max(1.0);
        let t0 = (opening.offset_mm as f64 / len).clamp(0.0, 1.0);
        let t1 = ((opening.offset_mm + opening.width_mm) as f64 / len).clamp(0.0, 1.0);
        let a = fp_core::Pt::new(
            (wall.start.x as f64 + (wall.end.x - wall.start.x) as f64 * t0).round() as i32,
            (wall.start.y as f64 + (wall.end.y - wall.start.y) as f64 * t0).round() as i32,
        );
        let b = fp_core::Pt::new(
            (wall.start.x as f64 + (wall.end.x - wall.start.x) as f64 * t1).round() as i32,
            (wall.start.y as f64 + (wall.end.y - wall.start.y) as f64 * t1).round() as i32,
        );
        let ux = (b.x - a.x) as f64;
        let uy = (b.y - a.y) as f64;
        let ul = (ux * ux + uy * uy).sqrt();
        if ul < 1.0 {
            continue;
        }
        let unit = (ux / ul, uy / ul);
        let normal = (-unit.1, unit.0);
        match opening.kind {
            fp_model::OpeningKind::Door | fp_model::OpeningKind::Sliding => {
                let width = opening.width_mm as f64;
                let leaf_end = (a.x as f64 + normal.0 * width, a.y as f64 + normal.1 * width);
                let _ = writeln!(
                    out,
                    "  <line x1=\"{}\" y1=\"{}\" x2=\"{:.0}\" y2=\"{:.0}\" stroke=\"#D0851F\" stroke-width=\"3\"/>",
                    a.x, a.y, leaf_end.0, leaf_end.1
                );
                let _ = writeln!(
                    out,
                    "  <path d=\"M {:.0},{:.0} A {:.0},{:.0} 0 0 1 {},{}\" fill=\"none\" stroke=\"#D0851F\" stroke-width=\"2\"/>",
                    leaf_end.0, leaf_end.1, width, width, b.x, b.y
                );
            }
            fp_model::OpeningKind::Window | fp_model::OpeningKind::Bay => {
                let half = 70.0;
                let quad = [
                    fp_core::Pt::new(
                        (a.x as f64 + normal.0 * half) as i32,
                        (a.y as f64 + normal.1 * half) as i32,
                    ),
                    fp_core::Pt::new(
                        (b.x as f64 + normal.0 * half) as i32,
                        (b.y as f64 + normal.1 * half) as i32,
                    ),
                    fp_core::Pt::new(
                        (b.x as f64 - normal.0 * half) as i32,
                        (b.y as f64 - normal.1 * half) as i32,
                    ),
                    fp_core::Pt::new(
                        (a.x as f64 - normal.0 * half) as i32,
                        (a.y as f64 - normal.1 * half) as i32,
                    ),
                ];
                let _ = writeln!(
                    out,
                    "  <polygon points=\"{}\" fill=\"#3A85E6\"/>",
                    polygon_points(&quad)
                );
            }
        }
    }
    let _ = writeln!(out, "</g>");

    // furniture
    let _ = writeln!(out, "<g id=\"furniture\">");
    for item in &doc.furniture {
        let corners = fp_render2d::corners(item);
        let _ = writeln!(
            out,
            "  <polygon points=\"{}\" fill=\"{}\" stroke=\"#9A9384\" stroke-width=\"2\"/>",
            polygon_points(&corners),
            if item.color.starts_with('#') {
                item.color.clone()
            } else {
                "#E8E2D8".to_string()
            }
        );
    }
    let _ = writeln!(out, "</g>");

    // labels
    let _ = writeln!(
        out,
        "<g id=\"labels\" font-family=\"PingFang SC, Microsoft YaHei, Noto Sans CJK SC, sans-serif\" fill=\"#1F2430\">"
    );
    if doc.settings.show_room_names {
        for room in &doc.rooms {
            if room.polygon.len() < 3 {
                continue;
            }
            let centre = polygon_centroid(&room.polygon);
            let area = fp_geom::area(&room.polygon) / 1_000_000.0;
            let _ = writeln!(
                out,
                "  <text x=\"{}\" y=\"{}\" font-size=\"150\" text-anchor=\"middle\">{} {:.1}㎡</text>",
                centre.x,
                centre.y,
                esc(&room.name),
                area
            );
        }
    }
    if doc.settings.show_dimensions {
        for wall in &doc.walls {
            let len =
                ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
            if len < 1200.0 {
                continue;
            }
            let mid = fp_core::Pt::new(
                (wall.start.x + wall.end.x) / 2,
                (wall.start.y + wall.end.y) / 2,
            );
            let _ = writeln!(
                out,
                "  <text x=\"{}\" y=\"{}\" font-size=\"110\" fill=\"#4A4F5A\" text-anchor=\"middle\">{:.2}m</text>",
                mid.x,
                mid.y,
                len / 1000.0
            );
        }
    }
    let _ = writeln!(out, "</g>");
    let _ = writeln!(out, "</svg>");
    out
}

/// Save an editable vector plan.
pub fn export_svg(state: &SceneState, path: &Path) -> Result<String, String> {
    std::fs::write(path, plan_svg(state)).map_err(|e| e.to_string())?;
    Ok("已导出 SVG 矢量图".to_string())
}

const SHADER: &str = r#"
struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> viewport: vec4<f32>;

@vertex
fn vs_main(in: VIn) -> VOut {
    var out: VOut;
    let ndc = vec2(
        in.pos.x / viewport.x * 2.0 - 1.0,
        1.0 - in.pos.y / viewport.y * 2.0,
    );
    out.clip = vec4(ndc, 0.0, 1.0);
    out.color = in.color;
    return out;
}
@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::SceneState;
    use fp_core::EntityId;
    use fp_core::Pt;
    use fp_model::{Document, Wall, WallKind};
    use std::path::PathBuf;

    #[test]
    fn pdf_embeds_an_image_object_and_xref() {
        let rgba = [
            255u8, 255, 255, 255, 0, 0, 0, 255, 30, 60, 90, 255, 200, 10, 10, 255,
        ];
        let pdf = pdf_from_rgba(2, 2, &rgba).expect("pdf");
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Subtype /Image"));
        assert!(text.contains("/Filter /FlateDecode"));
        assert!(text.contains("xref"));
        assert!(text.trim_end().ends_with("%%EOF"));
    }

    #[test]
    fn svg_contains_rooms_walls_and_labels() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/samples/demo-apartment.json");
        let text = std::fs::read_to_string(&path).expect("fixture");
        let doc = Document::from_json(&text).expect("parse");
        let state = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/x.json"));
        let svg = plan_svg(&state);
        assert!(svg.starts_with("<?xml"), "{svg:.80}");
        assert!(svg.contains("id=\"rooms\""));
        assert!(svg.contains("id=\"walls\""));
        assert!(svg.contains("id=\"furniture\""));
        assert!(svg.contains("客厅 21.5"), "room label missing");
        assert!(svg.contains("stroke=\"#D0851F\""), "door symbol missing");
        assert!(svg.contains("#3A85E6"), "window symbol missing");
        assert!(svg.trim_end().ends_with("</svg>"));

        let out = PathBuf::from("/tmp/fp-plan-test.svg");
        export_svg(&state, &out).expect("svg");
        assert!(std::fs::metadata(&out).unwrap().len() > 8_000);
    }

    #[test]
    fn bom_lists_rooms_and_materials() {
        use fp_model::{Material, Room};
        let mut doc = Document::new("bom-test");
        let mat = EntityId(900);
        doc.materials.push(Material {
            id: mat,
            name: "木地板".into(),
            price_per_m2: 180.0,
            kind: "wood".into(),
        });
        doc.rooms.push(Room {
            id: EntityId(901),
            polygon: vec![
                Pt::new(0, 0),
                Pt::new(6000, 0),
                Pt::new(6000, 4000),
                Pt::new(0, 4000),
            ],
            name: "客厅".into(),
            material_id: Some(mat),
            counted: true,
        });
        doc.walls.push(Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(6000, 0),
            200,
            WallKind::Bearing,
        ));
        let state = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/x.json"));
        let csv = bom_csv(&state);
        assert!(csv.contains("客厅"), "{csv}");
        assert!(csv.contains("24.00"), "{csv}");
        assert!(csv.contains("25.20"), "5% waste area: {csv}");
        assert!(csv.contains("4536.00"), "billed subtotal: {csv}");
        assert!(csv.contains("地面小计"), "{csv}");
        assert!(csv.contains("材料损耗"), "{csv}");
        assert!(csv.contains("承重墙"), "{csv}");
        assert!(csv.contains("6.00"), "{csv}");

        let path = PathBuf::from("/tmp/fp-bom-test.csv");
        export_bom(&state, &path).expect("bom");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with('\u{feff}'));
    }

    #[test]
    #[ignore = "needs a GPU; run with --ignored"]
    fn export_demo_fixture_looks_right() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/samples/demo-apartment.json");
        let text = std::fs::read_to_string(&path).expect("fixture");
        let doc = Document::from_json(&text).expect("parse");
        let state = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/x.json"));
        let msg = export_png(&state, Path::new("/tmp/fp-demo.png")).expect("export demo");
        assert!(msg.contains("含标注"), "{msg}");
        let bom = bom_csv(&state);
        assert!(bom.contains("承重墙") || bom.contains("外墙"), "{bom}");
        assert!(bom.contains("沙发") || bom.contains("床"), "{bom}");
    }

    #[test]
    #[ignore = "needs a GPU; writes preview files"]
    fn export_template_probes() {
        for (kind, stem) in [
            (crate::templates::TemplateKind::Studio, "studio"),
            (crate::templates::TemplateKind::TwoBed, "two-bed"),
        ] {
            let doc = crate::templates::build(kind, crate::i18n::Lang::Zh);
            let mut state =
                SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/template.json"));
            state.sun = 0.42;
            export_png(&state, Path::new(&format!("/tmp/fp-{stem}-2d.png"))).expect("template 2d");
            export_png_3d(&state, Path::new(&format!("/tmp/fp-{stem}-3d.png")))
                .expect("template 3d");
        }
    }

    #[test]
    #[ignore]
    fn export_shadow_probe() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = fp_model::Document::from_json(json).expect("demo json");
        let mut state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/p.json"));
        state.sun = 0.08;
        export_png_3d(&state, Path::new("/tmp/fp-shadow-probe.png")).expect("shadow probe");
    }

    #[test]
    #[ignore]
    fn export_env_probe() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = fp_model::Document::from_json(json).expect("demo json");
        let mut state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/p.json"));
        state.sun = 0.55;
        const W: u32 = 1600;
        const H: u32 = 1000;
        let mut cam = fp_render3d::OrbitCamera::new(glam::Vec3::new(7.55, 0.9, 8.0));
        cam.distance = 6.6;
        cam.yaw = 0.92;
        cam.pitch = 1.01;
        cam.fov_y = 40f32.to_radians();
        let view_proj = cam.view_proj(W as f32 / H as f32);
        let mesh = scene_mesh_full(
            &state.doc.walls,
            &state.doc.openings,
            &state.doc.rooms,
            &state.doc.materials,
            &state.doc.furniture,
        );
        render_3d_frame(
            &mesh.vertices,
            Path::new("/tmp/fp-env-probe.png"),
            W,
            H,
            view_proj,
            cam.eye().to_array(),
            state.look3d(),
        )
        .expect("env probe");
    }

    #[test]
    #[ignore]
    fn export_glass_probe() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = fp_model::Document::from_json(json).expect("demo json");
        let state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/p.json"));
        const W: u32 = 1600;
        const H: u32 = 1000;
        let mut cam = fp_render3d::OrbitCamera::new(glam::Vec3::new(7.3, 1.6, 10.6));
        cam.distance = 4.6;
        cam.yaw = 0.55;
        cam.pitch = 0.08;
        cam.fov_y = 52f32.to_radians();
        let view_proj = cam.view_proj(W as f32 / H as f32);
        let mesh = scene_mesh_full(
            &state.doc.walls,
            &state.doc.openings,
            &state.doc.rooms,
            &state.doc.materials,
            &state.doc.furniture,
        );
        render_3d_frame(
            &mesh.vertices,
            Path::new("/tmp/fp-glass-probe.png"),
            W,
            H,
            view_proj,
            cam.eye().to_array(),
            state.look3d(),
        )
        .expect("glass probe");
    }

    #[test]
    #[ignore]
    fn export_night_probe() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = fp_model::Document::from_json(json).expect("demo json");
        let mut state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/p.json"));
        state.night = true;
        export_png_3d(&state, Path::new("/tmp/fp-night-probe.png")).expect("night probe");
    }

    #[test]
    #[ignore = "needs a GPU; run with --ignored"]
    fn export_3d_renders_the_demo() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/samples/demo-apartment.json");
        let text = std::fs::read_to_string(&path).expect("fixture");
        let doc = Document::from_json(&text).expect("parse");
        let state = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/x.json"));
        let msg = export_png_3d(&state, Path::new("/tmp/fp-demo-3d.png")).expect("export 3d");
        assert!(msg.contains("1920"), "{msg}");
        let len = std::fs::metadata("/tmp/fp-demo-3d.png").expect("png").len();
        assert!(len > 20_000, "png is {len} bytes");
    }

    #[test]
    #[ignore = "needs a GPU; run with --ignored"]
    fn export_writes_a_real_pdf() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/samples/demo-apartment.json");
        let text = std::fs::read_to_string(&path).expect("fixture");
        let doc = Document::from_json(&text).expect("parse");
        let state = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/x.json"));
        let out = PathBuf::from("/tmp/fp-export-test.pdf");
        let msg = export_pdf(&state, &out).expect("export pdf");
        assert!(msg.contains("PDF"), "{msg}");
        let bytes = std::fs::read(&out).expect("pdf exists");
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.len() > 100_000, "pdf is {} bytes", bytes.len());
    }

    #[test]
    #[ignore = "needs a GPU; run with --ignored"]
    fn export_writes_a_real_png() {
        let mut doc = Document::new("export-test");
        let ids = [1u128, 2, 3, 4];
        let pts = [
            (Pt::new(0, 0), Pt::new(6000, 0)),
            (Pt::new(6000, 0), Pt::new(6000, 4000)),
            (Pt::new(6000, 4000), Pt::new(0, 4000)),
            (Pt::new(0, 4000), Pt::new(0, 0)),
        ];
        for (i, (a, b)) in pts.into_iter().enumerate() {
            doc.walls
                .push(Wall::new(EntityId(ids[i]), a, b, 120, WallKind::Partition));
        }
        let state = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/x.json"));
        let path = PathBuf::from("/tmp/fp-export-test.png");
        let msg = export_png(&state, &path).expect("export");
        assert!(msg.contains("×"));
        let len = std::fs::metadata(&path).expect("png exists").len();
        assert!(len > 10_000, "png is {len} bytes");
    }

    #[test]
    #[ignore]
    fn walk_frame_renders_room_interior() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = fp_model::Document::from_json(json).expect("demo json");
        let state = SceneState::from_doc(
            doc,
            [1280.0, 720.0],
            std::path::PathBuf::from("/tmp/fp-walk.json"),
        );
        let path = std::path::Path::new("/tmp/fp-walk.png");
        let cam = state.walk;
        render_3d_frame(
            &state.mesh3d_walk(),
            path,
            1280,
            720,
            cam.view_proj(1280.0 / 720.0),
            cam.pos.to_array(),
            state.look3d(),
        )
        .expect("walk frame");

        let file = std::fs::File::open(path).expect("png file");
        let decoder = png::Decoder::new(file);
        let mut reader = decoder.read_info().expect("png info");
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("png frame");
        let row = info.line_size as usize;
        let bpp = row / info.width as usize;
        let corner = [buf[0], buf[1], buf[2]];
        let mut diff = 0usize;
        for y in 0..info.height as usize {
            for x in 0..info.width as usize {
                let i = y * row + x * bpp;
                let d = (buf[i] as i32 - corner[0] as i32).abs()
                    + (buf[i + 1] as i32 - corner[1] as i32).abs()
                    + (buf[i + 2] as i32 - corner[2] as i32).abs();
                if d > 24 {
                    diff += 1;
                }
            }
        }
        let ratio = diff as f32 / (info.width as f32 * info.height as f32);
        assert!(
            ratio > 0.2,
            "only {ratio:.3} of the walk frame is scene geometry"
        );
    }
}
