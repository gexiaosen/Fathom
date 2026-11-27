//! PNG export: render the plan offscreen into a texture and save it.

use std::path::Path;

use crate::quote::{Quote, money};
use crate::render3d::Renderer3d;
use fp_core::{EntityId, Pt};
use fp_model::catalog::material_color;
use fp_model::{Document, FinishSurface, Material, OpeningKind, Room, WallFinish, WallKind};
use fp_render2d::{Batch, Camera2D};
use fp_render3d::scene_mesh_full;
use wgpu::util::DeviceExt;

use crate::scene::{SceneState, plan_bounds, plan_bounds_for_floor};

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
    let labelled = overlay_labels(
        &mut rgba,
        width,
        height,
        state,
        &cam,
        state.floor(),
        state.doc.settings.show_room_names,
        state.doc.settings.show_dimensions,
    );
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

/// A3 landscape construction sheet. Every storey gets its own framed plan in
/// the sheet, with overall dimensions, opening tags, door/window schedule,
/// room finish schedule, material schedule and a title block.
pub fn export_construction_pdf(state: &SceneState, path: &Path) -> Result<String, String> {
    let kinds = sheet_kinds(&state.doc);
    let mut pages = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let (width, height, rgba) = construction_sheet(state, kind)?;
        pages.push((width, height, rgba));
    }
    let page_count = pages.len();
    let pdf = pdf_from_pages(&pages, 1191.0, 842.0)?;
    std::fs::write(path, pdf).map_err(|e| format!("施工图写入失败: {e}"))?;
    Ok(format!(
        "已导出施工图 · A3 · {page_count} 页 · {} 层 · {} 个房间 · {} 个门窗",
        state.doc.level_count(),
        state.doc.rooms.len(),
        state.doc.openings.len()
    ))
}

/// One page in the A3 construction set. Plans and finishes are split per
/// storey so each drawing keeps a readable scale instead of shrinking all
/// floors into a single sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SheetKind {
    Cover,
    Plan(u32),
    Finish(u32),
    Elevations,
    Section,
    FinishSchedule,
    Details,
}

impl SheetKind {
    fn floor(self) -> Option<u32> {
        match self {
            Self::Cover
            | Self::Elevations
            | Self::Section
            | Self::FinishSchedule
            | Self::Details => None,
            Self::Plan(floor) | Self::Finish(floor) => Some(floor),
        }
    }

    fn title(self, doc: &Document) -> String {
        match self {
            Self::Cover => "住宅室内施工图 · 图纸目录".to_string(),
            Self::Plan(floor) => format!("{} 平面布置图", doc.level_name(floor as usize)),
            Self::Finish(floor) => format!("{} 地面铺装图", doc.level_name(floor as usize)),
            Self::Elevations => "四向立面图".to_string(),
            Self::Section => "1-1 纵向剖面图".to_string(),
            Self::FinishSchedule => "空间饰面表".to_string(),
            Self::Details => "节点大样".to_string(),
        }
    }

    fn code(self) -> String {
        match self {
            Self::Cover => "F-00  /  COVER".to_string(),
            Self::Plan(floor) => format!("F-{:02}  /  PLAN", (floor + 1) * 2 - 1),
            Self::Finish(floor) => format!("F-{:02}  /  FINISH", (floor + 1) * 2),
            Self::Elevations => "A-01  /  ELEVATION".to_string(),
            Self::Section => "A-02  /  SECTION".to_string(),
            Self::FinishSchedule => "A-03  /  FINISH SCHEDULE".to_string(),
            Self::Details => "D-01  /  DETAIL".to_string(),
        }
    }

    fn drawing_no(self) -> String {
        match self {
            Self::Cover => "F-00".to_string(),
            Self::Plan(floor) => format!("F-{:02}", (floor + 1) * 2 - 1),
            Self::Finish(floor) => format!("F-{:02}", (floor + 1) * 2),
            Self::Elevations => "A-01".to_string(),
            Self::Section => "A-02".to_string(),
            Self::FinishSchedule => "A-03".to_string(),
            Self::Details => "D-01".to_string(),
        }
    }

    fn caption(self) -> String {
        match self {
            Self::Cover => "封面".to_string(),
            Self::Plan(_) => "平面图".to_string(),
            Self::Finish(_) => "地面铺装".to_string(),
            Self::Elevations => "立面图".to_string(),
            Self::Section => "剖面图".to_string(),
            Self::FinishSchedule => "空间饰面表".to_string(),
            Self::Details => "节点大样".to_string(),
        }
    }
}

fn sheet_kinds(doc: &Document) -> Vec<SheetKind> {
    let levels = doc.level_count().max(1) as u32;
    let mut kinds = Vec::with_capacity(1 + levels as usize * 2 + 4);
    kinds.push(SheetKind::Cover);
    for floor in 0..levels {
        kinds.push(SheetKind::Plan(floor));
        kinds.push(SheetKind::Finish(floor));
    }
    kinds.push(SheetKind::Elevations);
    kinds.push(SheetKind::Section);
    kinds.push(SheetKind::FinishSchedule);
    kinds.push(SheetKind::Details);
    kinds
}

fn sheet_index(doc: &Document) -> Vec<(String, String, String)> {
    sheet_kinds(doc)
        .into_iter()
        .map(|kind| {
            let note = match kind {
                SheetKind::Cover => "项目信息 · 图纸清单",
                SheetKind::Plan(_) => "墙体、门窗、家具、尺寸与门窗表",
                SheetKind::Finish(_) => "材质分格、房间铺装表与材料用量",
                SheetKind::Elevations => "南、北、东、西四向立面与屋顶轮廓",
                SheetKind::Section => "1-1 剖切、楼层标高与屋面构造",
                SheetKind::FinishSchedule => "按空间汇总墙面、地面、顶面饰面",
                SheetKind::Details => "楼面、窗台、门槛石与檐口节点",
            };
            (kind.drawing_no(), kind.title(doc), note.to_string())
        })
        .collect()
}

pub const SHEET_W: u32 = 2800;
pub const SHEET_H: u32 = 1900;

const PAPER: [f32; 3] = [0.965, 0.957, 0.941];
const INK: [f32; 3] = [0.10, 0.10, 0.09];
const MUTED_INK: [f32; 3] = [0.42, 0.41, 0.38];
const ACCENT: [f32; 3] = [0.72, 0.35, 0.13];
const RULE: [f32; 3] = [0.70, 0.68, 0.63];

fn sheet_canvas() -> Vec<u8> {
    let mut rgba = vec![0u8; (SHEET_W * SHEET_H * 4) as usize];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[0] = (PAPER[0] * 255.0) as u8;
        pixel[1] = (PAPER[1] * 255.0) as u8;
        pixel[2] = (PAPER[2] * 255.0) as u8;
        pixel[3] = 255;
    }
    rgba
}

fn sheet_page_number(doc: &Document, kind: SheetKind) -> (usize, usize) {
    let kinds = sheet_kinds(doc);
    let total = kinds.len();
    let number = kinds
        .iter()
        .position(|candidate| *candidate == kind)
        .map(|index| index + 1)
        .unwrap_or(1);
    (number, total)
}

fn draw_sheet_header(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    doc: &Document,
    kind: SheetKind,
) {
    stroke_rect(
        rgba,
        SHEET_W,
        SHEET_H,
        48.0,
        48.0,
        SHEET_W as f32 - 96.0,
        SHEET_H as f32 - 96.0,
        3.0,
        INK,
    );
    let (page, total) = sheet_page_number(doc, kind);
    let levels = doc.level_count().max(1);
    if let Some(p) = painter {
        text_left(p, rgba, "FATHOM / 方寸", 66.0, 94.0, 26.0, INK);
        text_left(p, rgba, "住宅室内施工图", 66.0, 132.0, 15.0, MUTED_INK);
        text_center(
            p,
            rgba,
            &kind.title(doc),
            SHEET_W as f32 * 0.5,
            102.0,
            42.0,
            INK,
        );
        text_right(
            p,
            rgba,
            &format!("A3  ·  {levels} 层  ·  第 {page}/{total} 页"),
            SHEET_W as f32 - 66.0,
            82.0,
            18.0,
            MUTED_INK,
        );
        text_right(
            p,
            rgba,
            &kind.code(),
            SHEET_W as f32 - 66.0,
            106.0,
            15.0,
            ACCENT,
        );
    }
    hline(rgba, SHEET_W, 66.0, SHEET_W as f32 - 66.0, 148.0, 2.0, INK);
}

fn construction_sheet(state: &SceneState, kind: SheetKind) -> Result<(u32, u32, Vec<u8>), String> {
    match kind {
        SheetKind::Cover => construction_cover(state, kind),
        SheetKind::Plan(_) | SheetKind::Finish(_) => construction_floor_sheet(state, kind),
        SheetKind::Elevations => construction_elevation_sheet(state, kind),
        SheetKind::Section => construction_section_sheet(state, kind),
        SheetKind::FinishSchedule => construction_finish_schedule(state, kind),
        SheetKind::Details => construction_details_sheet(state, kind),
    }
}

fn construction_cover(state: &SceneState, kind: SheetKind) -> Result<(u32, u32, Vec<u8>), String> {
    let mut rgba = sheet_canvas();
    let painter = TextPainter::load();
    let doc = &state.doc;
    let today = doc
        .meta
        .updated_at
        .get(0..10)
        .filter(|date| !date.is_empty())
        .map(str::to_string)
        .unwrap_or_else(current_date);

    draw_sheet_header(&mut rgba, painter.as_ref(), doc, kind);

    let index = sheet_index(doc);
    let pages = index.len();
    let levels = doc.level_count().max(1);
    let rooms = doc.rooms.len();
    let materials = doc.materials.len();
    let net_area: f64 = doc
        .rooms
        .iter()
        .map(|room| fp_geom::area(&room.polygon) / 1_000_000.0)
        .sum();

    // Project identity.
    if let Some(p) = &painter {
        text_left(
            p,
            &mut rgba,
            "项目 / PROJECT",
            120.0,
            288.0,
            14.0,
            MUTED_INK,
        );
        text_left(p, &mut rgba, &project_title(doc), 120.0, 352.0, 52.0, INK);
        let address = if doc.meta.address.is_empty() {
            "地址待填写".to_string()
        } else {
            doc.meta.address.clone()
        };
        text_left(p, &mut rgba, &address, 120.0, 404.0, 18.0, MUTED_INK);
        let client = if doc.meta.client.is_empty() {
            "—"
        } else {
            doc.meta.client.as_str()
        };
        text_left(
            p,
            &mut rgba,
            &format!("客户 / CLIENT   {client}"),
            120.0,
            438.0,
            15.0,
            MUTED_INK,
        );

        let stats = [
            (format!("{levels}"), "楼层 / LEVELS".to_string()),
            (format!("{rooms}"), "房间 / ROOMS".to_string()),
            (format!("{net_area:.1} ㎡"), "房间净面积 / NET".to_string()),
            (format!("{materials}"), "材料 / MATERIALS".to_string()),
            (format!("{pages}"), "图纸 / SHEETS".to_string()),
        ];
        let mut x = 1980.0f32;
        for (value, label) in &stats {
            text_left(p, &mut rgba, value, x, 330.0, 30.0, INK);
            text_left(p, &mut rgba, label, x, 362.0, 11.0, MUTED_INK);
            x += 170.0;
        }
    }
    hline(
        &mut rgba,
        SHEET_W,
        120.0,
        SHEET_W as f32 - 120.0,
        484.0,
        2.0,
        INK,
    );

    // Drawing index.
    let mut y = draw_section(
        &mut rgba,
        painter.as_ref(),
        120.0,
        534.0,
        SHEET_W as f32 - 240.0,
        "图纸目录 / DRAWING INDEX",
        &format!("{pages} 张"),
    );
    let index_cols = [86.0, 170.0, 760.0, 1230.0, 314.0];
    y = draw_table_header(
        &mut rgba,
        painter.as_ref(),
        120.0,
        y,
        &index_cols,
        &["序号", "图号", "图纸名称", "内容", "备注"],
    );
    for (i, (number, title, note)) in index.iter().take(13).enumerate() {
        y = draw_table_row(
            &mut rgba,
            painter.as_ref(),
            120.0,
            y,
            &index_cols,
            &[
                format!("{:02}", i + 1),
                number.clone(),
                title.clone(),
                note.clone(),
                if i == 0 {
                    "本册".to_string()
                } else {
                    String::new()
                },
            ],
            INK,
        );
    }
    if index.len() > 13 {
        y = draw_caption_line(
            &mut rgba,
            painter.as_ref(),
            122.0,
            y + 8.0,
            &format!("另有 {} 张图纸未列出", index.len() - 13),
        );
    }

    // Notes and scope.
    y += 34.0;
    y = draw_section(
        &mut rgba,
        painter.as_ref(),
        120.0,
        y,
        SHEET_W as f32 - 240.0,
        "图纸说明 / NOTES",
        "",
    );
    let notes = [
        "1. 本图纸集按楼层分页出图，F-00 为封面与图纸目录。",
        "2. 平面图单位为毫米，比例见各页比例尺；现场尺寸须复核。",
        "3. 地面、墙面与顶面饰面共用一套 finish_id；用量与报价见饰面表与报价单。",
        "4. 门窗编号与门窗表对应；洞口尺寸以现场复核后的最终尺寸为准。",
        "5. 立面、剖面、饰面表与节点见 A-01、A-02、A-03、D-01，屋面按当前坡屋顶设置出图。",
    ];
    if let Some(p) = &painter {
        for (i, note) in notes.iter().enumerate() {
            text_left(
                p,
                &mut rgba,
                note,
                122.0,
                y + 24.0 + i as f32 * 27.0,
                15.0,
                MUTED_INK,
            );
        }
    }

    // Finish legend, the quick cover reference for every wall/floor/ceiling
    // record used by the drawings and the quote.
    let palette_y = y + 196.0;
    if !doc.materials.is_empty() && palette_y < 1_450.0 {
        let mut materials: Vec<&Material> = doc.materials.iter().collect();
        materials.sort_by_key(|m| (finish_surface_rank(m.surface), finish_code(doc, m)));
        draw_section(
            &mut rgba,
            painter.as_ref(),
            120.0,
            palette_y,
            SHEET_W as f32 - 240.0,
            "饰面图例 / FINISH LEGEND",
            &format!("{} 种", materials.len()),
        );
        if let Some(p) = &painter {
            for (i, material) in materials.iter().take(12).enumerate() {
                let col = i % 4;
                let row = i / 4;
                let x = 132.0 + col as f32 * 620.0;
                let top = palette_y + 32.0 + row as f32 * 50.0;
                let color = material_color(material);
                fill_rect(&mut rgba, SHEET_W, SHEET_H, x, top, 28.0, 28.0, color, 1.0);
                stroke_rect(&mut rgba, SHEET_W, SHEET_H, x, top, 28.0, 28.0, 1.0, RULE);
                text_left(
                    p,
                    &mut rgba,
                    &format!("{}  {}", finish_code(doc, material), material.name),
                    x + 38.0,
                    top + 14.0,
                    14.0,
                    INK,
                );
                text_left(
                    p,
                    &mut rgba,
                    &format!(
                        "{} · {} · ¥{:.0}/㎡",
                        finish_surface_cn(material.surface),
                        material_kind_cn(&material.kind),
                        material.price_per_m2
                    ),
                    x + 38.0,
                    top + 30.0,
                    10.0,
                    MUTED_INK,
                );
            }
            if materials.len() > 12 {
                text_left(
                    p,
                    &mut rgba,
                    &format!("另有 {} 种，见 A-03 饰面材料表", materials.len() - 12),
                    132.0,
                    palette_y + 194.0,
                    11.0,
                    MUTED_INK,
                );
            }
        }
    }

    // Scope and revision fill the lower half of the cover.
    let scope_y = (palette_y + 190.0).max(1_360.0);
    if scope_y < 1_460.0 {
        let cards = [
            ("01  平面布置", "墙体、门窗、家具", "尺寸链与门窗表"),
            ("02  地面铺装", "材质、分格与收口", "逐层铺装与用量"),
            ("03  饰面表", "墙面、地面、顶面", "A-03 编号与色卡"),
            ("04  立面与节点", "四向立面、纵向剖面", "檐口与收口大样"),
        ];
        y = draw_section(
            &mut rgba,
            painter.as_ref(),
            120.0,
            scope_y,
            SHEET_W as f32 - 240.0,
            "设计范围 / SCOPE",
            "",
        );
        if let Some(p) = &painter {
            for (i, (title, line1, line2)) in cards.iter().enumerate() {
                let x = 132.0 + i as f32 * 632.0;
                text_left(p, &mut rgba, title, x, y + 28.0, 18.0, INK);
                text_left(p, &mut rgba, line1, x, y + 54.0, 12.0, MUTED_INK);
                text_left(p, &mut rgba, line2, x, y + 74.0, 12.0, MUTED_INK);
            }
        }

        let revision_y = y + 132.0;
        if revision_y < 1_650.0 {
            y = draw_section(
                &mut rgba,
                painter.as_ref(),
                120.0,
                revision_y,
                SHEET_W as f32 - 240.0,
                "修订记录 / REVISION",
                "",
            );
            let rev_cols = [120.0, 260.0, 1500.0, 680.0];
            y = draw_table_header(
                &mut rgba,
                painter.as_ref(),
                120.0,
                y,
                &rev_cols,
                &["版本", "日期", "修订说明", "出图"],
            );
            y = draw_table_row(
                &mut rgba,
                painter.as_ref(),
                120.0,
                y,
                &rev_cols,
                &[
                    "V1.0".to_string(),
                    today.clone(),
                    "初版施工图图纸集".to_string(),
                    "FATHOM".to_string(),
                ],
                INK,
            );
            let _ = y;
        }
    }

    draw_sheet_title_block(&mut rgba, painter.as_ref(), doc, kind, &today);
    Ok((SHEET_W, SHEET_H, rgba))
}

fn construction_floor_sheet(
    state: &SceneState,
    kind: SheetKind,
) -> Result<(u32, u32, Vec<u8>), String> {
    let mut rgba = sheet_canvas();
    let painter = TextPainter::load();
    let doc = &state.doc;
    let today = doc
        .meta
        .updated_at
        .get(0..10)
        .filter(|date| !date.is_empty())
        .map(str::to_string)
        .unwrap_or_else(current_date);
    draw_sheet_header(&mut rgba, painter.as_ref(), doc, kind);

    // One storey per plan sheet. A full-width field gives the drawing a
    // readable scale instead of shrinking every floor into a grid.
    const PLAN_X: f32 = 66.0;
    const PLAN_Y: f32 = 176.0;
    const PLAN_W: f32 = 1810.0;
    const PLAN_H: f32 = 1514.0;
    let floor = kind.floor().unwrap_or(0);
    let caption_h = 42u32;
    let plan_h = PLAN_H as u32 - caption_h - 8;
    let (min, max) = plan_bounds_for_floor(doc, floor);
    let mut cam = Camera2D::new([PLAN_W, plan_h as f32]);
    cam.fit(min, max, 96.0);
    let batch = match kind {
        SheetKind::Plan(_) => state.render_batch_on(floor, &cam, &[], &[], None),
        SheetKind::Finish(_) => state.render_batch_finish_on(floor, &cam, None),
        _ => unreachable!(),
    };
    let mut tile = render_to_rgba(&batch, PLAN_W as u32, plan_h)?;
    if let Some(p) = &painter {
        match kind {
            SheetKind::Plan(_) => {
                overlay_labels(
                    &mut tile,
                    PLAN_W as u32,
                    plan_h,
                    state,
                    &cam,
                    floor,
                    true,
                    false,
                );
                draw_opening_tags(&mut tile, PLAN_W as u32, plan_h, p, &cam, doc, floor);
            }
            SheetKind::Finish(_) => {
                draw_finish_patterns(&mut tile, PLAN_W as u32, plan_h, &cam, doc, floor);
                overlay_finish_labels(&mut tile, PLAN_W as u32, plan_h, p, &cam, doc, floor);
            }
            _ => unreachable!(),
        }
        if let Some((min, max)) = floor_drawing_bounds(doc, floor) {
            draw_dimensions(&mut tile, PLAN_W as u32, plan_h, p, &cam, min, max);
        }
    }
    blit_rgba(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        &tile,
        PLAN_W as u32,
        plan_h,
        PLAN_X as u32,
        PLAN_Y as u32 + caption_h,
    );

    if let Some(p) = &painter {
        text_left(
            p,
            &mut rgba,
            &format!("{}  {}", doc.level_name(floor as usize), kind.caption()),
            PLAN_X + 18.0,
            PLAN_Y + 30.0,
            25.0,
            INK,
        );
        text_left(
            p,
            &mut rgba,
            &floor_stats(doc, floor),
            PLAN_X + 18.0,
            PLAN_Y + 62.0,
            15.0,
            MUTED_INK,
        );
    }
    hline(
        &mut rgba,
        SHEET_W,
        PLAN_X + 16.0,
        PLAN_X + PLAN_W - 16.0,
        PLAN_Y + caption_h as f32 - 2.0,
        1.0,
        RULE,
    );
    draw_scale_bar(
        &mut rgba,
        PLAN_X + PLAN_W - 292.0,
        PLAN_Y + 17.0,
        cam.scale,
        painter.as_ref(),
    );

    // Right column schedules.
    let sx = 1938.0f32;
    let sw = SHEET_W as f32 - 66.0 - sx;
    if matches!(kind, SheetKind::Plan(_)) {
        let mut y = 184.0f32;
        let openings: Vec<OpeningEntry> = opening_entries(doc)
            .into_iter()
            .filter(|entry| entry.floor == floor)
            .collect();
        y = draw_section(
            &mut rgba,
            painter.as_ref(),
            sx,
            y,
            sw,
            "门窗表 / OPENINGS",
            &format!("{} 个", openings.len()),
        );
        let door_rows: Vec<&OpeningEntry> = openings.iter().take(13).collect();
        let door_cols = [78.0, 96.0, 220.0, 96.0, 74.0, 74.0, 158.0];
        y = draw_table_header(
            &mut rgba,
            painter.as_ref(),
            sx,
            y,
            &door_cols,
            &["编号", "类型", "洞口 W×H", "窗台", "楼层", "数量", "单价"],
        );
        for entry in &door_rows {
            y = draw_table_row(
                &mut rgba,
                painter.as_ref(),
                sx,
                y,
                &door_cols,
                &[
                    entry.tag.clone(),
                    opening_kind_label(entry.kind).to_string(),
                    format!("{}×{}", entry.width, entry.height),
                    format!("{}", entry.sill),
                    doc.level_name(entry.floor as usize),
                    "1".to_string(),
                    money(entry.price),
                ],
                INK,
            );
        }
        if openings.len() > door_rows.len() {
            y = draw_caption_line(
                &mut rgba,
                painter.as_ref(),
                sx + 2.0,
                y + 8.0,
                &format!(
                    "另有 {} 个门窗，详见报价清单",
                    openings.len() - door_rows.len()
                ),
            );
        }

        y += 26.0;
        y = draw_section(
            &mut rgba,
            painter.as_ref(),
            sx,
            y,
            sw,
            "房间做法表 / ROOMS",
            &format!("{} 间", doc.rooms.len()),
        );
        let room_cols = [320.0, 110.0, 300.0, 66.0];
        y = draw_table_header(
            &mut rgba,
            painter.as_ref(),
            sx,
            y,
            &room_cols,
            &["房间", "面积", "地面材质", "楼层"],
        );
        let rooms: Vec<&fp_model::Room> = doc
            .rooms
            .iter()
            .filter(|r| r.floor == floor)
            .take(10)
            .collect();
        for room in &rooms {
            let material = room
                .material_id
                .and_then(|id| doc.materials.iter().find(|m| m.id == id))
                .map(|m| m.name.as_str())
                .unwrap_or("—");
            let area = fp_geom::area(&room.polygon) / 1.0e6;
            y = draw_table_row(
                &mut rgba,
                painter.as_ref(),
                sx,
                y,
                &room_cols,
                &[
                    room.name.clone(),
                    format!("{area:.1} m²"),
                    material.to_string(),
                    doc.level_name(room.floor as usize),
                ],
                INK,
            );
        }
        let floor_room_count = doc.rooms.iter().filter(|r| r.floor == floor).count();
        if floor_room_count > rooms.len() {
            y = draw_caption_line(
                &mut rgba,
                painter.as_ref(),
                sx + 2.0,
                y + 8.0,
                &format!("另有 {} 间未列出", floor_room_count - rooms.len()),
            );
        }

        y += 26.0;
        let wall_finishes = floor_wall_finishes(doc, floor);
        y = draw_section(
            &mut rgba,
            painter.as_ref(),
            sx,
            y,
            sw,
            "墙饰面图例 / WALL FINISHES",
            &format!("{} 种", wall_finishes.len()),
        );
        if wall_finishes.is_empty() {
            y = draw_caption_line(
                &mut rgba,
                painter.as_ref(),
                sx + 2.0,
                y + 12.0,
                "本层墙体未设置饰面",
            );
        } else {
            let legend_cols = [74.0, 74.0, 290.0, 150.0, 164.0];
            y = draw_table_header(
                &mut rgba,
                painter.as_ref(),
                sx,
                y,
                &legend_cols,
                &["编号", "颜色", "名称", "类型", "本层用墙"],
            );
            for material in wall_finishes.iter().take(6) {
                let walls_used = doc
                    .walls
                    .iter()
                    .filter(|w| w.floor == floor)
                    .filter(|w| wall_finish_material(doc, w).map(|m| m.id) == Some(material.id))
                    .count();
                y = draw_finish_material_row(
                    &mut rgba,
                    painter.as_ref(),
                    sx,
                    y,
                    &legend_cols,
                    &[
                        finish_code(doc, material),
                        String::new(),
                        material.name.clone(),
                        material_kind_cn(&material.kind).to_string(),
                        format!("{walls_used} 面"),
                    ],
                    material_color(material),
                    1,
                );
            }
        }
    } else {
        draw_finish_schedules(
            &mut rgba,
            painter.as_ref(),
            doc,
            kind.floor().unwrap_or(0),
            sx,
            184.0,
            sw,
        );
    }

    // North arrow in the free top-right margin of the plan field.
    draw_north_arrow(
        &mut rgba,
        PLAN_X + PLAN_W - 26.0,
        PLAN_Y + 112.0,
        painter.as_ref(),
    );

    draw_sheet_title_block(&mut rgba, painter.as_ref(), doc, kind, &today);

    Ok((SHEET_W, SHEET_H, rgba))
}

/// Building extents in metres, including the roof eave, for elevations and
/// sections. The drawing set always shows the roof defined in the document,
/// independent of the 3D view's roof visibility toggle.
fn building_extents(doc: &Document) -> (f32, f32, f32, f32, f32) {
    let (min, max) = plan_bounds(doc);
    let over = doc.roof.normalized().overhang_mm as f32 / 1000.0;
    (
        min.x as f32 / 1000.0 - over,
        max.x as f32 / 1000.0 + over,
        min.y as f32 / 1000.0 - over,
        max.y as f32 / 1000.0 + over,
        over,
    )
}

fn roof_rise(doc: &Document, span: f32) -> f32 {
    let roof = doc.roof.normalized();
    match roof.kind {
        fp_model::RoofKind::Flat => 0.16,
        fp_model::RoofKind::Gable => (span * 0.5) * roof.pitch_deg.to_radians().tan(),
    }
}

/// The same roof mesh the 3D view uses, so the L-shaped footprint also reads
/// correctly in the elevation sheets instead of falling back to a bounding box.
/// Intersect the roof mesh with the section plane, returning 2D segments in
/// (horizontal, height) metres. This keeps the section profile true where the
/// footprint is L-shaped instead of drawing a full-width gable triangle.
fn roof_mesh_section(
    mesh: &[fp_render3d::MeshVertex],
    cut_across_y: bool,
    fixed: f32,
) -> Vec<([f32; 2], [f32; 2])> {
    fn axis(p: [f32; 3], cut_across_y: bool) -> f32 {
        if cut_across_y { p[0] } else { p[2] }
    }
    fn point(p: [f32; 3], cut_across_y: bool) -> [f32; 2] {
        if cut_across_y {
            [p[2], p[1]]
        } else {
            [p[0], p[1]]
        }
    }
    let mut segments = Vec::new();
    for tri in mesh.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let p = [tri[0].pos, tri[1].pos, tri[2].pos];
        let d = [
            axis(p[0], cut_across_y) - fixed,
            axis(p[1], cut_across_y) - fixed,
            axis(p[2], cut_across_y) - fixed,
        ];
        let mut hits: Vec<[f32; 2]> = Vec::new();
        for i in 0..3 {
            let j = (i + 1) % 3;
            if d[i].abs() < 1e-5 {
                hits.push(point(p[i], cut_across_y));
            }
            if d[i] * d[j] < 0.0 {
                let t = d[i] / (d[i] - d[j]);
                let q = [
                    p[i][0] + (p[j][0] - p[i][0]) * t,
                    p[i][1] + (p[j][1] - p[i][1]) * t,
                    p[i][2] + (p[j][2] - p[i][2]) * t,
                ];
                hits.push(point(q, cut_across_y));
            }
        }
        if hits.len() < 2 {
            continue;
        }
        let (a, b) = (hits[0], hits[1]);
        if (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4 {
            continue;
        }
        segments.push((a, b));
    }
    segments
}

fn roof_mesh_for_sheet(doc: &Document, eave_h: f32) -> Vec<fp_render3d::MeshVertex> {
    let levels = doc.level_count().max(1);
    let top = (levels - 1) as u32;
    let rooms: Vec<fp_model::Room> = doc
        .rooms
        .iter()
        .filter(|room| room.floor == top)
        .cloned()
        .collect();
    let roof = doc.roof.normalized();
    fp_render3d::roof_mesh_for(&rooms, eave_h, roof.kind, roof.pitch_deg, roof.overhang_mm)
}

#[allow(clippy::too_many_arguments)]
fn fill_gable_triangle(
    rgba: &mut [u8],
    left: f32,
    right: f32,
    base_y: f32,
    eave_h: f32,
    rise: f32,
    scale: f32,
    color: [f32; 3],
) {
    let mid = (left + right) * 0.5;
    let half = ((right - left) * 0.5).max(1.0);
    let mut x = left;
    while x < right {
        let t = 1.0 - ((x + 1.0 - mid).abs() / half).min(1.0);
        let top = base_y - (eave_h + rise * t) * scale;
        let bottom = base_y - eave_h * scale;
        fill_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            x,
            top,
            2.0,
            bottom - top,
            color,
            1.0,
        );
        x += 2.0;
    }
}

/// One elevation panel. `dir`: 0 south, 1 north, 2 east, 3 west.
#[allow(clippy::too_many_arguments)]
fn draw_elevation_panel(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    doc: &Document,
    dir: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    roof_mesh: &[fp_render3d::MeshVertex],
) {
    const WALL_LIGHT: [f32; 3] = [0.82, 0.80, 0.76];
    const WALL_BEARING: [f32; 3] = [0.72, 0.69, 0.64];
    const GLASS: [f32; 3] = [0.68, 0.79, 0.83];
    const DOOR: [f32; 3] = [0.55, 0.43, 0.31];
    const ROOF: [f32; 3] = [0.56, 0.52, 0.47];
    const GROUND: [f32; 3] = [0.38, 0.36, 0.32];
    let (x0, x1, z0, z1, _) = building_extents(doc);
    let levels = doc.level_count().max(1);
    let top = levels - 1;
    let eave_h = (doc.level_elevation_mm(top) + doc.level_height_mm(top)) as f32 / 1000.0;
    let span_x = (x1 - x0).max(0.5);
    let span_z = (z1 - z0).max(0.5);
    let ridge_along_x = span_x >= span_z;
    let gable = doc.roof.normalized().kind == fp_model::RoofKind::Gable;
    let rise = if gable {
        roof_rise(doc, if ridge_along_x { span_z } else { span_x })
    } else {
        0.16
    };
    let draw_x = x + 46.0;
    let draw_y = y + 84.0;
    let draw_w = w - 92.0;
    let draw_h = h - 136.0;

    stroke_rect(rgba, SHEET_W, SHEET_H, x, y, w, h, 1.0, RULE);
    if let Some(p) = painter {
        let names = [
            "南立面 / SOUTH",
            "北立面 / NORTH",
            "东立面 / EAST",
            "西立面 / WEST",
        ];
        text_left(p, rgba, names[dir], x + 16.0, y + 30.0, 20.0, INK);
    }

    let horizontal = dir < 2;
    let width = if horizontal { span_x } else { span_z };
    let total_h = eave_h + rise + 0.25;
    let scale = (draw_w / (width + 0.8)).min(draw_h / (total_h + 0.8));
    let left = draw_x + (draw_w - width * scale) * 0.5;
    let base_y = draw_y + draw_h - 28.0;
    let u0 = if horizontal { x0 } else { z0 };
    let project = |u: f32| left + (u - u0) * scale;
    let height_y = |hm: f32| base_y - hm * scale;

    // Facade silhouette.
    let envelope = if horizontal { [x0, x1] } else { [z0, z1] };
    fill_rect(
        rgba,
        SHEET_W,
        SHEET_H,
        project(envelope[0]),
        height_y(eave_h),
        (envelope[1] - envelope[0]) * scale,
        eave_h * scale,
        [0.88, 0.86, 0.82],
        1.0,
    );

    let edge = match dir {
        0 => z1,
        1 => z0,
        2 => x1,
        _ => x0,
    };
    let mut walls_drawn: Vec<EntityId> = Vec::new();
    let mut wall_entries: Vec<(f32, EntityId, f32, f32, f32, f32, bool)> = Vec::new();
    for wall in &doc.walls {
        let (su, eu) = if horizontal {
            (wall.start.x as f32 / 1000.0, wall.end.x as f32 / 1000.0)
        } else {
            (wall.start.y as f32 / 1000.0, wall.end.y as f32 / 1000.0)
        };
        let (depth_a, depth_b) = if horizontal {
            (wall.start.y as f32 / 1000.0, wall.end.y as f32 / 1000.0)
        } else {
            (wall.start.x as f32 / 1000.0, wall.end.x as f32 / 1000.0)
        };
        let depth = (depth_a + depth_b) * 0.5;
        let parallel = if horizontal {
            (wall.end.x - wall.start.x).abs() >= (wall.end.y - wall.start.y).abs()
        } else {
            (wall.end.y - wall.start.y).abs() > (wall.end.x - wall.start.x).abs()
        };
        if !parallel || (depth - edge).abs() > 1.2 {
            continue;
        }
        let elevation = doc.level_elevation_mm(wall.floor as usize) as f32 / 1000.0;
        let top_h = elevation + wall.height_mm as f32 / 1000.0;
        wall_entries.push((
            depth,
            wall.id,
            su.min(eu),
            su.max(eu),
            elevation,
            top_h,
            wall.kind == WallKind::Bearing,
        ));
    }
    wall_entries.sort_by(|a, b| {
        let far_first = match dir {
            0 | 2 => a.0.partial_cmp(&b.0),
            _ => b.0.partial_cmp(&a.0),
        };
        far_first.unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut counts = (0usize, 0usize);
    for (_, id, uu0, uu1, base, top_h, bearing) in &wall_entries {
        walls_drawn.push(*id);
        let sx = project(*uu0);
        let ex = project(*uu1);
        let color = if *bearing { WALL_BEARING } else { WALL_LIGHT };
        fill_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            sx,
            height_y(*top_h),
            ex - sx,
            (*top_h - *base) * scale,
            color,
            1.0,
        );
        stroke_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            sx,
            height_y(*top_h),
            ex - sx,
            (*top_h - *base) * scale,
            1.0,
            RULE,
        );
        if let Some(p) = painter {
            let face_w = ex - sx;
            let face_h = (*top_h - *base) * scale;
            if face_w >= 76.0 && face_h >= 42.0 {
                if let Some(wall) = doc.walls.iter().find(|w| w.id == *id) {
                    let label = wall_finish_label(doc, wall);
                    if label != "—" {
                        text_center(
                            p,
                            rgba,
                            &label.split(' ').next().unwrap_or(""),
                            (sx + ex) * 0.5,
                            height_y((*base + *top_h) * 0.5) + 4.0,
                            12.0,
                            INK,
                        );
                    }
                }
            }
        }
    }

    // Storey divisions on the facade.
    for level in 1..levels {
        let elevation = doc.level_elevation_mm(level) as f32 / 1000.0;
        hline(
            rgba,
            SHEET_W,
            project(envelope[0]),
            project(envelope[1]),
            height_y(elevation),
            1.0,
            RULE,
        );
    }

    // Openings on the visible facade.
    for opening in &doc.openings {
        if !walls_drawn.contains(&opening.wall_id) {
            continue;
        }
        let Some(wall) = doc.walls.iter().find(|wall| wall.id == opening.wall_id) else {
            continue;
        };
        let len = ((wall.end.x - wall.start.x) as f32).hypot((wall.end.y - wall.start.y) as f32);
        if len < 1.0 {
            continue;
        }
        let delta = if horizontal {
            (wall.end.x - wall.start.x) as f32 / len
        } else {
            (wall.end.y - wall.start.y) as f32 / len
        };
        let wall_u = if horizontal {
            wall.start.x as f32 / 1000.0
        } else {
            wall.start.y as f32 / 1000.0
        };
        let o0 = wall_u + delta * opening.offset_mm as f32 / 1000.0;
        let o1 = o0 + delta * opening.width_mm as f32 / 1000.0;
        let base = doc.level_elevation_mm(wall.floor as usize) as f32 / 1000.0
            + opening.sill_mm as f32 / 1000.0;
        let top_h = base + opening.height_mm as f32 / 1000.0;
        let sx = project(o0.min(o1));
        let ex = project(o0.max(o1));
        let door = opening.kind == OpeningKind::Door;
        if door {
            counts.0 += 1;
        } else {
            counts.1 += 1;
        }
        fill_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            sx,
            height_y(top_h),
            ex - sx,
            (top_h - base) * scale,
            if door { DOOR } else { GLASS },
            1.0,
        );
        stroke_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            sx,
            height_y(top_h),
            ex - sx,
            (top_h - base) * scale,
            1.5,
            INK,
        );
        if !door {
            let mullion = (ex - sx) * 0.5;
            vline(
                rgba,
                SHEET_W,
                sx + mullion,
                height_y(top_h),
                height_y(base),
                1.0,
                INK,
            );
        }
    }

    // Roof silhouette projected from the actual roof mesh. Drawing the same
    // triangles keeps L-shaped and notched footprints from being flattened
    // back to a rectangle in the elevations.
    if !roof_mesh.is_empty() {
        let project_point = |p: [f32; 3]| {
            let u = if horizontal { p[0] } else { p[2] };
            [project(u), height_y(p[1])]
        };
        for tri in roof_mesh.chunks(3) {
            // Skip the downward-facing soffit, but keep vertical gable-end
            // faces: viewed from east/west they are what forms the roof head.
            if tri.len() < 3 || tri[0].normal[1] < -0.05 {
                continue;
            }
            fill_triangle(
                rgba,
                SHEET_W,
                SHEET_H,
                project_point(tri[0].pos),
                project_point(tri[1].pos),
                project_point(tri[2].pos),
                ROOF,
            );
        }
        hline(
            rgba,
            SHEET_W,
            left,
            left + width * scale,
            height_y(eave_h),
            2.0,
            INK,
        );
    } else if gable {
        let profile = if horizontal {
            !ridge_along_x
        } else {
            ridge_along_x
        };
        if profile {
            fill_gable_triangle(
                rgba,
                project(if horizontal { x0 } else { z0 }),
                project(if horizontal { x1 } else { z1 }),
                base_y,
                eave_h,
                rise,
                scale,
                ROOF,
            );
            hline(
                rgba,
                SHEET_W,
                project(if horizontal { x0 } else { z0 }),
                project(if horizontal { x1 } else { z1 }),
                height_y(eave_h),
                2.0,
                INK,
            );
        } else {
            let rx = project(if horizontal { x0 } else { z0 });
            let ry = height_y(eave_h + rise);
            fill_rect(
                rgba,
                SHEET_W,
                SHEET_H,
                rx,
                ry,
                width * scale,
                rise * scale,
                ROOF,
                1.0,
            );
            stroke_rect(
                rgba,
                SHEET_W,
                SHEET_H,
                rx,
                ry,
                width * scale,
                rise * scale,
                1.0,
                INK,
            );
            for course in 1..4 {
                let cy = height_y(eave_h + rise * course as f32 / 4.0);
                hline(
                    rgba,
                    SHEET_W,
                    rx,
                    rx + width * scale,
                    cy,
                    1.0,
                    [0.48, 0.45, 0.40],
                );
            }
        }
    } else {
        let rx = project(if horizontal { x0 } else { z0 });
        let ry = height_y(eave_h + 0.16);
        fill_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            rx,
            ry,
            width * scale,
            0.16 * scale,
            ROOF,
            1.0,
        );
        stroke_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            rx,
            ry,
            width * scale,
            0.16 * scale,
            1.0,
            INK,
        );
    }

    // Ground line and level markers.
    hline(
        rgba,
        SHEET_W,
        left - 20.0,
        left + width * scale + 20.0,
        base_y,
        3.0,
        GROUND,
    );
    let mut hatch = left - 10.0;
    while hatch < left + width * scale + 10.0 {
        hline(rgba, SHEET_W, hatch, hatch + 8.0, base_y + 9.0, 1.0, GROUND);
        hatch += 18.0;
    }
    if let Some(p) = painter {
        for level in 0..levels {
            let elevation = doc.level_elevation_mm(level) as f32 / 1000.0;
            let ty = height_y(elevation);
            text_right(
                p,
                rgba,
                &format!("{:.3}", elevation),
                left - 8.0,
                ty + 4.0,
                11.0,
                MUTED_INK,
            );
            hline(rgba, SHEET_W, left - 5.0, left + 4.0, ty, 1.0, MUTED_INK);
        }
        text_right(
            p,
            rgba,
            &format!("宽 {:.2} m", width),
            left + width * scale + 8.0,
            base_y + 30.0,
            11.0,
            MUTED_INK,
        );
        text_left(
            p,
            rgba,
            &format!(
                "1:{:.0} · 门 {} / 窗 {}",
                6667.0 / scale.max(1.0),
                counts.0,
                counts.1
            ),
            x + 16.0,
            y + 54.0,
            11.0,
            MUTED_INK,
        );
    }
}

fn construction_elevation_sheet(
    state: &SceneState,
    kind: SheetKind,
) -> Result<(u32, u32, Vec<u8>), String> {
    let mut rgba = sheet_canvas();
    let painter = TextPainter::load();
    let doc = &state.doc;
    let today = doc
        .meta
        .updated_at
        .get(0..10)
        .filter(|date| !date.is_empty())
        .map(str::to_string)
        .unwrap_or_else(current_date);
    draw_sheet_header(&mut rgba, painter.as_ref(), doc, kind);
    let levels = doc.level_count().max(1);
    let eave_h =
        (doc.level_elevation_mm(levels - 1) + doc.level_height_mm(levels - 1)) as f32 / 1000.0;
    let roof_mesh = roof_mesh_for_sheet(doc, eave_h);
    for dir in 0..4 {
        let x = if dir % 2 == 0 { 84.0 } else { 1420.0 };
        let y = if dir < 2 { 184.0 } else { 896.0 };
        draw_elevation_panel(
            &mut rgba,
            painter.as_ref(),
            doc,
            dir,
            x,
            y,
            1300.0,
            680.0,
            &roof_mesh,
        );
    }
    draw_sheet_title_block(&mut rgba, painter.as_ref(), doc, kind, &today);
    Ok((SHEET_W, SHEET_H, rgba))
}

fn construction_section_sheet(
    state: &SceneState,
    kind: SheetKind,
) -> Result<(u32, u32, Vec<u8>), String> {
    const WALL: [f32; 3] = [0.78, 0.76, 0.72];
    const SLAB: [f32; 3] = [0.55, 0.53, 0.49];
    const ROOF: [f32; 3] = [0.56, 0.52, 0.47];
    let mut rgba = sheet_canvas();
    let painter = TextPainter::load();
    let doc = &state.doc;
    let today = doc
        .meta
        .updated_at
        .get(0..10)
        .filter(|date| !date.is_empty())
        .map(str::to_string)
        .unwrap_or_else(current_date);
    draw_sheet_header(&mut rgba, painter.as_ref(), doc, kind);
    let x = 90.0;
    let y = 190.0;
    let w = 2620.0;
    let h = 1460.0;
    stroke_rect(&mut rgba, SHEET_W, SHEET_H, x, y, w, h, 1.0, RULE);
    let (x0, x1, z0, z1, _) = building_extents(doc);
    let levels = doc.level_count().max(1);
    let top = levels - 1;
    let eave_h = (doc.level_elevation_mm(top) + doc.level_height_mm(top)) as f32 / 1000.0;
    let span_x = (x1 - x0).max(0.5);
    let span_z = (z1 - z0).max(0.5);
    let ridge_x = span_x >= span_z;
    let cut_across_y = ridge_x;
    let width = if cut_across_y { span_z } else { span_x };
    let rise = if doc.roof.normalized().kind == fp_model::RoofKind::Gable {
        roof_rise(doc, width)
    } else {
        0.16
    };
    let draw_x = x + 90.0;
    let draw_y = y + 96.0;
    let draw_w = w - 180.0;
    let draw_h = h - 176.0;
    let total_h = eave_h + rise + 0.3;
    let scale = (draw_w / (width + 1.0)).min(draw_h / (total_h + 1.0));
    let left = draw_x + (draw_w - width * scale) * 0.5;
    let base_y = draw_y + draw_h - 45.0;
    let u0 = if cut_across_y { z0 } else { x0 };
    let project = |u: f32| left + (u - u0) * scale;
    let height_y = |hm: f32| base_y - hm * scale;

    if let Some(p) = painter.as_ref() {
        text_left(
            p,
            &mut rgba,
            "1-1 剖面图 / SECTION",
            x + 20.0,
            y + 32.0,
            22.0,
            INK,
        );
        text_left(
            p,
            &mut rgba,
            &format!(
                "剖切位置：{} · 1:{:.0}",
                if cut_across_y {
                    "建筑中部（南北向）"
                } else {
                    "建筑中部（东西向）"
                },
                6667.0 / scale.max(1.0)
            ),
            x + 20.0,
            y + 58.0,
            12.0,
            MUTED_INK,
        );
    }
    // Exterior walls.
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        left - 8.0,
        height_y(eave_h),
        16.0,
        eave_h * scale,
        WALL,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        left + width * scale - 8.0,
        height_y(eave_h),
        16.0,
        eave_h * scale,
        WALL,
        1.0,
    );
    // Background partitions beyond the cut plane, kept light so the cut
    // walls and floor slabs stay dominant.
    let fixed = if cut_across_y {
        (x0 + x1) * 0.5
    } else {
        (z0 + z1) * 0.5
    };
    let mut background: Vec<(f32, f32, f32, f32)> = Vec::new();
    for wall in &doc.walls {
        let (a, b, depth_a, depth_b) = if cut_across_y {
            (
                wall.start.y as f32 / 1000.0,
                wall.end.y as f32 / 1000.0,
                wall.start.x as f32 / 1000.0,
                wall.end.x as f32 / 1000.0,
            )
        } else {
            (
                wall.start.x as f32 / 1000.0,
                wall.end.x as f32 / 1000.0,
                wall.start.y as f32 / 1000.0,
                wall.end.y as f32 / 1000.0,
            )
        };
        let depth = (depth_a + depth_b) * 0.5;
        let crosses = if cut_across_y {
            let wa = wall.start.x as f32 / 1000.0;
            let wb = wall.end.x as f32 / 1000.0;
            wa.min(wb) - wall.thickness_mm as f32 / 1000.0 <= fixed
                && wa.max(wb) + wall.thickness_mm as f32 / 1000.0 >= fixed
                && (wall.start.y - wall.end.y).abs() > (wall.start.x - wall.end.x).abs()
        } else {
            let wa = wall.start.y as f32 / 1000.0;
            let wb = wall.end.y as f32 / 1000.0;
            wa.min(wb) - wall.thickness_mm as f32 / 1000.0 <= fixed
                && wa.max(wb) + wall.thickness_mm as f32 / 1000.0 >= fixed
                && (wall.start.x - wall.end.x).abs() > (wall.start.y - wall.end.y).abs()
        };
        if crosses || depth >= fixed {
            continue;
        }
        let elevation = doc.level_elevation_mm(wall.floor as usize) as f32 / 1000.0;
        let top_h = elevation + wall.height_mm as f32 / 1000.0;
        background.push((depth, a.min(b), a.max(b), elevation.max(top_h - 2.800)));
    }
    background.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, a, b, elevation) in &background {
        let top_h = *elevation + 2.800;
        let sx = project(*a);
        let ex = project(*b);
        fill_rect(
            &mut rgba,
            SHEET_W,
            SHEET_H,
            sx,
            height_y(top_h),
            ex - sx,
            (top_h - *elevation) * scale,
            [0.88, 0.86, 0.82],
            0.45,
        );
        stroke_rect(
            &mut rgba,
            SHEET_W,
            SHEET_H,
            sx,
            height_y(top_h),
            ex - sx,
            (top_h - *elevation) * scale,
            1.0,
            [0.72, 0.69, 0.64],
        );
    }

    // Floor slabs.
    for level in 0..levels {
        let elevation = doc.level_elevation_mm(level) as f32 / 1000.0;
        fill_rect(
            &mut rgba,
            SHEET_W,
            SHEET_H,
            left,
            height_y(elevation),
            width * scale,
            0.14 * scale,
            SLAB,
            1.0,
        );
        hline(
            &mut rgba,
            SHEET_W,
            left,
            left + width * scale,
            height_y(elevation),
            2.0,
            INK,
        );
        let slab_bottom = height_y(elevation) + 0.14 * scale;
        let mut hx = left + 30.0;
        while hx < left + width * scale - 20.0 {
            hline(
                &mut rgba,
                SHEET_W,
                hx,
                hx + 26.0,
                slab_bottom - 5.0,
                1.0,
                [0.40, 0.38, 0.34],
            );
            hx += 48.0;
        }
    }
    // Cut partitions.
    for wall in &doc.walls {
        let crosses = if cut_across_y {
            let mid = (x0 + x1) * 0.5;
            let a = wall.start.x as f32 / 1000.0;
            let b = wall.end.x as f32 / 1000.0;
            a.min(b) - wall.thickness_mm as f32 / 1000.0 <= mid
                && a.max(b) + wall.thickness_mm as f32 / 1000.0 >= mid
                && (wall.start.y - wall.end.y).abs() > (wall.start.x - wall.end.x).abs()
        } else {
            let mid = (z0 + z1) * 0.5;
            let a = wall.start.y as f32 / 1000.0;
            let b = wall.end.y as f32 / 1000.0;
            a.min(b) - wall.thickness_mm as f32 / 1000.0 <= mid
                && a.max(b) + wall.thickness_mm as f32 / 1000.0 >= mid
                && (wall.start.x - wall.end.x).abs() > (wall.start.y - wall.end.y).abs()
        };
        if !crosses {
            continue;
        }
        let u = if cut_across_y {
            (wall.start.y + wall.end.y) as f32 / 2000.0
        } else {
            (wall.start.x + wall.end.x) as f32 / 2000.0
        };
        let elevation = doc.level_elevation_mm(wall.floor as usize) as f32 / 1000.0;
        let top_h = elevation + wall.height_mm as f32 / 1000.0;
        let wpx = (wall.thickness_mm as f32 / 1000.0 * scale).clamp(3.0, 20.0);
        fill_rect(
            &mut rgba,
            SHEET_W,
            SHEET_H,
            project(u) - wpx * 0.5,
            height_y(top_h),
            wpx,
            (top_h - elevation) * scale,
            [0.56, 0.53, 0.49],
            1.0,
        );
    }
    // Roof section from the actual roof mesh. Fall back to the old bbox
    // shape only if the cut plane misses the roof entirely.
    let fixed = if cut_across_y {
        (x0 + x1) * 0.5
    } else {
        (z0 + z1) * 0.5
    };
    let roof_mesh = roof_mesh_for_sheet(doc, eave_h);
    let segments = roof_mesh_section(&roof_mesh, cut_across_y, fixed);
    if !segments.is_empty() {
        for (a, b) in segments {
            let a = [project(a[0]), height_y(a[1])];
            let b = [project(b[0]), height_y(b[1])];
            let e0 = [a[0], height_y(eave_h)];
            let e1 = [b[0], height_y(eave_h)];
            fill_triangle(&mut rgba, SHEET_W, SHEET_H, a, b, e1, ROOF);
            fill_triangle(&mut rgba, SHEET_W, SHEET_H, a, e1, e0, ROOF);
        }
    } else if doc.roof.normalized().kind == fp_model::RoofKind::Gable {
        fill_gable_triangle(
            &mut rgba,
            left,
            left + width * scale,
            base_y,
            eave_h,
            rise,
            scale,
            ROOF,
        );
    } else {
        fill_rect(
            &mut rgba,
            SHEET_W,
            SHEET_H,
            left,
            height_y(eave_h + 0.16),
            width * scale,
            0.16 * scale,
            ROOF,
            1.0,
        );
    }
    hline(
        &mut rgba,
        SHEET_W,
        left,
        left + width * scale,
        height_y(eave_h),
        2.0,
        INK,
    );
    hline(
        &mut rgba,
        SHEET_W,
        left - 40.0,
        left + width * scale + 40.0,
        base_y,
        3.0,
        [0.38, 0.36, 0.32],
    );
    if let Some(p) = painter.as_ref() {
        let storey_h = doc.level_height_mm(0) as f32 / 1000.0;
        text_center(
            p,
            &mut rgba,
            &format!("层高 {:.2} m", storey_h),
            left + width * scale * 0.5,
            height_y(storey_h * 0.55),
            18.0,
            INK,
        );
        text_left(
            p,
            &mut rgba,
            &format!("檐口 +{:.2}", eave_h),
            left + width * scale + 28.0,
            height_y(eave_h) + 4.0,
            12.0,
            MUTED_INK,
        );
        text_left(
            p,
            &mut rgba,
            &format!("脊高 +{:.2}", eave_h + rise),
            left + width * scale + 28.0,
            height_y(eave_h + rise) + 4.0,
            12.0,
            MUTED_INK,
        );
        if doc.roof.normalized().kind == fp_model::RoofKind::Gable {
            text_left(
                p,
                &mut rgba,
                &format!("坡度 {}°", doc.roof.normalized().pitch_deg),
                left + width * scale * 0.58,
                height_y(eave_h + rise * 0.45),
                14.0,
                INK,
            );
        }
        for level in 0..levels {
            let elevation = doc.level_elevation_mm(level) as f32 / 1000.0;
            text_right(
                p,
                &mut rgba,
                &format!("{:.3}", elevation),
                left - 14.0,
                height_y(elevation) + 4.0,
                12.0,
                MUTED_INK,
            );
        }
        text_left(
            p,
            &mut rgba,
            "楼板 120 mm · 面层与找平层见节点 D-01；屋面坡度与檐口见屋面说明。",
            x + 20.0,
            y + h - 28.0,
            12.0,
            MUTED_INK,
        );
    }
    draw_sheet_title_block(&mut rgba, painter.as_ref(), doc, kind, &today);
    Ok((SHEET_W, SHEET_H, rgba))
}

fn finish_label(doc: &Document, material: &Material) -> String {
    format!("{} {}", finish_code(doc, material), material.name)
}

fn draw_color_swatch(rgba: &mut [u8], x: f32, y: f32, size: f32, color: [f32; 3]) {
    fill_rect(rgba, SHEET_W, SHEET_H, x, y, size, size, color, 1.0);
    stroke_rect(rgba, SHEET_W, SHEET_H, x, y, size, size, 1.0, RULE);
}

fn draw_finish_schedule_row(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    x: f32,
    y: f32,
    room: &Room,
    doc: &Document,
    floor_material: Option<&Material>,
    ceiling_material: Option<&Material>,
    wall_materials: &[&Material],
    floor_area: f64,
    wall_area: f64,
) -> f32 {
    let cols = [280.0f32, 500.0, 500.0, 760.0, 150.0, 180.0, 298.0];
    let h = 42.0f32;
    if let Some(p) = painter {
        let mut cx = x + 8.0;
        text_left(
            p,
            rgba,
            &format!("{}  {}", doc.level_name(room.floor as usize), room.name),
            cx,
            y + 27.0,
            15.0,
            INK,
        );
        cx += cols[0];

        for (slot, material) in [(1usize, floor_material), (2usize, ceiling_material)] {
            if let Some(m) = material {
                let color = material_color(m);
                draw_color_swatch(rgba, cx + 4.0, y + 15.0, 12.0, color);
                text_left(
                    p,
                    rgba,
                    &finish_label(doc, m),
                    cx + 23.0,
                    y + 27.0,
                    14.0,
                    INK,
                );
            } else {
                text_left(p, rgba, "—", cx + 8.0, y + 23.0, 13.0, MUTED_INK);
            }
            cx += cols[slot];
        }

        if wall_materials.is_empty() {
            text_left(p, rgba, "—", cx + 8.0, y + 23.0, 13.0, MUTED_INK);
        } else {
            let mut wx = cx + 8.0;
            for (index, m) in wall_materials.iter().take(2).enumerate() {
                if index > 0 {
                    wx += 300.0;
                }
                let color = material_color(m);
                draw_color_swatch(rgba, wx, y + 15.0, 12.0, color);
                text_left(
                    p,
                    rgba,
                    &format!("{} {}", finish_code(doc, m), m.name),
                    wx + 19.0,
                    y + 27.0,
                    13.0,
                    INK,
                );
            }
            if wall_materials.len() > 2 {
                text_left(
                    p,
                    rgba,
                    &format!("+{}", wall_materials.len() - 2),
                    cx + 630.0,
                    y + 27.0,
                    13.0,
                    MUTED_INK,
                );
            }
        }
        cx += cols[3];
        text_left(
            p,
            rgba,
            &format!("{floor_area:.1}"),
            cx + 8.0,
            y + 27.0,
            14.0,
            INK,
        );
        cx += cols[4];
        text_left(
            p,
            rgba,
            &format!("{wall_area:.1}"),
            cx + 8.0,
            y + 27.0,
            14.0,
            INK,
        );
        cx += cols[5];
        text_left(
            p,
            rgba,
            if room.counted { "计价" } else { "不计价" },
            cx + 8.0,
            y + 27.0,
            13.0,
            MUTED_INK,
        );
    }
    hline(
        rgba,
        SHEET_W,
        x,
        x + cols.iter().sum::<f32>(),
        y + h,
        0.8,
        RULE,
    );
    y + h
}

fn draw_finish_material_row(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    x: f32,
    y: f32,
    cols: &[f32],
    values: &[String],
    swatch: [f32; 3],
    swatch_col: usize,
) -> f32 {
    let h = 36.0f32;
    if let Some(p) = painter {
        let mut cx = x + 8.0;
        for (i, (col, value)) in cols.iter().zip(values).enumerate() {
            if i == swatch_col {
                draw_color_swatch(rgba, cx + 4.0, y + 11.0, 13.0, swatch);
            } else {
                text_left(p, rgba, value, cx, y + 24.0, 14.0, INK);
            }
            cx += *col;
        }
    }
    hline(
        rgba,
        SHEET_W,
        x,
        x + cols.iter().sum::<f32>(),
        y + h,
        0.8,
        RULE,
    );
    y + h
}

fn construction_finish_schedule(
    state: &SceneState,
    kind: SheetKind,
) -> Result<(u32, u32, Vec<u8>), String> {
    let mut rgba = sheet_canvas();
    let painter = TextPainter::load();
    let doc = &state.doc;
    let today = doc
        .meta
        .updated_at
        .get(0..10)
        .filter(|date| !date.is_empty())
        .map(str::to_string)
        .unwrap_or_else(current_date);
    draw_sheet_header(&mut rgba, painter.as_ref(), doc, kind);

    let x = 66.0f32;
    let width = SHEET_W as f32 - 132.0;
    let room_cols = [280.0f32, 500.0, 500.0, 760.0, 150.0, 180.0, 298.0];
    let mut y = draw_section(
        &mut rgba,
        painter.as_ref(),
        x,
        184.0,
        width,
        "空间饰面表 / ROOM FINISH SCHEDULE",
        &format!("{} 间 · 单位 ㎡", doc.rooms.len()),
    );
    y = draw_table_header(
        &mut rgba,
        painter.as_ref(),
        x,
        y,
        &room_cols,
        &[
            "空间",
            "地面饰面",
            "顶面饰面",
            "墙面饰面（本层）",
            "地面/顶面",
            "墙面估算",
            "备注",
        ],
    );
    let rooms: Vec<&Room> = doc.rooms.iter().take(14).collect();
    for room in &rooms {
        let floor_material = room_floor_material(doc, room);
        let ceiling_material = room_ceiling_material(doc, room);
        let wall_materials = floor_wall_finishes(doc, room.floor);
        let area = fp_geom::area(&room.polygon) / 1.0e6;
        y = draw_finish_schedule_row(
            &mut rgba,
            painter.as_ref(),
            x,
            y,
            room,
            doc,
            floor_material,
            ceiling_material,
            &wall_materials,
            area,
            room_wall_area_m2(doc, room),
        );
    }
    if doc.rooms.len() > rooms.len() {
        y = draw_caption_line(
            &mut rgba,
            painter.as_ref(),
            x + 2.0,
            y + 8.0,
            &format!(
                "另有 {} 间未列出，完整清单见报价 XLSX",
                doc.rooms.len() - rooms.len()
            ),
        );
    }

    y += 28.0;
    y = draw_section(
        &mut rgba,
        painter.as_ref(),
        x,
        y,
        width,
        "饰面材料表 / FINISH LIBRARY",
        &format!("{} 种", doc.materials.len()),
    );
    let material_cols = [
        100.0f32, 150.0, 560.0, 110.0, 220.0, 180.0, 150.0, 250.0, 948.0,
    ];
    y = draw_table_header(
        &mut rgba,
        painter.as_ref(),
        x,
        y,
        &material_cols,
        &[
            "编号",
            "适用面",
            "名称",
            "颜色",
            "纹理/类型",
            "单价/㎡",
            "损耗",
            "使用面积",
            "备注",
        ],
    );
    let mut materials: Vec<&Material> = doc.materials.iter().collect();
    materials.sort_by_key(|m| (finish_surface_rank(m.surface), finish_code(doc, m)));
    for material in materials.iter().take(16) {
        let usage = finish_usage_area(doc, material);
        y = draw_finish_material_row(
            &mut rgba,
            painter.as_ref(),
            x,
            y,
            &material_cols,
            &[
                finish_code(doc, material),
                finish_surface_cn(material.surface).to_string(),
                material.name.clone(),
                String::new(),
                material_kind_cn(&material.kind).to_string(),
                format!("{:.2}", material.price_per_m2),
                format!("{:.0}%", material.waste_rate * 100.0),
                format!("{usage:.1}"),
                if material.built_in {
                    "内置".to_string()
                } else {
                    "自定义".to_string()
                },
            ],
            material_color(material),
            3,
        );
    }
    if materials.len() > 16 {
        y = draw_caption_line(
            &mut rgba,
            painter.as_ref(),
            x + 2.0,
            y + 8.0,
            &format!("另有 {} 种饰面", materials.len() - 16),
        );
    }

    y += 24.0;
    y = draw_section(
        &mut rgba,
        painter.as_ref(),
        x,
        y,
        width,
        "编号与计量说明 / NOTES",
        "",
    );
    let notes = [
        "1. 编号规则：W 墙面、F 地面、C 顶面；编号按当前饰面库顺序生成，3D、报价与本图共用同一 finish_id。",
        "2. 地面与顶面面积取房间净面积；墙面面积按房间内轮廓周长×层高估算，门窗洞口调整以报价工程量为准。",
        "3. 同一楼层不同墙体的饰面可能不同，本表列出该层实际使用的饰面编号；逐墙明细见平面填充与 A-01 立面标注。",
    ];
    if let Some(p) = painter.as_ref() {
        for (index, note) in notes.iter().enumerate() {
            text_left(
                p,
                &mut rgba,
                note,
                84.0,
                y + 24.0 + index as f32 * 27.0,
                14.0,
                MUTED_INK,
            );
        }
    }

    draw_sheet_title_block(&mut rgba, painter.as_ref(), doc, kind, &today);
    Ok((SHEET_W, SHEET_H, rgba))
}

fn draw_detail_panel(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    index: &str,
    title: &str,
    subtitle: &str,
) {
    stroke_rect(rgba, SHEET_W, SHEET_H, x, y, w, h, 1.0, RULE);
    if let Some(p) = painter {
        text_left(
            p,
            rgba,
            &format!("{index}  {title}"),
            x + 18.0,
            y + 32.0,
            20.0,
            INK,
        );
        text_left(p, rgba, subtitle, x + 18.0, y + 56.0, 11.0, MUTED_INK);
        text_left(
            p,
            rgba,
            "示意 1:5 · 现场复核后放样",
            x + 18.0,
            y + h - 18.0,
            10.0,
            MUTED_INK,
        );
    }
}

fn construction_details_sheet(
    state: &SceneState,
    kind: SheetKind,
) -> Result<(u32, u32, Vec<u8>), String> {
    const CONCRETE: [f32; 3] = [0.70, 0.68, 0.64];
    const FINISH: [f32; 3] = [0.64, 0.53, 0.41];
    const FRAME: [f32; 3] = [0.45, 0.42, 0.38];
    const GLASS: [f32; 3] = [0.68, 0.79, 0.83];
    const ROOF: [f32; 3] = [0.56, 0.52, 0.47];
    let mut rgba = sheet_canvas();
    let painter = TextPainter::load();
    let doc = &state.doc;
    let today = doc
        .meta
        .updated_at
        .get(0..10)
        .filter(|date| !date.is_empty())
        .map(str::to_string)
        .unwrap_or_else(current_date);
    draw_sheet_header(&mut rgba, painter.as_ref(), doc, kind);

    let panels = [
        (84.0, 184.0),
        (1420.0, 184.0),
        (84.0, 896.0),
        (1420.0, 896.0),
    ];
    let (x, y) = panels[0];
    draw_detail_panel(
        &mut rgba,
        painter.as_ref(),
        x,
        y,
        1300.0,
        680.0,
        "01",
        "楼面与踢脚",
        "地面完成面、找平层、楼板与踢脚收口",
    );
    // Wall on the left, floor slab and finish layer.
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 120.0,
        y + 140.0,
        260.0,
        380.0,
        CONCRETE,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 380.0,
        y + 430.0,
        760.0,
        90.0,
        CONCRETE,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 380.0,
        y + 404.0,
        760.0,
        26.0,
        FINISH,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 120.0,
        y + 364.0,
        34.0,
        240.0,
        FINISH,
        1.0,
    );
    hline(
        &mut rgba,
        SHEET_W,
        x + 120.0,
        x + 1140.0,
        y + 520.0,
        2.0,
        INK,
    );
    if let Some(p) = painter.as_ref() {
        text_left(p, &mut rgba, "踢脚 80 mm", x + 180.0, y + 340.0, 12.0, INK);
        text_left(p, &mut rgba, "面层 15 mm", x + 860.0, y + 390.0, 12.0, INK);
        text_left(p, &mut rgba, "找平 30 mm", x + 860.0, y + 450.0, 12.0, INK);
        text_left(
            p,
            &mut rgba,
            "结构楼板 120 mm",
            x + 860.0,
            y + 560.0,
            12.0,
            INK,
        );
    }

    let (x, y) = panels[1];
    draw_detail_panel(
        &mut rgba,
        painter.as_ref(),
        x,
        y,
        1300.0,
        680.0,
        "02",
        "窗台收口",
        "窗框、玻璃、窗台板与滴水构造",
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 120.0,
        y + 250.0,
        1040.0,
        300.0,
        CONCRETE,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 440.0,
        y + 180.0,
        420.0,
        370.0,
        GLASS,
        1.0,
    );
    stroke_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 420.0,
        y + 160.0,
        460.0,
        410.0,
        12.0,
        FRAME,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 380.0,
        y + 540.0,
        540.0,
        40.0,
        [0.73, 0.70, 0.64],
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 380.0,
        y + 580.0,
        540.0,
        18.0,
        FRAME,
        1.0,
    );
    hline(
        &mut rgba,
        SHEET_W,
        x + 380.0,
        x + 920.0,
        y + 530.0,
        2.0,
        INK,
    );
    if let Some(p) = painter.as_ref() {
        text_left(
            p,
            &mut rgba,
            "窗台板 20 mm · 外低 5%",
            x + 940.0,
            y + 560.0,
            12.0,
            INK,
        );
        text_left(
            p,
            &mut rgba,
            "滴水槽 10×10",
            x + 940.0,
            y + 590.0,
            12.0,
            INK,
        );
        text_left(
            p,
            &mut rgba,
            "窗框与墙体间发泡胶 + 密封胶",
            x + 180.0,
            y + 650.0,
            11.0,
            MUTED_INK,
        );
    }

    let (x, y) = panels[2];
    draw_detail_panel(
        &mut rgba,
        painter.as_ref(),
        x,
        y,
        1300.0,
        680.0,
        "03",
        "门槛石",
        "门樘、门槛石与两侧地面收口",
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 100.0,
        y + 450.0,
        1100.0,
        120.0,
        CONCRETE,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 100.0,
        y + 420.0,
        1100.0,
        30.0,
        FINISH,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 460.0,
        y + 390.0,
        380.0,
        40.0,
        [0.60, 0.59, 0.56],
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 430.0,
        y + 180.0,
        30.0,
        240.0,
        FRAME,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 840.0,
        y + 180.0,
        30.0,
        240.0,
        FRAME,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 460.0,
        y + 180.0,
        380.0,
        210.0,
        [0.92, 0.90, 0.86],
        1.0,
    );
    if let Some(p) = painter.as_ref() {
        text_left(
            p,
            &mut rgba,
            "门槛石 20 mm",
            x + 860.0,
            y + 400.0,
            12.0,
            INK,
        );
        text_left(p, &mut rgba, "门樘基层", x + 860.0, y + 250.0, 12.0, INK);
        text_left(
            p,
            &mut rgba,
            "两侧地面完成面同高",
            x + 180.0,
            y + 640.0,
            11.0,
            MUTED_INK,
        );
    }

    let (x, y) = panels[3];
    draw_detail_panel(
        &mut rgba,
        painter.as_ref(),
        x,
        y,
        1300.0,
        680.0,
        "04",
        "檐口",
        "屋面坡度、檐口出挑、封檐板与排水",
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 160.0,
        y + 330.0,
        300.0,
        330.0,
        CONCRETE,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 420.0,
        y + 300.0,
        760.0,
        36.0,
        ROOF,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 420.0,
        y + 330.0,
        800.0,
        24.0,
        FRAME,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 1160.0,
        y + 300.0,
        26.0,
        120.0,
        FRAME,
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 1160.0,
        y + 410.0,
        50.0,
        50.0,
        [0.44, 0.46, 0.48],
        1.0,
    );
    fill_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        x + 420.0,
        y + 350.0,
        700.0,
        10.0,
        [0.32, 0.30, 0.27],
        1.0,
    );
    if let Some(p) = painter.as_ref() {
        text_left(
            p,
            &mut rgba,
            "屋面坡度 ≥ 5%",
            x + 820.0,
            y + 260.0,
            12.0,
            INK,
        );
        text_left(
            p,
            &mut rgba,
            "封檐板 20 mm",
            x + 900.0,
            y + 380.0,
            12.0,
            INK,
        );
        text_left(p, &mut rgba, "成品檐沟", x + 900.0, y + 440.0, 12.0, INK);
    }

    draw_sheet_title_block(&mut rgba, painter.as_ref(), doc, kind, &today);
    Ok((SHEET_W, SHEET_H, rgba))
}

fn draw_sheet_title_block(
    mut rgba: &mut [u8],
    painter: Option<&TextPainter>,
    doc: &Document,
    kind: SheetKind,
    today: &str,
) {
    // Title block.
    let total = sheet_kinds(doc).len();
    let title_y = 1718.0f32;
    let title_h = 128.0f32;
    stroke_rect(
        &mut rgba,
        SHEET_W,
        SHEET_H,
        66.0,
        title_y,
        SHEET_W as f32 - 2.0 * 66.0,
        title_h,
        2.5,
        INK,
    );
    vline(
        &mut rgba,
        SHEET_W,
        1026.0,
        title_y,
        title_y + title_h,
        1.0,
        RULE,
    );
    vline(
        &mut rgba,
        SHEET_W,
        1918.0,
        title_y,
        title_y + title_h,
        1.0,
        RULE,
    );
    if let Some(p) = &painter {
        text_left(
            p,
            &mut rgba,
            "项目 / PROJECT",
            82.0,
            title_y + 28.0,
            14.0,
            MUTED_INK,
        );
        text_left(
            p,
            &mut rgba,
            &project_title(doc),
            82.0,
            title_y + 62.0,
            29.0,
            INK,
        );
        text_left(
            p,
            &mut rgba,
            &doc.meta.address,
            82.0,
            title_y + 92.0,
            16.0,
            MUTED_INK,
        );
        let client = if doc.meta.client.is_empty() {
            "—"
        } else {
            doc.meta.client.as_str()
        };
        text_left(
            p,
            &mut rgba,
            &format!("客户 / CLIENT   {client}"),
            82.0,
            title_y + 116.0,
            14.0,
            MUTED_INK,
        );

        text_left(
            p,
            &mut rgba,
            "图名 / DRAWING",
            1046.0,
            title_y + 28.0,
            14.0,
            MUTED_INK,
        );
        text_left(
            p,
            &mut rgba,
            &kind.title(doc),
            1046.0,
            title_y + 65.0,
            28.0,
            INK,
        );
        text_left(
            p,
            &mut rgba,
            "FATHOM 方寸 · 自动生成，现场须复核",
            1046.0,
            title_y + 96.0,
            15.0,
            MUTED_INK,
        );
        text_left(
            p,
            &mut rgba,
            &format!("图号 {}    共 {total} 张    单位 mm", kind.drawing_no()),
            1046.0,
            title_y + 118.0,
            14.0,
            MUTED_INK,
        );

        text_left(
            p,
            &mut rgba,
            "签发 / ISSUE",
            1938.0,
            title_y + 28.0,
            14.0,
            MUTED_INK,
        );
        let date = if today.is_empty() { "—" } else { today };
        text_left(
            p,
            &mut rgba,
            &format!("日期   {date}"),
            1938.0,
            title_y + 60.0,
            19.0,
            INK,
        );
        text_left(
            p,
            &mut rgba,
            "比例   见图内比例尺",
            1938.0,
            title_y + 88.0,
            16.0,
            MUTED_INK,
        );
        text_left(
            p,
            &mut rgba,
            "阶段   施工图",
            1938.0,
            title_y + 112.0,
            16.0,
            MUTED_INK,
        );
    }
}

/// Local (UTC+8) calendar date for the title block, used when the plan has no
/// saved timestamp. Avoids pulling a date crate just for `YYYY-MM-DD`.
fn current_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 + 8 * 3600)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn text_left(
    painter: &TextPainter,
    rgba: &mut [u8],
    text: &str,
    x: f32,
    baseline: f32,
    size: f32,
    color: [f32; 3],
) {
    painter.draw_left(rgba, SHEET_W, SHEET_H, text, x, baseline, size, color);
}

fn text_center(
    painter: &TextPainter,
    rgba: &mut [u8],
    text: &str,
    cx: f32,
    baseline: f32,
    size: f32,
    color: [f32; 3],
) {
    let x = cx - painter.text_width(text, size) * 0.5;
    painter.draw_left(rgba, SHEET_W, SHEET_H, text, x, baseline, size, color);
}

fn text_right(
    painter: &TextPainter,
    rgba: &mut [u8],
    text: &str,
    right: f32,
    baseline: f32,
    size: f32,
    color: [f32; 3],
) {
    let x = right - painter.text_width(text, size);
    painter.draw_left(rgba, SHEET_W, SHEET_H, text, x, baseline, size, color);
}

fn hline(rgba: &mut [u8], width: u32, x0: f32, x1: f32, y: f32, thickness: f32, color: [f32; 3]) {
    fill_rect(rgba, width, SHEET_H, x0, y, x1 - x0, thickness, color, 1.0);
}

fn vline(rgba: &mut [u8], width: u32, x: f32, y0: f32, y1: f32, thickness: f32, color: [f32; 3]) {
    fill_rect(rgba, width, SHEET_H, x, y0, thickness, y1 - y0, color, 1.0);
}

#[allow(clippy::too_many_arguments)]
fn stroke_rect(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    thickness: f32,
    color: [f32; 3],
) {
    fill_rect(rgba, width, height, x, y, w, thickness, color, 1.0);
    fill_rect(
        rgba,
        width,
        height,
        x,
        y + h - thickness,
        w,
        thickness,
        color,
        1.0,
    );
    fill_rect(rgba, width, height, x, y, thickness, h, color, 1.0);
    fill_rect(
        rgba,
        width,
        height,
        x + w - thickness,
        y,
        thickness,
        h,
        color,
        1.0,
    );
}

fn blit_rgba(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    src: &[u8],
    src_w: u32,
    src_h: u32,
    ox: u32,
    oy: u32,
) {
    for row in 0..src_h {
        let dy = oy + row;
        if dy >= dst_h {
            break;
        }
        let src_start = (row * src_w * 4) as usize;
        let src_end = src_start + (src_w * 4) as usize;
        let dst_start = ((dy * dst_w + ox) * 4) as usize;
        let dst_end = dst_start + (src_w * 4) as usize;
        if dst_end <= dst.len() {
            dst[dst_start..dst_end].copy_from_slice(&src[src_start..src_end]);
        }
    }
}

fn project_title(doc: &Document) -> String {
    let raw = doc.meta.name.trim();
    if raw.is_empty() {
        return "未命名户型".to_string();
    }
    let looks_like_url = raw.starts_with("u=")
        || raw.contains("&fm=")
        || raw.contains("&app=")
        || raw.contains("http://")
        || raw.contains("https://");
    if looks_like_url {
        return "导入户型".to_string();
    }
    if raw.chars().count() > 24 {
        let mut short: String = raw.chars().take(22).collect();
        short.push('…');
        return short;
    }
    raw.to_string()
}

fn floor_stats(doc: &Document, floor: u32) -> String {
    let rooms: Vec<&fp_model::Room> = doc.rooms.iter().filter(|r| r.floor == floor).collect();
    let area: f64 = rooms
        .iter()
        .map(|r| fp_geom::area(&r.polygon) / 1.0e6)
        .sum();
    let walls = doc.walls.iter().filter(|w| w.floor == floor).count();
    let openings = doc
        .openings
        .iter()
        .filter(|o| {
            doc.walls
                .iter()
                .any(|w| w.id == o.wall_id && w.floor == floor)
        })
        .count();
    let stairs = doc.stairs.iter().filter(|s| s.floor == floor).count();
    if rooms.is_empty() && walls == 0 {
        return "本层为空 · 尚未布置墙体".to_string();
    }
    let stair_note = if stairs > 0 {
        format!(" · 楼梯 {stairs} 部")
    } else {
        String::new()
    };
    format!(
        "本层 {area:.1} ㎡ · 房间 {} 间 · 墙体 {walls} 段 · 门窗 {openings} 个{stair_note}",
        rooms.len()
    )
}

fn draw_finish_schedules(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    doc: &Document,
    floor: u32,
    x: f32,
    mut y: f32,
    w: f32,
) {
    y = draw_section(
        rgba,
        painter,
        x,
        y,
        w,
        "房间铺装表 / ROOM FINISHES",
        &format!(
            "{} 间",
            doc.rooms.iter().filter(|r| r.floor == floor).count()
        ),
    );
    let room_cols = [300.0, 70.0, 120.0, 306.0];
    y = draw_table_header(
        rgba,
        painter,
        x,
        y,
        &room_cols,
        &["房间", "楼层", "面积", "地面材料"],
    );
    let rooms: Vec<&fp_model::Room> = doc
        .rooms
        .iter()
        .filter(|r| r.floor == floor)
        .take(14)
        .collect();
    for room in &rooms {
        let material = room
            .material_id
            .and_then(|id| doc.materials.iter().find(|m| m.id == id))
            .map(|m| m.name.as_str())
            .unwrap_or("未指定");
        let area = fp_geom::area(&room.polygon) / 1.0e6;
        y = draw_table_row(
            rgba,
            painter,
            x,
            y,
            &room_cols,
            &[
                room.name.clone(),
                doc.level_name(room.floor as usize),
                format!("{area:.1} m²"),
                material.to_string(),
            ],
            INK,
        );
    }
    let floor_room_count = doc.rooms.iter().filter(|r| r.floor == floor).count();
    if floor_room_count > rooms.len() {
        y = draw_caption_line(
            rgba,
            painter,
            x + 2.0,
            y + 8.0,
            &format!("另有 {} 间未列出", floor_room_count - rooms.len()),
        );
    }

    y += 26.0;
    y = draw_section(
        rgba,
        painter,
        x,
        y,
        w,
        "材料表 / MATERIAL LEGEND",
        &format!("{} 种", doc.materials.len()),
    );
    let material_cols = [330.0, 190.0, 150.0, 126.0];
    y = draw_table_header(
        rgba,
        painter,
        x,
        y,
        &material_cols,
        &["材料", "类型", "单价/㎡", "用量(含5%)"],
    );
    for material in doc.materials.iter().take(10) {
        let area: f64 = doc
            .rooms
            .iter()
            .filter(|r| r.floor == floor && r.material_id == Some(material.id))
            .map(|r| {
                let net = fp_geom::area(&r.polygon) / 1.0e6;
                if r.counted { net * 1.05 } else { net }
            })
            .sum();
        let color = material_color(material);
        fill_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            x + 6.0,
            y + 3.0,
            12.0,
            12.0,
            color,
            1.0,
        );
        y = draw_table_row_offset(
            rgba,
            painter,
            x,
            y,
            &material_cols,
            &[
                material.name.clone(),
                material_kind_cn(&material.kind).to_string(),
                money(material.price_per_m2),
                format!("{area:.1} ㎡"),
            ],
            INK,
            20.0,
        );
    }
    if doc.materials.len() > 10 {
        y = draw_caption_line(
            rgba,
            painter,
            x + 2.0,
            y + 8.0,
            &format!("另有 {} 种材料", doc.materials.len() - 10),
        );
    }

    y += 28.0;
    y = draw_section(rgba, painter, x, y, w, "铺装说明 / NOTES", "");
    let notes = [
        "1. 本图地面标高以室内完成面为 ±0.000，单位均为毫米。",
        "2. 木地板沿长边铺设，墙边留 8–10 mm 伸缩缝。",
        "3. 瓷砖、石材留缝 2 mm，湿区找坡 1% 并做闭水试验。",
        "4. 地毯与微水泥基层须干燥、平整、无起砂，含水率 ≤8%。",
        "5. 材料用量已含 5% 损耗；施工前须现场复核尺寸。",
    ];
    if let Some(p) = painter {
        for (index, note) in notes.iter().enumerate() {
            text_left(
                p,
                rgba,
                note,
                x + 2.0,
                y + 22.0 + index as f32 * 26.0,
                15.0,
                MUTED_INK,
            );
        }
    }
}

fn draw_finish_patterns(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    cam: &Camera2D,
    doc: &Document,
    floor: u32,
) {
    for room in doc.rooms.iter().filter(|r| r.floor == floor) {
        let Some(material) = room
            .material_id
            .and_then(|id| doc.materials.iter().find(|m| m.id == id))
        else {
            continue;
        };
        let kind = material.kind.to_ascii_lowercase();
        let (step_x, step_y) =
            if kind.contains("wood") || kind.contains("oak") || kind.contains("walnut") {
                (1_200.0, 180.0)
            } else if kind.contains("tile") {
                (600.0, 600.0)
            } else if kind.contains("marble") || kind.contains("stone") {
                (900.0, 600.0)
            } else if kind.contains("terrazzo") || kind.contains("concrete") {
                (800.0, 800.0)
            } else {
                continue;
            };
        if step_x * cam.scale < 7.0 && step_y * cam.scale < 7.0 {
            continue;
        }
        let mut min = Pt::new(i32::MAX, i32::MAX);
        let mut max = Pt::new(i32::MIN, i32::MIN);
        for p in &room.polygon {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
        if min.x > max.x || min.y > max.y {
            continue;
        }
        let color = if kind.contains("tile") || kind.contains("marble") || kind.contains("stone") {
            [0.36, 0.34, 0.31]
        } else {
            [0.42, 0.33, 0.24]
        };
        let alpha = 0.14;
        if step_x * cam.scale >= 7.0 {
            let start = (min.x as f64 / step_x).ceil() as i32 * step_x as i32;
            let mut gx = start;
            while gx <= max.x {
                draw_clipped_pattern_line(
                    rgba,
                    width,
                    height,
                    cam,
                    &room.polygon,
                    Pt::new(gx, min.y),
                    Pt::new(gx, max.y),
                    color,
                    alpha,
                );
                gx += step_x as i32;
            }
        }
        if step_y * cam.scale >= 7.0 {
            let start = (min.y as f64 / step_y).ceil() as i32 * step_y as i32;
            let mut gy = start;
            while gy <= max.y {
                draw_clipped_pattern_line(
                    rgba,
                    width,
                    height,
                    cam,
                    &room.polygon,
                    Pt::new(min.x, gy),
                    Pt::new(max.x, gy),
                    color,
                    alpha,
                );
                gy += step_y as i32;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_clipped_pattern_line(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    cam: &Camera2D,
    polygon: &[Pt],
    a: Pt,
    b: Pt,
    color: [f32; 3],
    alpha: f32,
) {
    let sa = cam.world_to_screen(a);
    let sb = cam.world_to_screen(b);
    let len = ((sb[0] - sa[0]).powi(2) + (sb[1] - sa[1]).powi(2)).sqrt();
    if len < 2.0 {
        return;
    }
    let steps = (len / 2.0).ceil() as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = sa[0] + (sb[0] - sa[0]) * t;
        let y = sa[1] + (sb[1] - sa[1]) * t;
        let world = cam.screen_to_world([x, y]);
        if fp_geom::point_in_polygon(world, polygon) {
            blend_px(
                rgba,
                width,
                height,
                x.floor() as i32,
                y.floor() as i32,
                color,
                alpha,
            );
            blend_px(
                rgba,
                width,
                height,
                x.floor() as i32 + 1,
                y.floor() as i32,
                color,
                alpha,
            );
        }
    }
}

fn overlay_finish_labels(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    painter: &TextPainter,
    cam: &Camera2D,
    doc: &Document,
    floor: u32,
) {
    let base = (width as f32 * 0.015).clamp(22.0, 40.0);
    for room in doc.rooms.iter().filter(|r| r.floor == floor) {
        if room.polygon.len() < 3 {
            continue;
        }
        let area = fp_geom::area(&room.polygon) / 1_000_000.0;
        if area < 1.5 {
            continue;
        }
        let centre = polygon_centroid(&room.polygon);
        let [sx, sy] = cam.world_to_screen(centre);
        let material = room
            .material_id
            .and_then(|id| doc.materials.iter().find(|m| m.id == id))
            .map(|m| m.name.as_str())
            .unwrap_or("未指定");
        painter.draw_centered(
            rgba,
            width,
            height,
            &room.name,
            sx,
            sy - base * 0.42,
            base * 0.82,
            LABEL_DARK,
        );
        painter.draw_centered(
            rgba,
            width,
            height,
            &format!("{material} · {area:.1}㎡"),
            sx,
            sy + base * 0.62,
            base * 0.52,
            LABEL_GRAY,
        );
    }
}

fn floor_drawing_bounds(doc: &Document, floor: u32) -> Option<(Pt, Pt)> {
    if !doc.walls.iter().any(|w| w.floor == floor) && !doc.rooms.iter().any(|r| r.floor == floor) {
        return None;
    }
    let (min, max) = plan_bounds_for_floor(doc, floor);
    if max.x <= min.x && max.y <= min.y {
        return None;
    }
    Some((min, max))
}

fn draw_dimensions(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    painter: &TextPainter,
    cam: &Camera2D,
    min: Pt,
    max: Pt,
) {
    let a = cam.world_to_screen(min);
    let b = cam.world_to_screen(max);
    let left = a[0].min(b[0]);
    let right = a[0].max(b[0]);
    let top = a[1].min(b[1]);
    let bottom = a[1].max(b[1]);
    let color = [0.28, 0.27, 0.25];
    if right - left > 70.0 {
        let y = (top - 30.0).max(8.0);
        fill_rect(rgba, width, height, left, y, right - left, 1.4, color, 1.0);
        for x in [left, right] {
            fill_rect(rgba, width, height, x, y - 7.0, 1.2, 14.0, color, 1.0);
            fill_rect(
                rgba,
                width,
                height,
                x,
                y,
                1.0,
                (top - y).max(0.0),
                color,
                0.45,
            );
        }
        let label = format!("{:.0}", (max.x - min.x).max(0));
        let tw = painter.text_width(&label, 17.0);
        fill_rect(
            rgba,
            width,
            height,
            (left + right) * 0.5 - tw * 0.5 - 5.0,
            y - 24.0,
            tw + 10.0,
            20.0,
            [0.965, 0.957, 0.941],
            0.92,
        );
        painter.draw_left(
            rgba,
            width,
            height,
            &label,
            (left + right) * 0.5 - tw * 0.5,
            y - 8.0,
            17.0,
            color,
        );
    }
    if bottom - top > 70.0 {
        let x = (left - 32.0).max(8.0);
        fill_rect(rgba, width, height, x, top, 1.4, bottom - top, color, 1.0);
        for y in [top, bottom] {
            fill_rect(rgba, width, height, x - 7.0, y, 14.0, 1.2, color, 1.0);
            fill_rect(
                rgba,
                width,
                height,
                x,
                y,
                (left - x).max(0.0),
                1.0,
                color,
                0.45,
            );
        }
        let label = format!("{:.0}", (max.y - min.y).max(0));
        let tw = painter.text_width(&label, 17.0);
        fill_rect(
            rgba,
            width,
            height,
            (x - tw - 12.0).max(4.0),
            (top + bottom) * 0.5 - 10.0,
            tw + 10.0,
            20.0,
            [0.965, 0.957, 0.941],
            0.92,
        );
        painter.draw_left(
            rgba,
            width,
            height,
            &label,
            (x - tw - 8.0).max(4.0),
            (top + bottom) * 0.5 + 6.0,
            17.0,
            color,
        );
    }
}

struct OpeningEntry {
    id: EntityId,
    wall_id: EntityId,
    tag: String,
    floor: u32,
    kind: OpeningKind,
    width: i32,
    height: i32,
    sill: i32,
    offset: i32,
    price: f64,
}

fn opening_entries(doc: &Document) -> Vec<OpeningEntry> {
    let mut doors = 0;
    let mut windows = 0;
    let mut out = Vec::new();
    for opening in &doc.openings {
        let wall = doc.walls.iter().find(|w| w.id == opening.wall_id);
        let floor = wall.map(|w| w.floor).unwrap_or(0);
        let tag = if opening.kind == OpeningKind::Door {
            doors += 1;
            format!("D{doors:02}")
        } else {
            windows += 1;
            format!("W{windows:02}")
        };
        out.push(OpeningEntry {
            id: opening.id,
            wall_id: opening.wall_id,
            tag,
            floor,
            kind: opening.kind,
            width: opening.width_mm,
            height: opening.height_mm,
            sill: opening.sill_mm,
            offset: opening.offset_mm,
            price: opening.effective_price(),
        });
    }
    out.sort_by_key(|e| (e.floor, e.id.0));
    out
}

fn opening_kind_label(kind: OpeningKind) -> &'static str {
    match kind {
        OpeningKind::Door => "平开门",
        OpeningKind::Window => "平开窗",
        OpeningKind::Sliding => "推拉",
        OpeningKind::Bay => "飘窗",
    }
}

fn draw_opening_tags(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    painter: &TextPainter,
    cam: &Camera2D,
    doc: &Document,
    floor: u32,
) {
    let entries: Vec<OpeningEntry> = opening_entries(doc)
        .into_iter()
        .filter(|e| e.floor == floor)
        .collect();
    for entry in entries {
        let Some(wall) = doc.walls.iter().find(|w| w.id == entry.wall_id) else {
            continue;
        };
        let len = ((wall.end.x - wall.start.x) as f64).hypot((wall.end.y - wall.start.y) as f64);
        if len < 1.0 {
            continue;
        }
        let t = (entry.offset as f64 + entry.width as f64 * 0.5) / len;
        let center = Pt::new(
            (wall.start.x as f64 + (wall.end.x - wall.start.x) as f64 * t).round() as i32,
            (wall.start.y as f64 + (wall.end.y - wall.start.y) as f64 * t).round() as i32,
        );
        let pos = cam.world_to_screen(center);
        painter.draw_centered(
            rgba,
            width,
            height,
            &entry.tag,
            pos[0].clamp(40.0, width as f32 - 40.0),
            (pos[1] - 6.0).clamp(28.0, height as f32 - 16.0),
            21.0,
            [0.47, 0.24, 0.06],
        );
    }
}

fn draw_scale_bar(rgba: &mut [u8], x: f32, y: f32, scale: f64, painter: Option<&TextPainter>) {
    let target = 120.0f64;
    let mut metres = 0.2f64;
    let mut best = f64::MAX;
    for candidate in [0.2, 0.5, 1.0, 2.0, 5.0, 10.0] {
        let px = candidate * 1000.0 * scale;
        let distance = (px - target).abs();
        if distance < best {
            best = distance;
            metres = candidate;
        }
    }
    let px = (metres * 1000.0 * scale) as f32;
    let seg = px / 2.0;
    let h = 7.0;
    for i in 0..2 {
        let color = if i == 0 { INK } else { [0.985, 0.982, 0.972] };
        fill_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            x + i as f32 * seg,
            y,
            seg,
            h,
            color,
            1.0,
        );
        stroke_rect(
            rgba,
            SHEET_W,
            SHEET_H,
            x + i as f32 * seg,
            y,
            seg,
            h,
            1.2,
            INK,
        );
    }
    if let Some(p) = painter {
        text_left(p, rgba, "0", x - 2.0, y + 22.0, 13.0, MUTED_INK);
        text_right(
            p,
            rgba,
            &format!("{metres:.0} m"),
            x + px + 3.0,
            y + 22.0,
            13.0,
            MUTED_INK,
        );
        text_right(
            p,
            rgba,
            "比例尺 / SCALE",
            x + px + 118.0,
            y + 8.0,
            13.0,
            MUTED_INK,
        );
    }
}

fn draw_north_arrow(rgba: &mut [u8], cx: f32, cy: f32, painter: Option<&TextPainter>) {
    fill_triangle(
        rgba,
        SHEET_W,
        SHEET_H,
        [cx, cy - 22.0],
        [cx - 8.0, cy + 8.0],
        [cx + 8.0, cy + 8.0],
        INK,
    );
    fill_rect(
        rgba,
        SHEET_W,
        SHEET_H,
        cx - 1.0,
        cy + 10.0,
        2.0,
        18.0,
        INK,
        1.0,
    );
    if let Some(p) = painter {
        text_center(p, rgba, "N", cx, cy - 30.0, 15.0, INK);
    }
}

fn fill_triangle(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: [f32; 3],
) {
    let min_x = a[0].min(b[0]).min(c[0]).floor() as i32;
    let max_x = a[0].max(b[0]).max(c[0]).ceil() as i32;
    let min_y = a[1].min(b[1]).min(c[1]).floor() as i32;
    let max_y = a[1].max(b[1]).max(c[1]).ceil() as i32;
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if area.abs() < 0.01 {
        return;
    }
    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let p = [px as f32 + 0.5, py as f32 + 0.5];
            let w0 = ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) / area;
            let w1 = ((c[0] - b[0]) * (p[1] - b[1]) - (c[1] - b[1]) * (p[0] - b[0])) / area;
            let w2 = ((a[0] - c[0]) * (p[1] - c[1]) - (a[1] - c[1]) * (p[0] - c[0])) / area;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                blend_px(rgba, width, height, px, py, color, 1.0);
            }
        }
    }
}

fn material_kind_cn(kind: &str) -> &'static str {
    match kind {
        "paint" | "plaster" => "涂料",
        "wallpaper" => "壁纸",
        "wood_panel" | "wood-panel" => "木饰面",
        "wood" | "oak" | "walnut" => "木地板",
        "tile" => "瓷砖",
        "marble" => "石材",
        "terrazzo" => "水磨石",
        "carpet" => "地毯",
        "stone" | "concrete" => "水泥",
        _ => "材料",
    }
}

fn finish_surface_prefix(surface: FinishSurface) -> &'static str {
    match surface {
        FinishSurface::Wall => "W",
        FinishSurface::Floor => "F",
        FinishSurface::Ceiling => "C",
    }
}

fn finish_surface_cn(surface: FinishSurface) -> &'static str {
    match surface {
        FinishSurface::Wall => "墙面",
        FinishSurface::Floor => "地面",
        FinishSurface::Ceiling => "顶面",
    }
}

fn finish_surface_rank(surface: FinishSurface) -> u8 {
    match surface {
        FinishSurface::Wall => 0,
        FinishSurface::Floor => 1,
        FinishSurface::Ceiling => 2,
    }
}

/// Drawing code derived from the current finish library order. Stable while
/// the user edits prices or colours, and shared by A-03, plans and elevations.
fn finish_code(doc: &Document, material: &Material) -> String {
    let index = doc
        .materials
        .iter()
        .filter(|m| m.surface == material.surface)
        .position(|m| m.id == material.id)
        .map(|i| i + 1)
        .unwrap_or(0);
    format!("{}{:02}", finish_surface_prefix(material.surface), index)
}

fn legacy_wall_kind(finish: WallFinish) -> &'static str {
    match finish {
        WallFinish::Paint => "paint",
        WallFinish::Wallpaper => "wallpaper",
        WallFinish::WoodPanel => "wood_panel",
        WallFinish::Tile => "tile",
    }
}

fn wall_finish_material<'a>(doc: &'a Document, wall: &fp_model::Wall) -> Option<&'a Material> {
    wall.finish_id
        .and_then(|id| {
            doc.materials
                .iter()
                .find(|m| m.id == id && m.surface == FinishSurface::Wall)
        })
        .or_else(|| {
            let kind = legacy_wall_kind(wall.finish);
            doc.materials
                .iter()
                .find(|m| m.surface == FinishSurface::Wall && m.built_in && m.kind == kind)
        })
}

fn wall_finish_label(doc: &Document, wall: &fp_model::Wall) -> String {
    wall_finish_material(doc, wall)
        .map(|m| format!("{} {}", finish_code(doc, m), m.name))
        .unwrap_or_else(|| "—".to_string())
}

fn room_floor_material<'a>(doc: &'a Document, room: &Room) -> Option<&'a Material> {
    room.material_id
        .and_then(|id| {
            doc.materials
                .iter()
                .find(|m| m.id == id && m.surface == FinishSurface::Floor)
        })
        .or_else(|| {
            doc.materials
                .iter()
                .find(|m| m.surface == FinishSurface::Floor)
        })
}

fn room_ceiling_material<'a>(doc: &'a Document, room: &Room) -> Option<&'a Material> {
    room.ceiling_material_id
        .and_then(|id| {
            doc.materials
                .iter()
                .find(|m| m.id == id && m.surface == FinishSurface::Ceiling)
        })
        .or_else(|| {
            doc.materials
                .iter()
                .find(|m| m.surface == FinishSurface::Ceiling && m.kind == "paint")
        })
}

fn floor_wall_finishes(doc: &Document, floor: u32) -> Vec<&Material> {
    let mut out: Vec<&Material> = Vec::new();
    for wall in doc.walls.iter().filter(|w| w.floor == floor) {
        if let Some(material) = wall_finish_material(doc, wall) {
            if !out.iter().any(|m| m.id == material.id) {
                out.push(material);
            }
        }
    }
    out.sort_by_key(|m| finish_code(doc, m));
    out
}

fn room_wall_area_m2(doc: &Document, room: &Room) -> f64 {
    let perimeter_mm = fp_geom::perimeter(&room.polygon, true);
    let height_m = doc.level_height_mm(room.floor as usize) as f64 / 1000.0;
    perimeter_mm / 1000.0 * height_m
}

fn finish_usage_area(doc: &Document, material: &Material) -> f64 {
    match material.surface {
        FinishSurface::Floor => doc
            .rooms
            .iter()
            .filter(|r| room_floor_material(doc, r).map(|m| m.id) == Some(material.id))
            .map(|r| fp_geom::area(&r.polygon) / 1.0e6)
            .sum(),
        FinishSurface::Ceiling => doc
            .rooms
            .iter()
            .filter(|r| room_ceiling_material(doc, r).map(|m| m.id) == Some(material.id))
            .map(|r| fp_geom::area(&r.polygon) / 1.0e6)
            .sum(),
        FinishSurface::Wall => doc
            .walls
            .iter()
            .filter(|w| wall_finish_material(doc, w).map(|m| m.id) == Some(material.id))
            .map(|w| {
                let len =
                    ((w.end.x - w.start.x) as f64).hypot((w.end.y - w.start.y) as f64) / 1000.0;
                len * w.height_mm as f64 / 1000.0
            })
            .sum(),
    }
}

fn draw_section(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    x: f32,
    y: f32,
    w: f32,
    title: &str,
    badge: &str,
) -> f32 {
    fill_rect(rgba, SHEET_W, SHEET_H, x, y + 1.0, 4.0, 23.0, ACCENT, 1.0);
    if let Some(p) = painter {
        text_left(p, rgba, title, x + 14.0, y + 21.0, 22.0, INK);
        text_right(p, rgba, badge, x + w, y + 20.0, 15.0, MUTED_INK);
    }
    hline(rgba, SHEET_W, x, x + w, y + 34.0, 2.0, INK);
    y + 44.0
}

fn draw_table_header(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    x: f32,
    y: f32,
    cols: &[f32],
    labels: &[&str],
) -> f32 {
    let h = 30.0f32;
    fill_rect(
        rgba,
        SHEET_W,
        SHEET_H,
        x,
        y,
        cols.iter().sum(),
        h,
        INK,
        0.065,
    );
    if let Some(p) = painter {
        let mut cx = x + 8.0;
        for (col, label) in cols.iter().zip(labels) {
            text_left(p, rgba, label, cx, y + 20.0, 15.0, MUTED_INK);
            cx += *col;
        }
    }
    hline(
        rgba,
        SHEET_W,
        x,
        x + cols.iter().sum::<f32>(),
        y + h,
        1.0,
        RULE,
    );
    y + h
}

fn draw_table_row(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    x: f32,
    y: f32,
    cols: &[f32],
    values: &[String],
    color: [f32; 3],
) -> f32 {
    draw_table_row_offset(rgba, painter, x, y, cols, values, color, 0.0)
}

#[allow(clippy::too_many_arguments)]
fn draw_table_row_offset(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    x: f32,
    y: f32,
    cols: &[f32],
    values: &[String],
    color: [f32; 3],
    indent: f32,
) -> f32 {
    let h = 29.0f32;
    if let Some(p) = painter {
        let mut cx = x + 8.0;
        for (i, (col, value)) in cols.iter().zip(values).enumerate() {
            let pad = if i == 0 { indent } else { 0.0 };
            text_left(p, rgba, value, cx + pad, y + 20.0, 15.0, color);
            cx += *col;
        }
    }
    hline(
        rgba,
        SHEET_W,
        x,
        x + cols.iter().sum::<f32>(),
        y + h,
        0.8,
        RULE,
    );
    y + h
}

fn draw_caption_line(
    rgba: &mut [u8],
    painter: Option<&TextPainter>,
    x: f32,
    y: f32,
    text: &str,
) -> f32 {
    if let Some(p) = painter {
        text_left(p, rgba, text, x, y + 14.0, 14.0, MUTED_INK);
    }
    y + 24.0
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
    let landscape = width >= height;
    let (page_w, page_h) = if landscape {
        (842.0f32, 595.0f32)
    } else {
        (595.0f32, 842.0f32)
    };
    pdf_from_rgba_page(width, height, rgba, page_w, page_h)
}

fn pdf_from_rgba_page(
    width: u32,
    height: u32,
    rgba: &[u8],
    page_w: f32,
    page_h: f32,
) -> Result<Vec<u8>, String> {
    pdf_from_pages(&[(width, height, rgba.to_vec())], page_w, page_h)
}

/// Multi-page A3 PDF. Every page is a full-sheet raster, so the paper size is
/// shared and only the image placement is generated per page.
fn pdf_from_pages(
    pages: &[(u32, u32, Vec<u8>)],
    page_w: f32,
    page_h: f32,
) -> Result<Vec<u8>, String> {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    if pages.is_empty() {
        return Err("PDF 没有页面".to_string());
    }
    let margin = if page_w > 1000.0 { 18.0f32 } else { 28.0f32 };
    let mut compressed_pages = Vec::with_capacity(pages.len());
    let mut placement = Vec::with_capacity(pages.len());
    for (width, height, rgba) in pages {
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
        compressed_pages.push(encoder.finish().map_err(|e| e.to_string())?);
        let scale =
            ((page_w - margin * 2.0) / *width as f32).min((page_h - margin * 2.0) / *height as f32);
        let draw_w = *width as f32 * scale;
        let draw_h = *height as f32 * scale;
        placement.push((
            (*width, *height),
            draw_w,
            draw_h,
            (page_w - draw_w) / 2.0,
            (page_h - draw_h) / 2.0,
        ));
    }

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
    let kids = (0..pages.len())
        .map(|i| format!("{} 0 R", 3 + i * 3))
        .collect::<Vec<_>>()
        .join(" ");
    push_obj(
        &mut out,
        &mut offsets,
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).as_bytes(),
    );

    for (index, (((width, height), draw_w, draw_h, x, y), compressed)) in
        placement.iter().zip(compressed_pages.iter()).enumerate()
    {
        let page_obj = 3 + index * 3;
        let image_obj = page_obj + 1;
        let content_obj = page_obj + 2;
        let page = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_w:.0} {page_h:.0}] \
             /Resources << /XObject << /Im{index} {image_obj} 0 R >> >> \
             /Contents {content_obj} 0 R >>"
        );
        push_obj(&mut out, &mut offsets, page.as_bytes());
        let mut image = format!(
            "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
             /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            compressed.len()
        )
        .into_bytes();
        image.extend_from_slice(compressed);
        image.extend_from_slice(b"\nendstream");
        push_obj(&mut out, &mut offsets, &image);
        let content =
            format!("q\n{draw_w:.2} 0 0 {draw_h:.2} {x:.2} {y:.2} cm\n/Im{index} Do\nQ\n");
        let stream = format!(
            "<< /Length {} >>\nstream\n{}endstream",
            content.len(),
            content
        );
        push_obj(&mut out, &mut offsets, stream.as_bytes());
    }

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
        let text_w = self.text_width(text, size);
        let left = cx - text_w / 2.0;
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
        self.draw_left(
            rgba,
            width,
            height,
            text,
            left,
            cy + size * 0.36,
            size,
            color,
        );
    }

    /// Left-aligned text; `baseline` is the baseline y in pixels.
    #[allow(clippy::too_many_arguments)]
    fn draw_left(
        &self,
        rgba: &mut [u8],
        width: u32,
        height: u32,
        text: &str,
        left: f32,
        baseline: f32,
        size: f32,
        color: [f32; 3],
    ) {
        let scaled = self.font.as_scaled(PxScale::from(size));
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
#[allow(clippy::too_many_arguments)]
fn overlay_labels(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    state: &SceneState,
    cam: &Camera2D,
    floor: u32,
    show_room_names: bool,
    show_wall_lengths: bool,
) -> bool {
    let Some(painter) = TextPainter::load() else {
        return false;
    };
    let base = (width as f32 * 0.015).clamp(22.0, 40.0);

    if show_room_names {
        for room in state.doc.rooms.iter().filter(|r| r.floor == floor) {
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

    if show_wall_lengths {
        for wall in state.doc.walls.iter().filter(|w| w.floor == floor) {
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

// ---------------------------------------------------------------- whole quote

/// Customer-facing quote as CSV: rooms, furniture, openings and totals.
pub fn quote_csv(state: &SceneState) -> String {
    let doc = &state.doc;
    let quote = Quote::compute(doc);
    let mut out = String::new();
    out.push_str("户型装修报价单\n");
    out.push_str(&format!("方案,{}\n", cell(&doc.meta.name)));
    out.push_str(&format!("客户,{}\n", cell(&doc.meta.client)));
    out.push_str(&format!("地址,{}\n\n", cell(&doc.meta.address)));

    out.push_str("房间,净面积(㎡),计损耗面积(㎡),地面材质,地面单价(元/㎡),地面小计(元),家具件数,家具金额(元),房间合计(元)\n");
    for room in &quote.rooms {
        out.push_str(&format!(
            "{},{:.2},{:.2},{},{:.2},{:.2},{},{:.2},{:.2}\n",
            cell(&room.name),
            room.area_m2,
            room.billed_area_m2,
            cell(&room.material_name),
            room.material_rate,
            room.floor_cost,
            room.furniture_count,
            room.furniture_cost,
            room.total()
        ));
    }
    out.push_str(&format!(
        "地面合计,{:.2},{:.2},,,{:.2},{},{:.2},{:.2}\n\n",
        quote.floor_net_area,
        quote.floor_billed_area,
        quote.floor_material,
        quote.furniture_count,
        quote.furniture_total,
        quote.floor_material + quote.furniture_total
    ));

    out.push_str("饰面,部位,净面积(㎡),计损耗面积(㎡),单价(元/㎡),小计(元)\n");
    for row in &quote.finish_rows {
        out.push_str(&format!(
            "{},{},{:.2},{:.2},{:.2},{:.2}\n",
            cell(&row.name),
            cell(&row.surface),
            row.area_m2,
            row.billed_area_m2,
            row.unit_price,
            row.amount
        ));
    }
    out.push_str(&format!(
        "墙面合计,,{:.2},{:.2},,\n顶面合计,,{:.2},{:.2},,\n\n",
        quote.wall_net_area,
        quote.wall_billed_area,
        quote.ceiling_net_area,
        quote.ceiling_billed_area
    ));

    if quote.levels.len() > 1 {
        out.push_str(
            "分楼层小计,房间数,净面积(㎡),地面小计(元),饰面小计(元),家具金额(元),门窗金额(元),楼层小计(元)\n",
        );
        for level in &quote.levels {
            out.push_str(&format!(
                "{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}\n",
                cell(&level.name),
                level.room_count,
                level.net_area_m2,
                level.floor_cost,
                level.finish_cost,
                level.furniture_cost,
                level.opening_total,
                level.total
            ));
        }
        out.push('\n');
    }

    out.push_str("家具,数量,单价(元),小计(元)\n");
    for row in &quote.furniture {
        out.push_str(&format!(
            "{},{},{:.2},{:.2}\n",
            cell(&row.name),
            row.count,
            row.unit_price,
            row.amount
        ));
    }
    out.push_str(&format!(
        "家具合计,{},{:.2}\n\n",
        quote.furniture_count, quote.furniture_total
    ));

    out.push_str("门窗,数量,金额(元)\n");
    out.push_str(&format!(
        "门,{},{:.2}\n窗,{},{:.2}\n门窗合计,{},{:.2}\n\n",
        quote.door_count,
        quote.door_total,
        quote.window_count,
        quote.window_total,
        quote.door_count + quote.window_count,
        quote.opening_total
    ));

    out.push_str("费用汇总,金额(元)\n");
    out.push_str(&format!(
        "地面材料,{:.2}\n墙面饰面,{:.2}\n顶面饰面,{:.2}\n家具,{:.2}\n门窗,{:.2}\n人工费,{:.2}\n辅材费,{:.2}\n小计,{:.2}\n折扣,{:.2}\n税金,{:.2}\n总计,{:.2}\n",
        quote.floor_material,
        quote.wall_material,
        quote.ceiling_material,
        quote.furniture_total,
        quote.opening_total,
        quote.labor,
        quote.auxiliary,
        quote.subtotal,
        quote.discount,
        quote.tax,
        quote.total
    ));
    out
}

pub fn export_quote_csv(state: &SceneState, path: &Path) -> Result<String, String> {
    let csv = quote_csv(state);
    let mut text = String::with_capacity(csv.len() + 1);
    text.push('\u{feff}');
    text.push_str(&csv);
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok("已导出全屋报价 CSV".to_string())
}

enum XCell {
    Text(String),
    Number(f64),
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn xlsx_sheet(rows: &[Vec<XCell>]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
         <sheetData>",
    );
    for (r, row) in rows.iter().enumerate() {
        out.push_str(&format!("<row r=\"{}\">", r + 1));
        for (c, cell) in row.iter().enumerate() {
            let col = (b'A' + (c as u8 % 26)) as char;
            let reference = format!("{col}{}", r + 1);
            match cell {
                XCell::Text(text) => out.push_str(&format!(
                    "<c r=\"{reference}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
                    xml_escape(text)
                )),
                XCell::Number(value) => {
                    out.push_str(&format!("<c r=\"{reference}\"><v>{value:.2}</v></c>"))
                }
            }
        }
        out.push_str("</row>");
    }
    out.push_str("</sheetData></worksheet>");
    out
}

/// Minimal XLSX (one worksheet, inline strings) so Windows Excel and Numbers
/// both open the quote without a CSV import step.
pub fn export_quote_xlsx(state: &SceneState, path: &Path) -> Result<String, String> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let doc = &state.doc;
    let quote = Quote::compute(doc);
    let mut rows: Vec<Vec<XCell>> = Vec::new();
    let text = |value: &str| XCell::Text(value.to_string());
    let num = XCell::Number;

    rows.push(vec![text("户型装修报价单")]);
    rows.push(vec![text("方案"), text(&doc.meta.name)]);
    rows.push(vec![text("客户"), text(&doc.meta.client)]);
    rows.push(vec![text("地址"), text(&doc.meta.address)]);
    rows.push(Vec::new());
    rows.push(vec![
        text("房间"),
        text("净面积(㎡)"),
        text("计损耗面积(㎡)"),
        text("地面材质"),
        text("地面单价(元/㎡)"),
        text("地面小计(元)"),
        text("家具件数"),
        text("家具金额(元)"),
        text("房间合计(元)"),
    ]);
    for room in &quote.rooms {
        rows.push(vec![
            text(&room.name),
            num(room.area_m2),
            num(room.billed_area_m2),
            text(&room.material_name),
            num(room.material_rate),
            num(room.floor_cost),
            num(room.furniture_count as f64),
            num(room.furniture_cost),
            num(room.total()),
        ]);
    }
    if quote.levels.len() > 1 {
        rows.push(Vec::new());
        rows.push(vec![
            text("分楼层小计"),
            text("房间数"),
            text("净面积(㎡)"),
            text("地面小计(元)"),
            text("饰面小计(元)"),
            text("家具金额(元)"),
            text("门窗金额(元)"),
            text("楼层小计(元)"),
        ]);
        for level in &quote.levels {
            rows.push(vec![
                text(&level.name),
                num(level.room_count as f64),
                num(level.net_area_m2),
                num(level.floor_cost),
                num(level.finish_cost),
                num(level.furniture_cost),
                num(level.opening_total),
                num(level.total),
            ]);
        }
    }
    rows.push(vec![
        text("地面合计"),
        num(quote.floor_net_area),
        num(quote.floor_billed_area),
        text(""),
        num(0.0),
        num(quote.floor_material),
        num(quote.furniture_count as f64),
        num(quote.furniture_total),
        num(quote.floor_material + quote.furniture_total),
    ]);
    rows.push(Vec::new());
    rows.push(vec![
        text("饰面"),
        text("部位"),
        text("净面积(㎡)"),
        text("计损耗面积(㎡)"),
        text("单价(元/㎡)"),
        text("小计(元)"),
    ]);
    for row in &quote.finish_rows {
        rows.push(vec![
            text(&row.name),
            text(&row.surface),
            num(row.area_m2),
            num(row.billed_area_m2),
            num(row.unit_price),
            num(row.amount),
        ]);
    }
    rows.push(vec![
        text("墙面合计"),
        text("墙面"),
        num(quote.wall_net_area),
        num(quote.wall_billed_area),
        text(""),
        num(quote.wall_material),
    ]);
    rows.push(vec![
        text("顶面合计"),
        text("顶面"),
        num(quote.ceiling_net_area),
        num(quote.ceiling_billed_area),
        text(""),
        num(quote.ceiling_material),
    ]);
    rows.push(Vec::new());
    rows.push(vec![
        text("家具"),
        text("数量"),
        text("单价(元)"),
        text("小计(元)"),
    ]);
    for row in &quote.furniture {
        rows.push(vec![
            text(&row.name),
            num(row.count as f64),
            num(row.unit_price),
            num(row.amount),
        ]);
    }
    rows.push(vec![
        text("家具合计"),
        num(quote.furniture_count as f64),
        text(""),
        num(quote.furniture_total),
    ]);
    rows.push(Vec::new());
    rows.push(vec![text("门窗"), text("数量"), text("金额(元)")]);
    rows.push(vec![
        text("门"),
        num(quote.door_count as f64),
        num(quote.door_total),
    ]);
    rows.push(vec![
        text("窗"),
        num(quote.window_count as f64),
        num(quote.window_total),
    ]);
    rows.push(vec![
        text("门窗合计"),
        num((quote.door_count + quote.window_count) as f64),
        num(quote.opening_total),
    ]);
    rows.push(Vec::new());
    rows.push(vec![text("费用汇总"), text("金额(元)")]);
    for (label, value) in [
        ("地面材料", quote.floor_material),
        ("墙面饰面", quote.wall_material),
        ("顶面饰面", quote.ceiling_material),
        ("家具", quote.furniture_total),
        ("门窗", quote.opening_total),
        ("人工费", quote.labor),
        ("辅材费", quote.auxiliary),
        ("小计", quote.subtotal),
        ("折扣", -quote.discount),
        ("税金", quote.tax),
        ("总计", quote.total),
    ] {
        rows.push(vec![text(label), num(value)]);
    }
    let sheet = xlsx_sheet(&rows);

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let write =
        |zip: &mut zip::ZipWriter<std::fs::File>, name: &str, body: &str| -> Result<(), String> {
            zip.start_file(name, options).map_err(|e| e.to_string())?;
            zip.write_all(body.as_bytes()).map_err(|e| e.to_string())
        };

    write(
        &mut zip,
        "[Content_Types].xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
         <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
         </Types>",
    )?;
    write(
        &mut zip,
        "_rels/.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
         </Relationships>",
    )?;
    write(
        &mut zip,
        "xl/workbook.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets><sheet name=\"报价\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>",
    )?;
    write(
        &mut zip,
        "xl/_rels/workbook.xml.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
         </Relationships>",
    )?;
    write(&mut zip, "xl/worksheets/sheet1.xml", &sheet)?;
    zip.finish().map_err(|e| e.to_string())?;
    Ok("已导出全屋报价 XLSX".to_string())
}

/// A4 portrait quote rendered on the CPU (same CJK font path as plan labels)
/// and then embedded as a single PDF image. Using a raster keeps Chinese text
/// correct without shipping a font subsetter.
pub fn export_quote_pdf(state: &SceneState, path: &Path) -> Result<String, String> {
    let Some(painter) = TextPainter::load() else {
        return Err("缺少中文字体，无法生成报价 PDF".into());
    };
    let doc = &state.doc;
    let quote = Quote::compute(doc);
    let width = 1240u32;
    let height = 1754u32;
    let mut rgba = vec![255u8; (width * height * 4) as usize];
    // Warm paper tint.
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[0] = 253;
        pixel[1] = 252;
        pixel[2] = 249;
        pixel[3] = 255;
    }
    let ink = [0.12, 0.14, 0.19];
    let muted = [0.38, 0.40, 0.46];
    let accent = [0.18, 0.44, 0.93];
    let margin = 92.0f32;
    let mut y = 120.0f32;
    let line = |rgba: &mut [u8], y: f32, color: [f32; 3], alpha: f32| {
        fill_rect(
            rgba,
            width,
            height,
            margin,
            y,
            width as f32 - margin * 2.0,
            1.2,
            color,
            alpha,
        );
    };
    let text_at =
        |rgba: &mut [u8], x: f32, baseline: f32, value: &str, size: f32, color: [f32; 3]| {
            painter.draw_left(rgba, width, height, value, x, baseline, size, color);
        };

    fill_rect(
        &mut rgba,
        width,
        height,
        0.0,
        0.0,
        width as f32,
        10.0,
        accent,
        1.0,
    );
    text_at(
        &mut rgba,
        margin,
        y,
        "Fathom 方寸 · 全屋装修报价单",
        34.0,
        ink,
    );
    y += 38.0;
    let mut subtitle = if doc.meta.name.is_empty() {
        "未命名方案".to_string()
    } else {
        doc.meta.name.clone()
    };
    if !doc.meta.client.is_empty() {
        subtitle.push_str("  ·  ");
        subtitle.push_str(&doc.meta.client);
    }
    if !doc.meta.address.is_empty() {
        subtitle.push_str("  ·  ");
        subtitle.push_str(&doc.meta.address);
    }
    text_at(&mut rgba, margin, y, &subtitle, 18.0, muted);
    y += 34.0;
    line(&mut rgba, y, accent, 0.7);
    y += 46.0;

    let col = [margin, 360.0, 610.0, 850.0, 1080.0];
    text_at(&mut rgba, col[0], y, "房间", 18.0, muted);
    text_at(&mut rgba, col[1], y, "面积", 18.0, muted);
    text_at(&mut rgba, col[2], y, "地面", 18.0, muted);
    text_at(&mut rgba, col[3], y, "家具", 18.0, muted);
    text_at(&mut rgba, col[4], y, "小计", 18.0, muted);
    y += 16.0;
    line(&mut rgba, y, muted, 0.35);
    y += 30.0;
    for (index, room) in quote.rooms.iter().take(10).enumerate() {
        if index > 0 {
            line(&mut rgba, y - 20.0, muted, 0.12);
        }
        let name: String = room.name.chars().take(10).collect();
        text_at(&mut rgba, col[0], y, &name, 18.0, ink);
        text_at(
            &mut rgba,
            col[1],
            y,
            &format!("{:.1} m²", room.area_m2),
            17.0,
            ink,
        );
        text_at(
            &mut rgba,
            col[2],
            y,
            &format!("{} {}", room.material_name, money(room.floor_cost)),
            16.0,
            ink,
        );
        text_at(
            &mut rgba,
            col[3],
            y,
            &format!("{} 件 {}", room.furniture_count, money(room.furniture_cost)),
            16.0,
            ink,
        );
        text_at(&mut rgba, col[4], y, &money(room.total()), 17.0, ink);
        y += 32.0;
    }
    if quote.rooms.len() > 10 {
        text_at(
            &mut rgba,
            margin,
            y,
            &format!("另有 {} 个房间，明细见 XLSX", quote.rooms.len() - 10),
            14.0,
            muted,
        );
        y += 28.0;
    }
    line(&mut rgba, y, muted, 0.35);
    y += 32.0;

    text_at(&mut rgba, margin, y, "饰面明细", 22.0, ink);
    y += 26.0;
    text_at(&mut rgba, margin, y, "部位", 16.0, muted);
    text_at(&mut rgba, 250.0, y, "名称", 16.0, muted);
    text_at(&mut rgba, 610.0, y, "净面积", 16.0, muted);
    text_at(&mut rgba, 790.0, y, "计损耗", 16.0, muted);
    text_at(&mut rgba, 960.0, y, "小计", 16.0, muted);
    y += 24.0;
    for row in quote.finish_rows.iter().take(9) {
        text_at(&mut rgba, margin, y, &row.surface, 16.0, muted);
        let name: String = row.name.chars().take(14).collect();
        text_at(&mut rgba, 250.0, y, &name, 16.0, ink);
        text_at(
            &mut rgba,
            610.0,
            y,
            &format!("{:.1} m²", row.area_m2),
            16.0,
            muted,
        );
        text_at(
            &mut rgba,
            790.0,
            y,
            &format!("{:.1} m²", row.billed_area_m2),
            16.0,
            muted,
        );
        text_at(&mut rgba, 960.0, y, &money(row.amount), 16.0, ink);
        y += 24.0;
    }
    y += 18.0;

    text_at(&mut rgba, margin, y, "费用汇总", 24.0, ink);
    y += 30.0;
    let rows: Vec<(String, f64)> = vec![
        ("地面材料（含损耗）".to_string(), quote.floor_material),
        ("墙面饰面（扣门窗洞口）".to_string(), quote.wall_material),
        ("顶面饰面".to_string(), quote.ceiling_material),
        ("家具".to_string(), quote.furniture_total),
        ("门 / 窗".to_string(), quote.opening_total),
        (
            format!(
                "人工费（{:.1} m² × ¥{:.0}）",
                quote.labor_area, doc.quote.labor_per_m2
            ),
            quote.labor,
        ),
        (
            format!(
                "辅材费（{:.1} m² × ¥{:.0}）",
                quote.labor_area, doc.quote.auxiliary_per_m2
            ),
            quote.auxiliary,
        ),
        ("小计".to_string(), quote.subtotal),
        (
            format!("折扣（{:.0}%）", doc.quote.discount_rate * 100.0),
            -quote.discount,
        ),
        (
            format!("税金（{:.0}%）", doc.quote.tax_rate * 100.0),
            quote.tax,
        ),
    ];
    for (label, value) in rows {
        text_at(&mut rgba, margin, y, &label, 18.0, muted);
        text_at(&mut rgba, 860.0, y, &money(value), 18.0, ink);
        y += 30.0;
    }
    y += 12.0;
    fill_rect(
        &mut rgba,
        width,
        height,
        margin,
        y - 20.0,
        width as f32 - margin * 2.0,
        64.0,
        accent,
        0.10,
    );
    text_at(&mut rgba, margin + 18.0, y + 20.0, "总计", 26.0, ink);
    text_at(
        &mut rgba,
        790.0,
        y + 22.0,
        &money(quote.total),
        30.0,
        accent,
    );
    y += 72.0;
    line(&mut rgba, y, muted, 0.2);
    y += 26.0;
    text_at(
        &mut rgba,
        margin,
        y,
        "报价含 5% 地面材料损耗；人工与辅材按建筑面积估算，最终以现场复尺为准。",
        14.0,
        muted,
    );
    y += 24.0;
    text_at(
        &mut rgba,
        margin,
        y,
        &format!("Fathom 方寸 · 生成于 {}", doc.meta.updated_at),
        13.0,
        muted,
    );

    let pdf = pdf_from_rgba(width, height, &rgba)?;
    std::fs::write(path, pdf).map_err(|e| format!("PDF 写入失败: {e}"))?;
    Ok(format!("已导出报价 PDF · A4 · 总计 {}", money(quote.total)))
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
    let svg_walls = SceneState::split_walls_at_openings(&doc.walls, &doc.openings);
    for wall in &svg_walls {
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
    use fp_core::{EntityId, Pt};
    use fp_model::{Document, Opening, OpeningKind, Room, Wall, WallFinish, WallKind};
    use std::path::PathBuf;

    #[test]
    fn roof_section_uses_the_real_mesh_profile() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/samples/demo-apartment.json");
        let text = std::fs::read_to_string(&path).expect("fixture");
        let doc = Document::from_json(&text).expect("parse");
        let levels = doc.level_count().max(1);
        let eave_h =
            (doc.level_elevation_mm(levels - 1) + doc.level_height_mm(levels - 1)) as f32 / 1000.0;
        let mesh = roof_mesh_for_sheet(&doc, eave_h);
        assert!(
            !mesh.is_empty(),
            "roof mesh should be generated for the sheet"
        );
        let (x0, x1, _, _, _) = building_extents(&doc);
        let segments = roof_mesh_section(&mesh, true, (x0 + x1) * 0.5);
        assert!(!segments.is_empty(), "section plane should cut the roof");
        let peak = segments
            .iter()
            .flat_map(|(a, b)| [a[1], b[1]])
            .fold(f32::MIN, f32::max);
        assert!(
            peak > eave_h + 0.2,
            "roof profile peak {peak} above eave {eave_h}"
        );
    }

    #[test]
    #[ignore = "needs a GPU; run with --ignored"]
    fn export_door_gap_probe() {
        let mut doc = Document::new("door-probe");
        doc.walls.push(Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            200,
            WallKind::Partition,
        ));
        doc.openings.push(Opening {
            id: EntityId(2),
            wall_id: EntityId(1),
            kind: OpeningKind::Door,
            offset_mm: 500,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 90.0,
            hinge_right: false,
            price: None,
        });
        let state = SceneState::from_doc(doc, [900.0, 700.0], PathBuf::from("/tmp/door.json"));
        export_png(&state, Path::new("/tmp/fp-door-probe.png")).expect("door probe");
    }

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
            color: String::new(),
            surface: fp_model::FinishSurface::Floor,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
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
            floor: 0,
            ceiling_material_id: None,
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
    fn quote_xlsx_contains_rooms_furniture_and_totals() {
        use fp_model::{Furniture, Material, Room};
        let mut doc = Document::new("报价测试");
        let mat = EntityId(910);
        doc.materials.push(Material {
            id: mat,
            name: "橡木地板".into(),
            price_per_m2: 320.0,
            kind: "wood".into(),
            color: String::new(),
            surface: fp_model::FinishSurface::Floor,
            waste_rate: fp_model::default_waste_rate(),
            built_in: false,
        });
        doc.rooms.push(Room {
            id: EntityId(911),
            polygon: vec![
                Pt::new(0, 0),
                Pt::new(4000, 0),
                Pt::new(4000, 3000),
                Pt::new(0, 3000),
            ],
            name: "客厅".into(),
            material_id: Some(mat),
            counted: true,
            floor: 0,
            ceiling_material_id: None,
        });
        doc.furniture.push(Furniture {
            id: EntityId(912),
            asset_id: "sofa".into(),
            name: "三人沙发".into(),
            center: Pt::new(2000, 1500),
            width_mm: 2200,
            depth_mm: 950,
            rotation_deg: 0.0,
            color: "#8B6F55".into(),
            room_id: Some(EntityId(911)),
            price: Some(4999.0),
            floor: 0,
        });
        let state = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/q.json"));
        let path = PathBuf::from("/tmp/fp-quote-test.xlsx");
        export_quote_xlsx(&state, &path).expect("xlsx");
        let file = std::fs::File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut sheet = String::new();
        use std::io::Read;
        archive
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet)
            .unwrap();
        assert!(sheet.contains("户型装修报价单"), "{sheet}");
        assert!(sheet.contains("客厅"), "{sheet}");
        assert!(sheet.contains("三人沙发"), "{sheet}");
        assert!(sheet.contains("4999.00"), "{sheet}");
        assert!(sheet.contains("总计"), "{sheet}");
        assert!(sheet.contains("饰面"), "{sheet}");
        let csv = quote_csv(&state);
        assert!(csv.contains("费用汇总"), "{csv}");
        assert!(csv.contains("墙面饰面"), "{csv}");
        assert!(csv.contains("顶面饰面"), "{csv}");
        assert!(csv.contains("4032.00"), "floor cost: {csv}");
    }

    #[test]
    fn quote_pdf_renders_a4_with_totals() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo json");
        let state = SceneState::from_doc(doc, [1000.0, 800.0], PathBuf::from("/tmp/q.json"));
        let path = PathBuf::from("/tmp/fp-quote-test.pdf");
        export_quote_pdf(&state, &path).expect("pdf");
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"), "pdf header");
        assert!(bytes.len() > 100_000, "pdf too small: {}", bytes.len());
    }

    #[test]
    #[ignore = "needs a GPU; writes preview files"]
    fn export_cutaway_probe() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo json");
        let mut state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/cut.json"));
        state.sun = 0.55;
        state.cutaway = true;
        state.preset3d(crate::scene::Preset3d::Oblique);
        let mesh = state.mesh3d();
        let vp = state.cam3.view_proj(1920.0 / 1200.0);
        render_3d_frame(
            &mesh,
            Path::new("/tmp/fp-cutaway-probe.png"),
            1920,
            1200,
            vp,
            state.cam3.eye().to_array(),
            state.look3d(),
        )
        .expect("cutaway probe");
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
    fn project_title_cleans_imported_url_names() {
        let mut doc = Document::new("u=2412825671,2944070284&fm=253&app=138&f=JPEG 户型");
        assert_eq!(project_title(&doc), "导入户型");
        doc.meta.name = "望江府 138㎡".to_string();
        assert_eq!(project_title(&doc), "望江府 138㎡");
        doc.meta.name = "ABCDEFGHIJKLMNOPQRSTUVWXYZ1234".to_string();
        assert_eq!(project_title(&doc).chars().count(), 23);
    }

    #[test]
    fn title_block_date_is_iso_formatted() {
        let date = current_date();
        assert_eq!(date.len(), 10, "{date}");
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
        assert!(date.starts_with("20"), "{date}");
    }

    #[test]
    fn a3_pdf_uses_the_large_page_box() {
        let rgba = [255u8, 255, 255, 255];
        let pdf = pdf_from_rgba_page(1, 1, &rgba, 1191.0, 842.0).expect("pdf");
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/MediaBox [0 0 1191 842]"), "{text}");
    }

    #[test]
    fn pdf_pages_share_the_a3_box_and_get_distinct_images() {
        let one = [255u8, 255, 255, 255];
        let two = [0u8, 0, 0, 255];
        let pages = [(1, 1, one.to_vec()), (1, 1, two.to_vec())];
        let pdf = pdf_from_pages(&pages, 1191.0, 842.0).expect("pdf");
        let text = String::from_utf8_lossy(&pdf);
        assert_eq!(text.matches("/MediaBox [0 0 1191 842]").count(), 2);
        assert!(text.contains("/Count 2"), "{text}");
        assert!(text.contains("/Im0"), "{text}");
        assert!(text.contains("/Im1"), "{text}");
    }

    #[test]
    fn openings_get_door_and_window_tags_per_kind() {
        let mut doc = Document::new("tags");
        let mut wall_a = Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            120,
            WallKind::Partition,
        );
        wall_a.floor = 1;
        let mut wall_b = Wall::new(
            EntityId(2),
            Pt::new(0, 3000),
            Pt::new(4000, 3000),
            120,
            WallKind::Partition,
        );
        wall_b.floor = 1;
        doc.walls.push(wall_a);
        doc.walls.push(wall_b);
        doc.openings.push(fp_model::Opening {
            id: EntityId(11),
            wall_id: EntityId(1),
            kind: OpeningKind::Door,
            offset_mm: 600,
            width_mm: 900,
            height_mm: 2100,
            sill_mm: 0,
            swing_deg: 90.0,
            hinge_right: false,
            price: None,
        });
        doc.openings.push(fp_model::Opening {
            id: EntityId(12),
            wall_id: EntityId(2),
            kind: OpeningKind::Window,
            offset_mm: 1200,
            width_mm: 1200,
            height_mm: 1400,
            sill_mm: 900,
            swing_deg: 0.0,
            hinge_right: false,
            price: None,
        });
        let entries = opening_entries(&doc);
        assert_eq!(entries[0].tag, "D01");
        assert_eq!(entries[0].floor, 1);
        assert_eq!(entries[1].tag, "W01");
        assert_eq!(entries[1].floor, 1);
        assert_eq!(entries[1].offset, 1200);
    }

    #[test]
    fn a03_room_row_lists_each_wall_finish_on_the_floor() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let mut state = SceneState::from_doc(
            doc,
            [1280.0, 720.0],
            PathBuf::from("/tmp/fp-finish-codes.json"),
        );
        let kinds = ["wallpaper", "wood_panel", "tile"];
        let wall_ids: Vec<EntityId> = state
            .doc
            .walls
            .iter()
            .filter(|w| w.floor == 0)
            .map(|w| w.id)
            .take(3)
            .collect();
        let mut expected = Vec::new();
        for (id, kind) in wall_ids.iter().zip(kinds) {
            let material = state
                .doc
                .materials
                .iter()
                .find(|m| m.surface == FinishSurface::Wall && m.kind == kind)
                .expect("built-in wall finish")
                .id;
            expected.push(material);
            if let Some(wall) = state.doc.walls.iter_mut().find(|w| w.id == *id) {
                wall.finish_id = Some(material);
            }
        }
        let labels: Vec<String> = expected
            .iter()
            .filter_map(|id| {
                let wall = state.doc.walls.iter().find(|w| w.finish_id == Some(*id))?;
                Some(wall_finish_label(&state.doc, wall))
            })
            .collect();
        assert_eq!(labels.len(), 3);
        assert!(labels.iter().all(|label| label.starts_with('W')));
        let finishes = floor_wall_finishes(&state.doc, 0);
        assert!(finishes.len() >= 3);
        assert!(
            expected
                .iter()
                .all(|id| finishes.iter().any(|m| m.id == *id))
        );
    }

    #[test]
    fn sheet_kinds_include_the_a03_finish_schedule() {
        let doc = Document::new("pages");
        let floors = doc.level_count().max(1);
        let kinds = sheet_kinds(&doc);
        assert_eq!(kinds.len(), 1 + 2 * floors + 4);
        assert!(kinds.contains(&SheetKind::FinishSchedule));
        assert!(sheet_index(&doc).iter().any(|(no, _, _)| no == "A-03"));
    }

    #[test]
    fn an_empty_upper_floor_gets_no_overall_dimensions() {
        let mut doc = Document::new("empty");
        doc.walls.push(Wall::new(
            EntityId(1),
            Pt::new(0, 0),
            Pt::new(4000, 0),
            120,
            WallKind::Partition,
        ));
        assert!(floor_drawing_bounds(&doc, 0).is_some());
        assert!(floor_drawing_bounds(&doc, 1).is_none());
    }

    #[test]
    #[ignore = "needs a GPU and FATHOM_PLAN; run with --ignored"]
    fn construction_sheet_from_real_plan() {
        let Ok(plan) = std::env::var("FATHOM_PLAN") else {
            return;
        };
        let text = std::fs::read_to_string(&plan).expect("read plan");
        let doc = Document::from_json(&text).expect("parse plan");
        let floors = doc.levels.len().max(1);
        let pages = 1 + 2 * floors + 4;
        let state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from(&plan));
        let path = PathBuf::from("/tmp/fp-construction-real.pdf");
        let msg = export_construction_pdf(&state, &path).expect("construction pdf");
        assert!(msg.contains("A3"), "{msg}");
        assert!(msg.contains(&format!("{pages} 页")), "{msg}");
    }

    #[test]
    #[ignore = "needs a GPU; run with --ignored"]
    fn construction_sheet_stacks_two_floors() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let mut doc = Document::from_json(json).expect("demo json");
        doc.ensure_levels();
        doc.levels.push(fp_model::Level {
            name: "2F".to_string(),
            height_mm: 2800,
        });
        let walls: Vec<Wall> = doc.walls.iter().filter(|w| w.floor == 0).cloned().collect();
        for mut wall in walls {
            wall.id = EntityId(doc.next_id());
            wall.floor = 1;
            doc.walls.push(wall);
        }
        let rooms: Vec<fp_model::Room> =
            doc.rooms.iter().filter(|r| r.floor == 0).cloned().collect();
        for mut room in rooms {
            room.id = EntityId(doc.next_id());
            room.floor = 1;
            doc.rooms.push(room);
        }
        let state = SceneState::from_doc(
            doc,
            [1280.0, 720.0],
            PathBuf::from("/tmp/fp-construction-2f.json"),
        );
        let path = PathBuf::from("/tmp/fp-construction-2f.pdf");
        let msg = export_construction_pdf(&state, &path).expect("2f construction pdf");
        assert!(msg.contains("2 层"), "{msg}");
        assert!(msg.contains("9 页"), "{msg}");
        let bytes = std::fs::read(&path).expect("pdf exists");
        assert!(bytes.len() > 200_000, "pdf is {} bytes", bytes.len());
    }

    #[test]
    #[ignore = "needs a GPU; finish schedule probe"]
    fn finish_schedule_sheet_probe() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo json");
        let state = SceneState::from_doc(
            doc,
            [1280.0, 720.0],
            PathBuf::from("/tmp/fp-finish-schedule.json"),
        );
        let (w, h, rgba) =
            construction_sheet(&state, SheetKind::FinishSchedule).expect("finish schedule");
        write_png(Path::new("/tmp/fp-finish-schedule.png"), w, h, &rgba)
            .expect("write finish schedule");
    }

    #[test]
    #[ignore = "needs a GPU; elevation probe"]
    fn elevation_probe_south() {
        use glam::{Mat4, Vec3};
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let mut state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/elev.json"));
        state.show_roof = true;
        let mesh = state.mesh3d();
        let (min, max) = plan_bounds(&state.doc);
        let levels = state.doc.level_count();
        let h = (state.doc.level_elevation_mm(levels - 1) + state.doc.level_height_mm(levels - 1))
            as f32
            / 1000.0;
        let cx = (min.x + max.x) as f32 / 2000.0;
        let cz = (min.y + max.y) as f32 / 2000.0;
        let width = (max.x - min.x) as f32 / 1000.0 * 1.12;
        let height = h * 1.25;
        let half_w = width * 0.5;
        let half_h = height * 0.5;
        let eye = Vec3::new(cx, height * 0.5, cz + 20.0);
        let target = Vec3::new(cx, height * 0.5, cz);
        let view = Mat4::look_at_rh(eye, target, Vec3::Y);
        let proj = Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, 0.05, 100.0);
        let mut look = state.look3d();
        look.shadows = 0.0;
        look.clear = [0.965, 0.957, 0.941];
        let path = Path::new("/tmp/elev-probe.png");
        render_3d_frame(&mesh, path, 1200, 800, proj * view, eye.to_array(), look).expect("render");
    }

    #[test]
    #[ignore = "needs a GPU; pitched-roof perspective probe"]
    fn roof_probe_perspective() {
        use glam::Vec3;
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let mut state = SceneState::from_doc(doc, [1280.0, 800.0], PathBuf::from("/tmp/roof.json"));
        state.show_roof = true;
        state.cutaway = false;
        let (min, max) = plan_bounds(&state.doc);
        let levels = state.doc.level_count();
        let top = (state.doc.level_elevation_mm(levels - 1) + state.doc.level_height_mm(levels - 1))
            as f32
            / 1000.0;
        let cx = (min.x + max.x) as f32 / 2000.0;
        let cz = (min.y + max.y) as f32 / 2000.0;
        let diag =
            (((max.x - min.x) as f32).powi(2) + ((max.y - min.y) as f32).powi(2) + top.powi(2))
                .sqrt()
                / 1000.0;
        state.cam3.target = Vec3::new(cx, top * 0.55, cz);
        state.cam3.distance = (diag * 1.45).clamp(6.0, 40.0);
        state.cam3.pitch = 0.32;
        state.cam3.yaw = 0.08;
        let mesh = state.mesh3d();
        let look = state.look3d();
        let path = Path::new("/tmp/roof-probe.png");
        render_3d_frame(
            &mesh,
            path,
            1200,
            800,
            state.cam3.view_proj(1.5),
            state.cam3.eye().to_array(),
            look,
        )
        .expect("render roof");
    }

    #[test]
    #[ignore = "needs a GPU; L-shaped roof probe"]
    fn roof_probe_l_shape_perspective() {
        use glam::Vec3;
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let mut doc = Document::from_json(json).expect("demo");
        doc.rooms.clear();
        doc.walls.clear();
        doc.openings.clear();
        doc.furniture.clear();
        doc.stairs.clear();
        doc.rooms.push(Room {
            id: EntityId(9001),
            polygon: vec![
                Pt::new(0, 0),
                Pt::new(6000, 0),
                Pt::new(6000, 4000),
                Pt::new(0, 4000),
            ],
            name: "主卧".into(),
            material_id: None,
            counted: true,
            floor: 0,
            ceiling_material_id: None,
        });
        doc.rooms.push(Room {
            id: EntityId(9002),
            polygon: vec![
                Pt::new(0, 4000),
                Pt::new(3000, 4000),
                Pt::new(3000, 7000),
                Pt::new(0, 7000),
            ],
            name: "次卧".into(),
            material_id: None,
            counted: true,
            floor: 0,
            ceiling_material_id: None,
        });
        let mut state =
            SceneState::from_doc(doc, [1280.0, 800.0], PathBuf::from("/tmp/roof-l.json"));
        state.show_roof = true;
        state.cutaway = false;
        let (min, max) = plan_bounds(&state.doc);
        let levels = state.doc.level_count();
        let top = (state.doc.level_elevation_mm(levels - 1) + state.doc.level_height_mm(levels - 1))
            as f32
            / 1000.0;
        let cx = (min.x + max.x) as f32 / 2000.0;
        let cz = (min.y + max.y) as f32 / 2000.0;
        let diag =
            (((max.x - min.x) as f32).powi(2) + ((max.y - min.y) as f32).powi(2) + top.powi(2))
                .sqrt()
                / 1000.0;
        state.cam3.target = Vec3::new(cx, top * 0.5, cz);
        state.cam3.distance = (diag * 1.45).clamp(6.0, 40.0);
        state.cam3.pitch = 0.48;
        state.cam3.yaw = 0.20;
        let mesh = state.mesh3d();
        let look = state.look3d();
        render_3d_frame(
            &mesh,
            Path::new("/tmp/roof-l-probe.png"),
            1200,
            800,
            state.cam3.view_proj(1.5),
            state.cam3.eye().to_array(),
            look,
        )
        .expect("render L roof");
    }

    #[test]
    #[ignore = "needs a GPU; cover sheet probe"]
    fn cover_sheet_probe_writes_png() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/cover.json"));
        let (w, h, rgba) = construction_sheet(&state, SheetKind::Cover).expect("cover sheet");
        write_png(Path::new("/tmp/fp-cover-page.png"), w, h, &rgba).expect("write cover page");
    }

    #[test]
    #[ignore = "needs a GPU; plan sheet probe"]
    fn plan_sheet_probe_writes_png() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/plan.json"));
        let (w, h, rgba) = construction_sheet(&state, SheetKind::Plan(0)).expect("plan sheet");
        write_png(Path::new("/tmp/fp-plan-page.png"), w, h, &rgba).expect("write plan page");
    }

    #[test]
    #[ignore = "needs a GPU; mixed wall finish plan probe"]
    fn mixed_wall_finish_plan_probe_writes_png() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let mut state = SceneState::from_doc(
            doc,
            [1280.0, 720.0],
            PathBuf::from("/tmp/fp-mixed-plan.json"),
        );
        let mut walls: Vec<(f64, EntityId)> = state
            .doc
            .walls
            .iter()
            .filter(|w| w.floor == 0)
            .map(|w| {
                let len = ((w.end.x - w.start.x) as f64).hypot((w.end.y - w.start.y) as f64);
                (len, w.id)
            })
            .collect();
        walls.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let wall_ids: Vec<EntityId> = walls.into_iter().take(3).map(|(_, id)| id).collect();
        for (id, kind) in wall_ids.iter().zip(["wallpaper", "wood_panel", "tile"]) {
            let material = state
                .doc
                .materials
                .iter()
                .find(|m| m.surface == FinishSurface::Wall && m.kind == kind)
                .expect("built-in wall finish")
                .id;
            if let Some(wall) = state.doc.walls.iter_mut().find(|w| w.id == *id) {
                wall.finish_id = Some(material);
            }
        }
        let (w, h, rgba) = construction_sheet(&state, SheetKind::Plan(0)).expect("mixed plan");
        write_png(Path::new("/tmp/fp-plan-mixed.png"), w, h, &rgba).expect("write mixed plan");
    }

    #[test]
    #[ignore = "needs a GPU; finish sheet probe"]
    fn finish_sheet_probe_writes_png() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/finish.json"));
        let (w, h, rgba) = construction_sheet(&state, SheetKind::Finish(0)).expect("finish sheet");
        write_png(Path::new("/tmp/fp-finish-page.png"), w, h, &rgba).expect("write finish page");
    }

    #[test]
    #[ignore = "needs a GPU; elevation sheet probe"]
    fn elevation_sheet_probe_writes_png() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let state =
            SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/elevation.json"));
        let (w, h, rgba) =
            construction_sheet(&state, SheetKind::Elevations).expect("elevation sheet");
        write_png(Path::new("/tmp/fp-elevation-page.png"), w, h, &rgba)
            .expect("write elevation page");
    }

    #[test]
    #[ignore = "needs a GPU; section sheet probe"]
    fn section_sheet_probe_writes_png() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/section.json"));
        let (w, h, rgba) = construction_sheet(&state, SheetKind::Section).expect("section sheet");
        write_png(Path::new("/tmp/fp-section-page.png"), w, h, &rgba).expect("write section page");
    }

    #[test]
    #[ignore = "needs a GPU; details sheet probe"]
    fn details_sheet_probe_writes_png() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo");
        let state = SceneState::from_doc(doc, [1280.0, 720.0], PathBuf::from("/tmp/details.json"));
        let (w, h, rgba) = construction_sheet(&state, SheetKind::Details).expect("details sheet");
        write_png(Path::new("/tmp/fp-details-page.png"), w, h, &rgba).expect("write details page");
    }

    #[test]
    fn sheet_kinds_include_elevations_section_and_details() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let mut doc = Document::from_json(json).expect("demo");
        let kinds = sheet_kinds(&doc);
        assert_eq!(kinds.len(), 7);
        assert_eq!(kinds[3].drawing_no(), "A-01");
        assert_eq!(kinds[4].drawing_no(), "A-02");
        assert_eq!(kinds[5].drawing_no(), "A-03");
        assert_eq!(kinds[6].drawing_no(), "D-01");
        doc.ensure_levels();
        doc.levels.push(fp_model::Level {
            name: "2F".to_string(),
            height_mm: 2800,
        });
        let kinds = sheet_kinds(&doc);
        assert_eq!(kinds.len(), 9);
        assert_eq!(kinds[5].drawing_no(), "A-01");
        assert_eq!(kinds[6].drawing_no(), "A-02");
        assert_eq!(kinds[7].drawing_no(), "A-03");
        assert_eq!(kinds[8].drawing_no(), "D-01");
    }

    #[test]
    #[ignore = "needs a GPU; two-floor elevation/section/cover probe"]
    fn two_floor_sheets_probe_writes_png() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let mut doc = Document::from_json(json).expect("demo");
        doc.ensure_levels();
        doc.levels.push(fp_model::Level {
            name: "2F".to_string(),
            height_mm: 2800,
        });
        let walls: Vec<Wall> = doc.walls.iter().filter(|w| w.floor == 0).cloned().collect();
        for mut wall in walls {
            wall.id = EntityId(doc.next_id());
            wall.floor = 1;
            doc.walls.push(wall);
        }
        let rooms: Vec<fp_model::Room> =
            doc.rooms.iter().filter(|r| r.floor == 0).cloned().collect();
        for mut room in rooms {
            room.id = EntityId(doc.next_id());
            room.floor = 1;
            doc.rooms.push(room);
        }
        let state = SceneState::from_doc(
            doc,
            [1280.0, 720.0],
            PathBuf::from("/tmp/two-floor-sheets.json"),
        );
        for (kind, path) in [
            (SheetKind::Elevations, "/tmp/fp-elevation-2f.png"),
            (SheetKind::Section, "/tmp/fp-section-2f.png"),
            (SheetKind::Cover, "/tmp/fp-cover-2f.png"),
        ] {
            let (w, h, rgba) = construction_sheet(&state, kind).expect("two-floor sheet");
            write_png(Path::new(path), w, h, &rgba).expect("write two-floor page");
        }
    }

    #[test]
    #[ignore = "needs a GPU; run with --ignored"]
    fn construction_sheet_writes_an_a3_pdf() {
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = Document::from_json(json).expect("demo json");
        let state = SceneState::from_doc(
            doc,
            [1280.0, 720.0],
            PathBuf::from("/tmp/fp-construction.json"),
        );
        let path = PathBuf::from("/tmp/fp-construction-test.pdf");
        let msg = export_construction_pdf(&state, &path).expect("construction pdf");
        assert!(msg.contains("A3"), "{msg}");
        assert!(msg.contains("7 页"), "{msg}");
        let bytes = std::fs::read(&path).expect("pdf exists");
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.len() > 200_000, "pdf is {} bytes", bytes.len());
    }

    #[test]
    #[ignore = "needs a GPU; close-up material probe"]
    fn material_macro_probe_writes_png() {
        use glam::Vec3;
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = fp_model::Document::from_json(json).expect("demo json");
        let sofa = doc
            .furniture
            .iter()
            .find(|f| f.asset_id == "sofa")
            .expect("demo sofa");
        let cx = sofa.center.x as f32 / 1000.0;
        let cz = sofa.center.y as f32 / 1000.0;
        let mut state = SceneState::from_doc(
            doc,
            [1280.0, 720.0],
            PathBuf::from("/tmp/material-macro.json"),
        );
        state.cutaway = true;
        state.cam3.target = Vec3::new(cx, 0.55, cz);
        state.cam3.distance = 2.6;
        state.cam3.pitch = 0.34;
        state.cam3.yaw = 0.92;
        let mesh = state.mesh3d();
        render_3d_frame(
            &mesh,
            Path::new("/tmp/fp-material-macro.png"),
            1600,
            1000,
            state.cam3.view_proj(1.6),
            state.cam3.eye().to_array(),
            state.look3d(),
        )
        .expect("material macro");
    }

    #[test]
    #[ignore]
    fn surface_micro_probe_writes_png() {
        use glam::Vec3;
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        let doc = fp_model::Document::from_json(json).expect("demo json");
        let mut state = SceneState::from_doc(
            doc,
            [1280.0, 720.0],
            PathBuf::from("/tmp/surface-micro.json"),
        );
        state.cutaway = false;
        state.cam3.target = Vec3::new(9.4, 1.35, 6.49);
        state.cam3.distance = 1.0;
        state.cam3.pitch = 0.0;
        state.cam3.yaw = 0.0;
        let mesh = state.mesh3d();
        render_3d_frame(
            &mesh,
            Path::new("/tmp/fp-surface-micro.png"),
            1600,
            1000,
            state.cam3.view_proj(1.6),
            state.cam3.eye().to_array(),
            state.look3d(),
        )
        .expect("surface micro");
    }

    #[test]
    #[ignore]
    fn wall_finish_probe_writes_pngs() {
        use glam::Vec3;
        let json = include_str!("../../../assets/samples/demo-apartment.json");
        for (finish, slug) in [
            (WallFinish::Paint, "paint"),
            (WallFinish::Wallpaper, "wallpaper"),
            (WallFinish::WoodPanel, "wood"),
            (WallFinish::Tile, "tile"),
        ] {
            let mut doc = fp_model::Document::from_json(json).expect("demo json");
            if let Some(wall) = doc.walls.iter_mut().find(|w| w.id == EntityId(48)) {
                wall.finish = finish;
            }
            let mut state = SceneState::from_doc(
                doc,
                [1280.0, 720.0],
                PathBuf::from(format!("/tmp/wall-{slug}.json")),
            );
            state.cutaway = false;
            state.cam3.target = Vec3::new(9.4, 1.35, 6.49);
            state.cam3.distance = 1.35;
            state.cam3.pitch = 0.0;
            state.cam3.yaw = 0.0;
            let mesh = state.mesh3d();
            render_3d_frame(
                &mesh,
                Path::new(&format!("/tmp/fp-finish-{slug}.png")),
                1600,
                1000,
                state.cam3.view_proj(1.6),
                state.cam3.eye().to_array(),
                state.look3d(),
            )
            .expect("wall finish probe");
        }
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
