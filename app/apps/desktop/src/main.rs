//! Floorplan-3D desktop editor: GPUI shell + wgpu plan view.
//!
//! The plan view renders on its own thread into a `WgpuSurface`; GPUI owns the
//! window, the toolbar and all input, and shares state through a mutex.

mod envmap;
mod export;
mod favorites;
mod i18n;
mod import;
mod recent;
mod render3d;
mod scene;
mod templates;
mod text_input;
mod underlay;
mod underlay_gpu;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use fp_model::WallKind;
use fp_model::catalog::{CATALOG, FurnitureCategory, TABS};
use fp_render2d::Batch;
use fp_ui::Tool;
use glam::Mat4;
use render3d::Renderer3d;
use scene::{
    InspectorAction, InspectorEdit, InspectorKind, InspectorView, Layer, PointerButton, Preset3d,
    SceneState, ViewMode,
};
use wgpui::{
    Animation, AnimationExt as _, AnyElement, App, Application, Bounds, Context, DragMoveEvent,
    Entity, FocusHandle, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString, TitlebarOptions,
    WeakEntity, WgpuSurfaceHandle, Window, WindowBounds, WindowOptions, div, ease_out_quint,
    prelude::*, px, rgb, rgba, size, svg, wgpu_surface,
};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 800;
const MAX_VERTICES: usize = 240_000;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    color: [f32; 4],
}

// ------------------------------------------------------------------ ui shell

#[derive(Clone)]
struct FurnitureDrag {
    index: usize,
}

struct FurnitureDragPreview;

impl Render for FurnitureDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // A single ghost tile follows the cursor. The furniture name used to
        // float above the plan as a second layer, which read like a duplicate
        // object while dragging; the tile alone is enough feedback.
        div()
            .w(px(30.0))
            .h(px(30.0))
            .rounded(px(8.0))
            .bg(rgba(0x23283318))
            .border_1()
            .border_color(rgba(0x2328332e))
            .flex()
            .items_center()
            .justify_center()
            .child(ui_icon(ICON_FURNITURE, 15.0, PANEL_TEXT))
    }
}

struct PlanEditor {
    surface: WgpuSurfaceHandle,
    state: Arc<Mutex<SceneState>>,
    focus: FocusHandle,
    focused: bool,
    fps: f64,
    fps_rx: Receiver<f64>,
    last_autosave: std::time::Instant,
    rename_input: Entity<text_input::TextInput>,
    rename_rx: Receiver<text_input::InputEvent>,
    renaming: Option<fp_core::EntityId>,
    /// Non-modal numeric editor opened from a clickable inspector field.
    inspector_edit: Option<InspectorEdit>,
    inspector_input: Entity<text_input::TextInput>,
    inspector_rx: Receiver<text_input::InputEvent>,
    project_open: bool,
    project_inputs: [Entity<text_input::TextInput>; 4],
    project_rx: Receiver<text_input::InputEvent>,
    import_open: bool,
    import_input: Entity<text_input::TextInput>,
    import_rx: Receiver<text_input::InputEvent>,
    import_path: Option<PathBuf>,
    library_open: bool,
    library_input: Entity<text_input::TextInput>,
    library_rx: Receiver<text_input::InputEvent>,
    library_query: String,
    favorites: Vec<String>,
    favorites_only: bool,
    layers_open: bool,
    /// Right inspector / 3D panel visibility; `]` collapses it like the web
    /// original's property-panel shortcut.
    right_open: bool,
    more_open: bool,
    export_open: bool,
    scale_open: bool,
    /// Wall-kind dropdown inside the inspector.
    wall_kind_open: bool,
    help_open: bool,
    /// Transient top-right toast. `SceneState::note` is the source, but the
    /// desktop shell keeps it on screen for a couple of seconds instead of
    /// letting an old status sentence pile up next to the toolbar.
    toast_msg: Option<String>,
    toast_visible: bool,
    toast_gen: u64,
    /// Last viewport reported by the window, used to reject canvas clicks that
    /// land on floating chrome. Without this the GPUI root also receives the
    /// bubbled click from every button on top of the wgpu surface.
    viewport: [f32; 2],
    /// Whether the inspector currently has a body (a selection exists).
    inspector_shown: bool,
    /// Last pointer position seen during a library drag, used to place the
    /// furniture where the mouse is released.
    drag_pos: Option<[f32; 2]>,
    sun_drag: bool,
    library_scroll: usize,
    new_menu_open: bool,
    recent_plans: Vec<recent::RecentPlan>,
    window_title: String,
}

impl PlanEditor {
    /// Floating panels, rails and status chips sit above the wgpu surface, but
    /// GPUI bubbles their mouse events to this root. Reject those points before
    /// they reach the plan so a button click never also selects or places.
    fn pointer_over_chrome(&self, pos: [f32; 2], view: ViewMode) -> bool {
        let [w, h] = self.viewport;
        if self.help_open {
            return true;
        }
        if pos[1] < 48.0 {
            return true; // titlebar drag strip and floating top chrome
        }
        // Left icon rail. It ends at ~460px on an 800px window.
        if pos[0] <= 66.0 && pos[1] <= 470.0 {
            return true;
        }
        // Two small bottom clusters instead of the old full-width bar.
        if pos[0] <= 105.0 && pos[1] >= h - 52.0 {
            return true;
        }
        if pos[0] >= w - 220.0 && pos[1] >= h - 52.0 {
            return true;
        }
        if self.scale_open && pos[0] <= 220.0 && pos[1] >= h - 250.0 {
            return true;
        }
        // Top-right menus.
        if self.more_open && pos[0] >= w - 250.0 && pos[1] <= 310.0 {
            return true;
        }
        if self.export_open && pos[0] >= w - 290.0 && pos[1] <= 310.0 {
            return true;
        }
        if self.library_open && pos[0] <= 334.0 && pos[1] >= 96.0 && pos[1] <= 700.0 {
            return true;
        }
        if self.layers_open && pos[0] <= 300.0 && pos[1] >= h - 390.0 {
            return true;
        }
        match view {
            ViewMode::Plan => {
                if self.right_open && self.inspector_shown {
                    if pos[0] >= w - 292.0 && pos[1] <= 620.0 {
                        return true;
                    }
                }
            }
            ViewMode::Three => {
                // The 3D lighting panel sits to the right of the icon rail.
                if pos[0] <= 300.0 && pos[1] <= 680.0 {
                    return true;
                }
                if self.right_open && pos[0] >= w - 292.0 && pos[1] <= 620.0 {
                    return true;
                }
            }
            ViewMode::Walk => {
                // On-screen D-pad in the bottom-left corner.
                if pos[0] <= 140.0 && pos[1] >= h - 180.0 {
                    return true;
                }
                if self.right_open && self.inspector_shown {
                    if pos[0] >= w - 292.0 && pos[1] <= 620.0 {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn down(&mut self, ev: &MouseDownEvent) {
        if self.new_menu_open
            || self.project_open
            || self.import_open
            || self.help_open
            || self.inspector_edit.is_some()
        {
            return;
        }
        let pos = [f32::from(ev.position.x), f32::from(ev.position.y)];
        let button = match ev.button {
            MouseButton::Left => PointerButton::Left,
            MouseButton::Right => PointerButton::Right,
            MouseButton::Middle => PointerButton::Middle,
            _ => return,
        };
        let shift = ev.modifiers.shift;
        let mut state = self.state.lock().unwrap();
        let over_chrome = self.pointer_over_chrome(pos, state.view);
        state.set_pointer_over_ui(over_chrome);
        if over_chrome {
            return;
        }
        state.pointer_down(pos, button, shift);
        drop(state);
        self.more_open = false;
        self.export_open = false;
        self.scale_open = false;
        self.wall_kind_open = false;
    }

    fn moved(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.new_menu_open
            || self.project_open
            || self.import_open
            || self.help_open
            || self.inspector_edit.is_some()
        {
            return;
        }
        let pos = [f32::from(ev.position.x), f32::from(ev.position.y)];
        if self.sun_drag {
            let value = ((pos[0] - SUN_SLIDER_LEFT) / SUN_SLIDER_WIDTH).clamp(0.0, 1.0);
            self.state.lock().unwrap().set_sun(value);
            cx.notify();
            return;
        }
        let shift = ev.modifiers.shift;
        let mut state = self.state.lock().unwrap();
        let over_chrome = self.pointer_over_chrome(pos, state.view);
        state.set_pointer_over_ui(over_chrome);
        if over_chrome && !state.pointer_captured() {
            return;
        }
        state.set_modifiers(shift);
        state.pointer_move(pos);
    }

    fn up(&mut self, ev: &MouseUpEvent) {
        if self.new_menu_open
            || self.project_open
            || self.import_open
            || self.help_open
            || self.inspector_edit.is_some()
        {
            return;
        }
        self.sun_drag = false;
        let pos = [f32::from(ev.position.x), f32::from(ev.position.y)];
        let button = match ev.button {
            MouseButton::Left => PointerButton::Left,
            MouseButton::Middle => PointerButton::Middle,
            _ => return,
        };
        let shift = ev.modifiers.shift;
        {
            let mut state = self.state.lock().unwrap();
            let over_chrome = self.pointer_over_chrome(pos, state.view);
            state.set_pointer_over_ui(over_chrome);
            state.clear_walk_stick();
            if over_chrome && !state.pointer_captured() {
                return;
            }
            state.pointer_up(pos, button, shift);
        }
    }

    fn wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if self.new_menu_open
            || self.project_open
            || self.import_open
            || self.inspector_edit.is_some()
        {
            return;
        }
        let pos = [f32::from(ev.position.x), f32::from(ev.position.y)];
        let dy = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y * 20.0,
        };
        if self.library_open && pos[0] >= 98.0 && pos[0] <= 306.0 && pos[1] >= 52.0 {
            let category = self.state.lock().unwrap().catalog_filter;
            let total = catalog_visible_count(
                category,
                &self.library_query,
                &self.favorites,
                self.favorites_only,
            );
            let step = if dy > 0.0 { -1 } else { 1 };
            let max = total.saturating_sub(LIBRARY_ROWS);
            self.library_scroll = (self.library_scroll as i32 + step).clamp(0, max as i32) as usize;
            return;
        }
        let view = self.state.lock().unwrap().view;
        if self.pointer_over_chrome(pos, view) {
            return;
        }
        self.state.lock().unwrap().scroll(pos, dy);
        cx.notify();
    }

    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let cmd = ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control;
        let shift = ev.keystroke.modifiers.shift;
        let alt = ev.keystroke.modifiers.alt;
        let key = ev.keystroke.key.clone();
        if self.help_open {
            if key == "escape" {
                self.help_open = false;
                cx.notify();
            }
            return;
        }
        if key == "escape" && (self.more_open || self.export_open || self.scale_open) {
            self.more_open = false;
            self.export_open = false;
            self.scale_open = false;
            self.wall_kind_open = false;
            cx.notify();
            return;
        }
        if cmd && key == "n" {
            self.open_new_menu(cx);
            return;
        }
        if cmd && key == "i" {
            self.open_project_info(window, cx);
            return;
        }
        if cmd && key == "p" {
            self.print_current(cx);
            return;
        }
        if self.inspector_edit.is_some() {
            if key == "escape" {
                self.close_inspector_edit(window, cx);
            }
            return;
        }
        if self.project_open {
            return;
        }
        if self.import_open {
            if key == "escape" {
                self.close_import(window, cx);
            }
            return;
        }
        if self.new_menu_open {
            if key == "escape" {
                self.new_menu_open = false;
                window.focus(&self.focus, cx);
                cx.notify();
            }
            return;
        }
        if cmd && key == "s" {
            self.state.lock().unwrap().save_now();
            cx.notify();
            return;
        }
        if shift && key == "f" && !cmd {
            window.toggle_fullscreen();
            cx.notify();
            return;
        }
        if !cmd && (key == "[" || key == "]") {
            if alt {
                let delta = if key == "[" { -100 } else { 100 };
                self.state.lock().unwrap().resize_furniture(delta, 0);
            } else if key == "[" {
                self.library_open = !self.library_open;
                if self.library_open {
                    self.layers_open = false;
                }
            } else {
                self.right_open = !self.right_open;
            }
            cx.notify();
            return;
        }
        if key == "l" && !cmd {
            self.library_open = !self.library_open;
            if self.library_open {
                self.layers_open = false;
            }
            cx.notify();
            return;
        }
        if key == "n" && !cmd && self.state.lock().unwrap().selected_room.is_some() {
            self.start_rename(window, cx);
            return;
        }
        self.state.lock().unwrap().key(&key, cmd, shift);
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, name)) = self.state.lock().unwrap().selected_room_prompt() else {
            return;
        };
        self.renaming = Some(id);
        self.rename_input
            .update(cx, |input, cx| input.set_text(&name, cx));
        let handle = self.rename_input.read(cx).handle();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn commit_rename(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.renaming.take() {
            let name = text.trim().to_string();
            if !name.is_empty() {
                self.state.lock().unwrap().rename_room(id, &name);
            }
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn start_inspector_edit(
        &mut self,
        edit: InspectorEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = {
            let state = self.state.lock().unwrap();
            state
                .inspector()
                .and_then(|view| edit.current(&view).map(|field| field.value.clone()))
        };
        let Some(value) = current else {
            return;
        };
        self.inspector_edit = Some(edit);
        self.inspector_input
            .update(cx, |input, cx| input.set_text(&value, cx));
        let handle = self.inspector_input.read(cx).handle();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn commit_inspector_edit(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.inspector_edit.take() else {
            return;
        };
        let trimmed = text.trim();
        if let Ok(value) = trimmed.parse::<f64>() {
            let value = value.round() as i32;
            let action = match edit {
                InspectorEdit::FurnitureWidth => Some(InspectorAction::FurnitureSetWidth(value)),
                InspectorEdit::FurnitureDepth => Some(InspectorAction::FurnitureSetDepth(value)),
                InspectorEdit::FurnitureX => Some(InspectorAction::FurnitureSetX(value)),
                InspectorEdit::FurnitureY => Some(InspectorAction::FurnitureSetY(value)),
                InspectorEdit::WallLength => Some(InspectorAction::WallSetLength(value)),
                InspectorEdit::WallThickness => Some(InspectorAction::WallSetThickness(value)),
                InspectorEdit::WallHeight => Some(InspectorAction::WallSetHeight(value)),
                InspectorEdit::OpeningWidth => Some(InspectorAction::OpeningSetWidth(value)),
                InspectorEdit::OpeningHeight => Some(InspectorAction::OpeningSetHeight(value)),
                InspectorEdit::OpeningSill => Some(InspectorAction::OpeningSetSill(value)),
            };
            if let Some(action) = action {
                self.state.lock().unwrap().apply_action(action);
            }
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn close_inspector_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.inspector_edit = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn open_project_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (name, client, address, note) = self.state.lock().unwrap().project_info();
        for (input, text) in self
            .project_inputs
            .iter()
            .zip([name, client, address, note])
        {
            input.update(cx, |field, cx| field.set_text(&text, cx));
        }
        self.project_open = true;
        let handle = self.project_inputs[0].read(cx).handle();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn save_project_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = [
            self.project_inputs[0].read(cx).text(),
            self.project_inputs[1].read(cx).text(),
            self.project_inputs[2].read(cx).text(),
            self.project_inputs[3].read(cx).text(),
        ];
        let _ = self
            .state
            .lock()
            .unwrap()
            .update_project_info(&values[0], &values[1], &values[2], &values[3]);
        self.project_open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn close_project_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn pick_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.more_open = false;
        self.export_open = false;
        self.scale_open = false;
        self.wall_kind_open = false;
        let picked = rfd::FileDialog::new()
            .add_filter("户型图", &["png", "jpg", "jpeg"])
            .pick_file();
        let Some(path) = picked else {
            return;
        };
        self.import_path = Some(path);
        self.import_input
            .update(cx, |input, cx| input.set_text("12000", cx));
        self.import_open = true;
        let handle = self.import_input.read(cx).handle();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn confirm_import(&mut self, recognize: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.import_path.clone() else {
            return;
        };
        let raw = self.import_input.read(cx).text();
        let width_mm: i32 = raw.trim().parse().unwrap_or(0);
        if !(1_000..=100_000).contains(&width_mm) {
            self.state.lock().unwrap().note = Some("图纸宽度需在 1000–100000 mm 之间".into());
            cx.notify();
            return;
        }
        let name = path
            .file_stem()
            .map(|s| format!("{} 户型", s.to_string_lossy()))
            .unwrap_or_else(|| "导入户型".into());
        match import::import_floorplan(&path, width_mm, &name, recognize) {
            Ok(outcome) => {
                let count = outcome.recognized_walls;
                let project = scene::next_project_path(&name);
                {
                    let mut state = self.state.lock().unwrap();
                    state.new_document(outcome.doc, project);
                    state.note = Some(if recognize {
                        format!("已导入底图 · 识别 {count} 段墙，请核对后补门窗")
                    } else {
                        "已导入底图 · 可直接描图".into()
                    });
                }
                self.import_open = false;
                self.import_path = None;
                window.focus(&self.focus, cx);
                cx.notify();
            }
            Err(err) => {
                self.state.lock().unwrap().note = Some(format!("导入失败 · {err}"));
                cx.notify();
            }
        }
    }

    fn close_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.import_open = false;
        self.import_path = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn archive_recent(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let current = self.state.lock().unwrap().saved_path.clone();
        if path == current {
            self.state.lock().unwrap().note = Some("当前打开的方案不能归档".into());
            cx.notify();
            return;
        }
        let note = match recent::archive(&path) {
            Ok(target) => format!("已归档到 {}", scene::display_path(&target)),
            Err(err) => err,
        };
        self.state.lock().unwrap().note = Some(note);
        self.recent_plans = recent::load();
        cx.notify();
    }

    fn forget_recent(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        recent::forget(&path);
        self.recent_plans = recent::load();
        self.state.lock().unwrap().note = Some("已从最近方案移除".into());
        cx.notify();
    }

    fn toggle_favorite(&mut self, asset_id: &str, cx: &mut Context<Self>) {
        self.favorites = favorites::toggle(asset_id);
        cx.notify();
    }

    fn print_current(&mut self, cx: &mut Context<Self>) {
        let outcome = {
            let state = self.state.lock().unwrap();
            export::print_plan(&state)
        };
        self.state.lock().unwrap().note = Some(outcome);
        cx.notify();
    }

    fn open_new_menu(&mut self, cx: &mut Context<Self>) {
        self.recent_plans = recent::load();
        self.new_menu_open = true;
        self.renaming = None;
        self.more_open = false;
        self.export_open = false;
        self.scale_open = false;
        self.wall_kind_open = false;
        cx.notify();
    }

    fn create_new(&mut self, kind: templates::TemplateKind, cx: &mut Context<Self>) {
        let lang = self.state.lock().unwrap().lang;
        let doc = templates::build(kind, lang);
        let name = doc.meta.name.clone();
        let path = scene::next_project_path(&name);
        self.state.lock().unwrap().new_document(doc, path);
        self.new_menu_open = false;
        self.recent_plans = recent::load();
        cx.notify();
    }

    fn open_recent(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.state.lock().unwrap().open_path(path);
        self.new_menu_open = false;
        self.recent_plans = recent::load();
        cx.notify();
    }

    fn key_release(&mut self, ev: &KeyUpEvent) {
        let shift = ev.keystroke.modifiers.shift;
        let key = ev.keystroke.key.clone();
        self.state.lock().unwrap().key_up(&key, shift);
    }
}

impl Render for PlanEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        while let Ok(ev) = self.rename_rx.try_recv() {
            match ev {
                text_input::InputEvent::Submit(text) => self.commit_rename(text, window, cx),
                text_input::InputEvent::Cancel => {
                    self.renaming = None;
                    window.focus(&self.focus, cx);
                    cx.notify();
                }
                text_input::InputEvent::Changed(_) => {}
            }
        }
        while let Ok(ev) = self.inspector_rx.try_recv() {
            match ev {
                text_input::InputEvent::Submit(text) => {
                    self.commit_inspector_edit(text, window, cx)
                }
                text_input::InputEvent::Cancel => self.close_inspector_edit(window, cx),
                text_input::InputEvent::Changed(_) => {}
            }
        }
        while let Ok(ev) = self.project_rx.try_recv() {
            match ev {
                text_input::InputEvent::Submit(_) => self.save_project_info(window, cx),
                text_input::InputEvent::Cancel => self.close_project_info(window, cx),
                text_input::InputEvent::Changed(_) => {}
            }
        }
        while let Ok(ev) = self.import_rx.try_recv() {
            match ev {
                text_input::InputEvent::Submit(_) => self.confirm_import(true, window, cx),
                text_input::InputEvent::Cancel => self.close_import(window, cx),
                text_input::InputEvent::Changed(_) => {}
            }
        }
        while let Ok(ev) = self.library_rx.try_recv() {
            match ev {
                text_input::InputEvent::Changed(text) => {
                    self.library_query = text;
                    self.library_scroll = 0;
                    cx.notify();
                }
                text_input::InputEvent::Submit(_) => {
                    window.focus(&self.focus, cx);
                    cx.notify();
                }
                text_input::InputEvent::Cancel => {
                    self.library_input
                        .update(cx, |input, cx| input.set_text("", cx));
                    self.library_query.clear();
                    window.focus(&self.focus, cx);
                    cx.notify();
                }
            }
        }
        while let Ok(fps) = self.fps_rx.try_recv() {
            self.fps = fps;
        }
        if self.last_autosave.elapsed() >= std::time::Duration::from_secs(12) {
            self.last_autosave = std::time::Instant::now();
            if self.state.lock().unwrap().autosave_if_dirty() {
                cx.notify();
            }
        }
        window.request_animation_frame();
        if !self.focused {
            window.focus(&self.focus, cx);
            self.focused = true;
        }

        let viewport = window.viewport_size();
        {
            let size = [f32::from(viewport.width), f32::from(viewport.height)];
            if size[0] > 1.0 && size[1] > 1.0 {
                self.viewport = size;
                self.state.lock().unwrap().set_view_size(size);
            }
        }

        let (
            tool,
            view,
            labels,
            inspector,
            lang,
            note,
            doc_name,
            scale_denom,
            dirty,
            plan_area,
            cutaway,
            night,
            sun,
        ) = {
            let s = self.state.lock().unwrap();
            (
                s.tool,
                s.view,
                if s.view == ViewMode::Plan {
                    s.labels()
                } else {
                    Vec::new()
                },
                s.inspector(),
                s.lang,
                s.note
                    .clone()
                    .filter(|n| !n.starts_with("已保存") && !n.starts_with("Saved")),
                s.doc.meta.name.clone(),
                s.scale_denominator(),
                s.is_dirty(),
                s.quote_totals().net_area,
                s.cutaway,
                s.night,
                s.sun,
            )
        };
        if let Some(msg) = note.clone() {
            if !self.toast_visible || self.toast_msg.as_deref() != Some(msg.as_str()) {
                self.toast_gen = self.toast_gen.wrapping_add(1);
                let generation = self.toast_gen;
                self.toast_msg = Some(msg.clone());
                self.toast_visible = true;
                let expiry_msg = msg;
                let weak = cx.entity().downgrade();
                cx.spawn(async move |_this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(2600))
                        .await;
                    if let Some(entity) = weak.upgrade() {
                        entity
                            .update(cx, |this, cx| {
                                if this.toast_gen == generation {
                                    this.toast_visible = false;
                                    this.toast_msg = None;
                                    let mut state = this.state.lock().unwrap();
                                    if state.note.as_deref() == Some(expiry_msg.as_str()) {
                                        state.note = None;
                                    }
                                    drop(state);
                                    cx.notify();
                                }
                            })
                            .ok();
                    }
                })
                .detach();
            }
        }
        let note = if self.toast_visible {
            self.toast_msg.clone()
        } else {
            None
        };
        let t = i18n::T(lang);
        let title = if doc_name.trim().is_empty() {
            "Fathom 方寸".to_string()
        } else {
            format!("{} · Fathom 方寸", doc_name.trim())
        };
        if self.window_title != title {
            window.set_window_title(&title);
            self.window_title = title;
        }
        let focus = self.focus.clone();
        let renaming = self.renaming;
        let project_open = self.project_open;
        let project_inputs = self.project_inputs.clone();
        let inspector_edit = self.inspector_edit;
        let inspector_input = self.inspector_input.clone();
        let import_open = self.import_open;
        let import_input = self.import_input.clone();
        let import_name = self
            .import_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let library_open = self.library_open;
        let library_input = self.library_input.clone();
        let library_query = self.library_query.clone();
        let library_favorites = self.favorites.clone();
        let favorites_only = self.favorites_only;
        let layers_open = self.layers_open;
        let right_open = self.right_open;
        let more_open = self.more_open;
        let export_open = self.export_open;
        let scale_open = self.scale_open;
        let wall_kind_open = self.wall_kind_open;
        let help_open = self.help_open;
        self.inspector_shown = right_open && inspector.is_some();

        let rooms3d = if view == ViewMode::Three {
            self.state
                .lock()
                .unwrap()
                .room_list()
                .into_iter()
                .map(|(id, name, area)| (id.0, name, area))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let (layer_flags, measure_count, furniture_count) = {
            let state = self.state.lock().unwrap();
            (
                [
                    state.layer_on(Layer::RoomNames),
                    state.layer_on(Layer::Furniture),
                    state.layer_on(Layer::Grid),
                    state.layer_on(Layer::Dimensions),
                    state.layer_on(Layer::Bearing),
                    state.layer_on(Layer::Underlay),
                ],
                state.measure_count(),
                state.furniture_count(),
            )
        };
        let (library_category, catalog_index, furniture_armed) = {
            let state = self.state.lock().unwrap();
            (
                state.catalog_filter,
                state.catalog_index,
                state.furniture_armed,
            )
        };
        let library_scroll = self.library_scroll;
        let dark_canvas = matches!(view, ViewMode::Walk) || (view == ViewMode::Three && night);
        let canvas_bg = if dark_canvas { 0x0b0d12 } else { 0xf6f4ef };
        let rename_weak = cx.entity().downgrade();
        let import_weak = cx.entity().downgrade();
        let rename_input = self.rename_input.clone();
        let scale_weak = cx.entity().downgrade();
        let walk_weak = cx.entity().downgrade();
        let help_weak = cx.entity().downgrade();
        let more_weak = cx.entity().downgrade();
        let more_close_weak = cx.entity().downgrade();
        let export_close_weak = cx.entity().downgrade();

        let view_label = match view {
            ViewMode::Plan => t.s("2D 平面", "2D plan"),
            ViewMode::Three => t.s("3D 视图", "3D view"),
            ViewMode::Walk => t.s("第一人称漫游", "First-person walk"),
        };

        div()
            .relative()
            .size_full()
            .bg(rgb(canvas_bg))
            .text_color(if dark_canvas {
                rgb(0xe6e9f0)
            } else {
                rgb(0x252b36)
            })
            .track_focus(&focus)
            .on_drag_move::<FurnitureDrag>(cx.listener(
                |this, ev: &DragMoveEvent<FurnitureDrag>, _window, cx| {
                    this.drag_pos = Some([
                        f32::from(ev.event.position.x),
                        f32::from(ev.event.position.y),
                    ]);
                    cx.notify();
                },
            ))
            .on_drop(cx.listener(|this, data: &FurnitureDrag, _window, cx| {
                if let Some(pos) = this.drag_pos.take() {
                    let mut state = this.state.lock().unwrap();
                    if !this.pointer_over_chrome(pos, state.view) {
                        state.place_furniture_at(data.index, pos);
                    }
                }
                cx.notify();
            }))
            .child(wgpu_surface(self.surface.clone()).absolute().inset_0())
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .h(px(44.0))
                    .bg(rgba(0x00000000))
                    .on_mouse_down(MouseButton::Left, move |_ev, window, _cx| {
                        begin_window_drag(window);
                    }),
            )
            // ------------------------------------------------------------------
            // Floating top chrome: no white strip, each cluster has its own glass.
            // macOS traffic lights occupy the first ~76px, so the brand starts
            // just to their right while the rest stays visually top-left.
            .child(
                div()
                    .absolute()
                    .top(px(12.0))
                    .left(px(80.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(brand_badge())
                    .child(doc_chip(doc_name.clone(), plan_area, lang, cx)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(10.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .h(px(34.0))
                            .px(px(3.0))
                            .rounded(px(10.0))
                            .bg(rgba(CHROME_BG))
                            .border_1()
                            .border_color(rgba(CHROME_BORDER))
                            .shadow_sm()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(view_chip(
                                t.s("2D", "2D"),
                                ICON_2D,
                                ViewMode::Plan,
                                view == ViewMode::Plan,
                                lang,
                                cx,
                            ))
                            .child(view_chip(
                                t.s("3D", "3D"),
                                ICON_3D,
                                ViewMode::Three,
                                view == ViewMode::Three,
                                lang,
                                cx,
                            ))
                            .child(view_chip(
                                t.s("漫游", "Walk"),
                                ICON_WALK,
                                ViewMode::Walk,
                                view == ViewMode::Walk,
                                lang,
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px(9.0))
                    .right(px(14.0))
                    .h(px(34.0))
                    .px(px(3.0))
                    .rounded(px(10.0))
                    .bg(rgba(CHROME_BG))
                    .border_1()
                    .border_color(rgba(CHROME_BORDER))
                    .shadow_sm()
                    .flex()
                    .items_center()
                    .gap(px(1.0))
                    .child(chrome_button(ICON_UNDO, false, cx, chrome_undo))
                    .child(chrome_button(ICON_REDO, false, cx, chrome_redo))
                    .child(
                        div()
                            .w(px(1.0))
                            .h(px(18.0))
                            .mx(px(5.0))
                            .bg(rgba(CHROME_BORDER)),
                    )
                    .child(save_chrome_button(dirty, cx))
                    .child(chrome_button(ICON_EXPORT, export_open, cx, chrome_export))
                    .child(chrome_button(ICON_MORE, more_open, cx, chrome_more))
                    .child(chrome_button(
                        if night { ICON_MOON } else { ICON_SUN },
                        night,
                        cx,
                        chrome_theme,
                    )),
            )
            // ------------------------------------------------------------------
            // Left icon rail: one glass column, the active tool is the only
            // filled chip. This mirrors the demo's 48px rail exactly.
            .child(if view == ViewMode::Plan {
                div()
                    .absolute()
                    .top(px(62.0))
                    .left(px(14.0))
                    .w(px(48.0))
                    .p(px(5.0))
                    .rounded(px(14.0))
                    .bg(rgba(CHROME_BG))
                    .border_1()
                    .border_color(rgba(CHROME_BORDER))
                    .shadow_sm()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(tool_button(
                        ICON_SELECT,
                        t.s("选择", "Select"),
                        Tool::Select,
                        tool == Tool::Select,
                        cx,
                    ))
                    .child(tool_button(
                        ICON_WALL,
                        t.s("画墙", "Wall"),
                        Tool::Wall,
                        tool == Tool::Wall,
                        cx,
                    ))
                    .child(tool_button(
                        ICON_DOOR,
                        t.s("门", "Door"),
                        Tool::Door,
                        tool == Tool::Door,
                        cx,
                    ))
                    .child(tool_button(
                        ICON_WINDOW,
                        t.s("窗", "Window"),
                        Tool::Window,
                        tool == Tool::Window,
                        cx,
                    ))
                    .child(library_button(
                        ICON_FURNITURE,
                        t.s("家具", "Furn."),
                        library_open,
                        tool == Tool::Furniture && furniture_armed,
                        cx,
                    ))
                    .child(tool_button(
                        ICON_MEASURE,
                        t.s("测量", "Meas."),
                        Tool::Measure,
                        tool == Tool::Measure,
                        cx,
                    ))
                    .child(rail_divider())
                    .child(tool_button(
                        ICON_DEMOLISH,
                        t.s("拆墙", "Demol."),
                        Tool::Demolish,
                        tool == Tool::Demolish,
                        cx,
                    ))
                    .child(rail_divider())
                    .child(layers_button(
                        ICON_LAYERS,
                        t.s("图层", "Layers"),
                        layers_open,
                        cx,
                    ))
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            // ------------------------------------------------------------------
            // Inspector: only visible when there is an actual selection.
            .child(
                div()
                    .absolute()
                    .top(px(62.0))
                    .right(px(14.0))
                    .children(if right_open {
                        inspector.map(|view| inspector_panel(view, wall_kind_open, cx))
                    } else {
                        None
                    }),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .children(labels.into_iter().map(|l| {
                        let size = if l.bold { 12.0 } else { 10.0 };
                        div()
                            .absolute()
                            .left(px(l.pos[0] - 40.0))
                            .top(px(l.pos[1] - 7.0))
                            .w(px(80.0))
                            .text_center()
                            .text_size(px(size))
                            .text_color(if l.bold { rgb(0x4a4f5c) } else { rgb(0x8a8f9c) })
                            .child(SharedString::from(l.text))
                    })),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(view == ViewMode::Walk, |el| {
                        el.child(
                            div()
                                .relative()
                                .w(px(15.0))
                                .h(px(15.0))
                                .child(
                                    div()
                                        .absolute()
                                        .left(px(7.0))
                                        .top(px(0.0))
                                        .w(px(1.0))
                                        .h(px(15.0))
                                        .bg(rgb(0xf2f5fa)),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .left(px(0.0))
                                        .top(px(7.0))
                                        .w(px(15.0))
                                        .h(px(1.0))
                                        .bg(rgb(0xf2f5fa)),
                                ),
                        )
                    }),
            )
            .when(view == ViewMode::Walk, |el| {
                let weak = walk_weak.clone();
                let pad_key = move |label: &'static str, direction: usize| {
                    let down_pick = weak.clone();
                    let up_pick = weak.clone();
                    div()
                        .w(px(30.0))
                        .h(px(26.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .bg(rgba(0xffffff10))
                        .border_1()
                        .border_color(rgba(0xffffff1a))
                        .text_xs()
                        .text_color(rgb(0xc7d0e0))
                        .child(label)
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = down_pick.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.state.lock().unwrap().set_walk_stick(direction, true);
                                    cx.notify();
                                });
                            }
                        })
                        .on_mouse_up(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = up_pick.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.state.lock().unwrap().set_walk_stick(direction, false);
                                    cx.notify();
                                });
                            }
                        })
                };
                el.child(
                    div()
                        .absolute()
                        .bottom(px(40.0))
                        .left(px(12.0))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x6b7385))
                                .child(t.s("移动", "Move")),
                        )
                        .child(div().flex().justify_center().child(pad_key("W", 0)))
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .child(pad_key("A", 1))
                                .child(pad_key("S", 2))
                                .child(pad_key("D", 3)),
                        ),
                )
            })
            // ------------------------------------------------------------------
            // Floating panels.
            .child(if view == ViewMode::Plan {
                library_panel(
                    library_open,
                    library_category,
                    catalog_index,
                    furniture_armed,
                    library_scroll,
                    &library_query,
                    &library_favorites,
                    favorites_only,
                    library_input.clone(),
                    lang,
                    cx,
                )
            } else {
                div().into_any_element()
            })
            .child(if view == ViewMode::Plan && layers_open {
                layers_panel(true, layer_flags, measure_count, furniture_count, lang, cx)
            } else {
                div().into_any_element()
            })
            .child(if view == ViewMode::Three && right_open {
                view3d_panel(rooms3d, cutaway, night, sun, lang, cx)
            } else {
                div().into_any_element()
            })
            // ------------------------------------------------------------------
            // Bottom-left drawing scale. The bottom bar is gone: only two small
            // floating clusters remain, one on each side of the canvas.
            .when(view == ViewMode::Plan, |el| {
                let weak = scale_weak.clone();
                let pop_weak = cx.entity().downgrade();
                el.child(
                    div()
                        .absolute()
                        .bottom(px(14.0))
                        .left(px(14.0))
                        .h(px(30.0))
                        .px(px(9.0))
                        .rounded(px(9.0))
                        .bg(if scale_open {
                            rgba(0x2328330a)
                        } else {
                            rgba(CHROME_BG)
                        })
                        .border_1()
                        .border_color(if scale_open {
                            rgba(0x23283324)
                        } else {
                            rgba(CHROME_BORDER)
                        })
                        .shadow_sm()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .hover(|style| style.bg(rgba(0x23283312)))
                        .child(ui_icon(ICON_SCALE, 13.0, PANEL_MUTED))
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(PANEL_TEXT))
                                .child(SharedString::from(format!("1:{scale_denom:.0}"))),
                        )
                        .child(ui_icon(
                            if scale_open {
                                ICON_CHEVRON_UP
                            } else {
                                ICON_CHEVRON
                            },
                            10.0,
                            PANEL_MUTED,
                        ))
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = weak.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.scale_open = !this.scale_open;
                                    this.more_open = false;
                                    this.export_open = false;
                                    cx.notify();
                                });
                            }
                        }),
                )
                .when(scale_open, |el| {
                    el.child(scale_menu(scale_denom, pop_weak, lang))
                })
            })
            // ------------------------------------------------------------------
            // Bottom-right: view, live fps, help. A compact pill pairs with the
            // scale pill instead of stretching a full-width status bar.
            .child(
                div()
                    .absolute()
                    .bottom(px(14.0))
                    .right(px(14.0))
                    .h(px(30.0))
                    .px(px(9.0))
                    .rounded(px(9.0))
                    .bg(rgba(CHROME_BG))
                    .border_1()
                    .border_color(rgba(CHROME_BORDER))
                    .shadow_sm()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(rgb(PANEL_MUTED))
                            .child(SharedString::from(view_label)),
                    )
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(rgb(PANEL_MUTED))
                            .child(SharedString::from(format!("{:.0} FPS", self.fps))),
                    )
                    .child(
                        div()
                            .w(px(22.0))
                            .h(px(22.0))
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.5))
                            .text_color(rgb(PANEL_MUTED))
                            .hover(|style| {
                                style.bg(rgba(0x23283318)).border_color(rgba(0x23283320))
                            })
                            .child(SharedString::from("?"))
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                if let Some(entity) = help_weak.upgrade() {
                                    entity.update(cx, |this, cx| {
                                        this.help_open = true;
                                        this.more_open = false;
                                        this.export_open = false;
                                        this.scale_open = false;
                                        cx.notify();
                                    });
                                }
                            }),
                    ),
            )
            // ------------------------------------------------------------------
            // Transient toast: same row as the top-right buttons, clears itself
            // after ~2.6s. Only the latest status sentence is shown.
            .when(note.is_some(), |el| {
                let msg = note.clone().unwrap_or_default();
                el.child(
                    div()
                        .absolute()
                        .top(px(13.0))
                        .right(px(220.0))
                        .h(px(28.0))
                        .max_w(px(300.0))
                        .px(px(10.0))
                        .rounded(px(8.0))
                        .bg(rgba(CHROME_BG))
                        .border_1()
                        .border_color(rgba(CHROME_BORDER))
                        .shadow_sm()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(ui_icon(ICON_CHECK, 12.0, GOOD))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(10.5))
                                .text_color(rgb(PANEL_TEXT))
                                .child(SharedString::from(msg)),
                        ),
                )
            })
            // ------------------------------------------------------------------
            // Top-right export menu.
            .when(export_open, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(46.0))
                        .right(px(76.0))
                        .w(px(208.0))
                        .rounded(px(13.0))
                        .bg(rgba(CHROME_BG))
                        .border_1()
                        .border_color(rgba(CHROME_BORDER))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(38.0))
                                .px(px(12.0))
                                .border_b_1()
                                .border_color(rgba(CHROME_BORDER))
                                .flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(wgpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(PANEL_TEXT))
                                        .child(t.s("导出", "Export")),
                                )
                                .child(
                                    div()
                                        .w(px(22.0))
                                        .h(px(22.0))
                                        .rounded(px(6.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .hover(|style| style.bg(rgba(0x2328330f)))
                                        .child(ui_icon(ICON_CLOSE, 12.0, PANEL_MUTED))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_ev, _window, cx| {
                                                if let Some(entity) = export_close_weak.upgrade() {
                                                    entity.update(cx, |this, cx| {
                                                        this.export_open = false;
                                                        cx.notify();
                                                    });
                                                }
                                            },
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .py(px(4.0))
                                .flex()
                                .flex_col()
                                .child(export_button(lang, cx))
                                .child(export_pdf_button(lang, cx))
                                .child(export_svg_button(lang, cx))
                                .child(export_bom_button(lang, cx))
                                .child(export_3d_button(lang, cx)),
                        )
                        .with_animation(
                            "export-menu-in",
                            Animation::new(Duration::from_millis(150))
                                .with_easing(ease_out_quint()),
                            |el, delta| el.opacity(delta),
                        ),
                )
            })
            // Top-right more menu: file operations and language only. Help and
            // theme intentionally live elsewhere to avoid duplicate entry points.
            .when(more_open, |el| {
                let lang_toggle = more_weak.clone();
                el.child(
                    div()
                        .absolute()
                        .top(px(46.0))
                        .right(px(14.0))
                        .w(px(226.0))
                        .rounded(px(13.0))
                        .bg(rgba(CHROME_BG))
                        .border_1()
                        .border_color(rgba(CHROME_BORDER))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(38.0))
                                .px(px(12.0))
                                .border_b_1()
                                .border_color(rgba(CHROME_BORDER))
                                .flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(wgpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(PANEL_TEXT))
                                        .child(t.s("更多", "More")),
                                )
                                .child(
                                    div()
                                        .w(px(22.0))
                                        .h(px(22.0))
                                        .rounded(px(6.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .hover(|style| style.bg(rgba(0x2328330f)))
                                        .child(ui_icon(ICON_CLOSE, 12.0, PANEL_MUTED))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_ev, _window, cx| {
                                                if let Some(entity) = more_close_weak.upgrade() {
                                                    entity.update(cx, |this, cx| {
                                                        this.more_open = false;
                                                        cx.notify();
                                                    });
                                                }
                                            },
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .py(px(4.0))
                                .flex()
                                .flex_col()
                                .child(new_button(t.s("新建方案", "New plan"), cx))
                                .child(file_button(t.s("打开方案", "Open"), cx, true))
                                .child(import_button(t.s("导入底图", "Import"), cx))
                                .child(file_button(t.s("另存为", "Save as"), cx, false))
                                .child(
                                    div()
                                        .h(px(1.0))
                                        .mx(px(6.0))
                                        .my(px(4.0))
                                        .bg(rgba(CHROME_BORDER)),
                                )
                                .child(
                                    div()
                                        .h(px(33.0))
                                        .px(px(11.0))
                                        .flex()
                                        .items_center()
                                        .gap(px(9.0))
                                        .text_size(px(10.5))
                                        .text_color(rgb(PANEL_TEXT))
                                        .hover(|style| style.bg(rgba(0x2328331c)))
                                        .child(ui_icon(ICON_GLOBE, 14.0, PANEL_MUTED))
                                        .child(t.s("语言", "Language"))
                                        .child(
                                            div()
                                                .ml_auto()
                                                .text_size(px(8.5))
                                                .text_color(rgb(PANEL_MUTED))
                                                .child(SharedString::from(lang.tag())),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_ev, _window, cx| {
                                                if let Some(entity) = lang_toggle.upgrade() {
                                                    entity.update(cx, |this, cx| {
                                                        let next = {
                                                            let mut s = this.state.lock().unwrap();
                                                            s.lang = s.lang.toggled();
                                                            s.lang
                                                        };
                                                        i18n::save_lang(next);
                                                        cx.notify();
                                                    });
                                                }
                                            },
                                        ),
                                ),
                        )
                        .with_animation(
                            "more-menu-in",
                            Animation::new(Duration::from_millis(150))
                                .with_easing(ease_out_quint()),
                            |el, delta| el.opacity(delta),
                        ),
                )
            })
            // ------------------------------------------------------------------
            .when(renaming.is_some(), |el| {
                el.child(rename_modal(
                    rename_input.clone(),
                    rename_weak.clone(),
                    lang,
                ))
            })
            .when(project_open, |el| {
                el.child(project_modal(
                    project_inputs.clone(),
                    rename_weak.clone(),
                    lang,
                ))
            })
            .when(inspector_edit.is_some(), |el| {
                el.child(inspector_edit_modal(
                    inspector_input.clone(),
                    inspector_edit.unwrap(),
                    rename_weak.clone(),
                    lang,
                ))
            })
            .when(import_open, |el| {
                el.child(import_modal(
                    import_input.clone(),
                    import_name.clone(),
                    import_weak.clone(),
                    lang,
                ))
            })
            .when(self.new_menu_open, |el| {
                el.child(new_project_modal(
                    lang,
                    self.recent_plans.clone(),
                    cx.entity().downgrade(),
                    self.state.lock().unwrap().saved_path.clone(),
                ))
            })
            .when(help_open, |el| el.child(help_modal(lang, cx)))
            // ------------------------------------------------------------------
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev, _, cx| {
                    this.down(ev);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, ev, _, cx| {
                    this.down(ev);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev, _, cx| this.moved(ev, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev, _, cx| {
                    this.up(ev);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, ev, _, cx| {
                    this.up(ev);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, ev, _, cx| this.wheel(ev, cx)))
            .on_key_down(cx.listener(|this, ev, window, cx| this.key(ev, window, cx)))
            .on_key_up(cx.listener(|this, ev, _, _| this.key_release(ev)))
    }
}

/// Light frosted side panels: the user asked for no black backing around the
/// floating editors, so the inspector/library/layers share this warm white tint
/// instead of the old near-opaque navy glass.
const SUN_SLIDER_LEFT: f32 = 106.0;
const SUN_SLIDER_WIDTH: f32 = 174.0;
const PANEL_BG: u32 = 0xfdfcf9eb;
const PANEL_BORDER: u32 = 0x23283322;
const PANEL_TEXT: u32 = 0x252b36;
const PANEL_MUTED: u32 = 0x5f6878;
const ROW_BG: u32 = 0x2328330d;
/// One neutral button tint that stays visible on both the paper-light 2D
/// canvas and the dark night / walk canvas. The previous mix of transparent,
/// `ffffff10` and dark fills made some rail buttons all but disappear.

const CHROME_BG: u32 = 0xfdfcf9e6;
const CHROME_BORDER: u32 = 0x23283321;
const ACCENT: u32 = 0x2f6fed;
const ACCENT_SOFT: u32 = 0x2f6fed1c;
const ACCENT_LINE: u32 = 0x2f6fed61;
const GOOD: u32 = 0x3d8a61;
const DOT_MUTED: u32 = 0x98a0ad;
const DANGER: u32 = 0xbd4a41;

// ---------------------------------------------------------------- icon kit
// The demo uses 24px line icons, so the desktop chrome draws the same shapes
// as inline SVG alpha masks. Keeping the paths here avoids shipping image
// assets and makes every icon tint with the current button state.

const ICON_SELECT: &str = r#"<path d='M5 3l14 8-6.5 1.7L9 19 5 3z'/>"#;
const ICON_WALL: &str = r#"<path d='M3 6h18v4H3zM8 10v4M3 14h18v4H3z'/>"#;
const ICON_DOOR: &str = r#"<path d='M5 4h9v16H5V4z'/><path d='M14 8l4 4'/><path d='M18 12a6 6 0 0 1-4 6' stroke-dasharray='2 3'/>"#;
const ICON_WINDOW: &str =
    r#"<rect x='4' y='6' width='16' height='12' rx='1.5'/><path d='M4 12h16M12 6v12'/>"#;
const ICON_FURNITURE: &str = r#"<path d='M4 11a3 3 0 0 1 3-3h10a3 3 0 0 1 3 3v2H4v-2z'/><path d='M4 13v5M20 13v5M6 13v4h12v-4'/>"#;
const ICON_MEASURE: &str =
    r#"<path d='M4 17L17 4l3 3L7 20l-3-3z'/><path d='M9 12l2 2M12 9l2 2M15 6l2 2'/>"#;
const ICON_DEMOLISH: &str = r#"<path d='M4 20l4-4M7 4h10v10H7z'/><path d='M9 8l6 6M15 8l-6 6'/>"#;
const ICON_LAYERS: &str =
    r#"<path d='M12 4l8 4-8 4-8-4 8-4z'/><path d='M4 12l8 4 8-4M4 16l8 4 8-4'/>"#;
const ICON_2D: &str =
    r#"<rect x='4' y='4' width='16' height='16' rx='2'/><path d='M4 10h16M10 4v16'/>"#;
const ICON_3D: &str = r#"<path d='M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z'/><path d='M12 12l8-4.5M12 12v9M12 12L4 7.5'/>"#;
const ICON_WALK: &str =
    r#"<circle cx='12' cy='5' r='2'/><path d='M12 8v6M12 14l-3 6M12 14l3 6M8 11l4-1 4 1'/>"#;
const ICON_UNDO: &str = r#"<path d='M9 7L4 12l5 5'/><path d='M4 12h9a4 4 0 0 1 0 8h-2'/>"#;
const ICON_REDO: &str = r#"<path d='M15 7l5 5-5 5'/><path d='M20 12h-9a4 4 0 0 0 0 8h2'/>"#;
const ICON_SAVE: &str = r#"<path d='M5 4h12l2 2v14H5V4z'/><path d='M8 4v6h8V4M8 20v-6h8v6'/>"#;
const ICON_EXPORT: &str = r#"<path d='M12 15V4M8 8l4-4 4 4'/><path d='M5 14v5h14v-5'/>"#;
const ICON_MORE: &str = r#"<circle cx='5' cy='12' r='1.4' stroke-width='2.8'/><circle cx='12' cy='12' r='1.4' stroke-width='2.8'/><circle cx='19' cy='12' r='1.4' stroke-width='2.8'/>"#;
const ICON_SUN: &str = r#"<circle cx='12' cy='12' r='4'/><path d='M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5L19 19M19 5l-1.5 1.5M6.5 17.5L5 19'/>"#;
const ICON_MOON: &str = r#"<path d='M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z'/>"#;
const ICON_SCALE: &str = r#"<path d='M5 19L19 5M9 5H5v4M15 19h4v-4'/>"#;
const ICON_CHEVRON: &str = r#"<path d='M4 6l4 4 4-4'/>"#;
const ICON_CHEVRON_UP: &str = r#"<path d='M4 10l4-4 4 4'/>"#;
const ICON_CLOSE: &str = r#"<path d='M6 6l12 12M18 6L6 18'/>"#;
const ICON_CHECK: &str = r#"<path d='M5 12l4 4L19 6'/>"#;
const ICON_FIT: &str = r#"<path d='M9 4H4v5M15 4h5v5M9 20H4v-5M15 20h5v-5'/>"#;
const ICON_NEW: &str = r#"<path d='M6 3h9l4 4v14H6V3z'/><path d='M15 3v4h4M12 11v6M9 14h6'/>"#;
const ICON_FOLDER: &str = r#"<path d='M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v9H3V7z'/>"#;
const ICON_IMAGE: &str = r#"<rect x='4' y='4' width='16' height='16' rx='2'/><path d='M4 15l4-4 4 4 3-3 5 5'/><circle cx='9' cy='9' r='1.3'/>"#;
const ICON_PDF: &str = r#"<path d='M6 3h9l4 4v14H6V3z'/><path d='M15 3v4h4M9 12h6M9 16h6'/>"#;
const ICON_SVG: &str = r#"<path d='M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z'/><path d='M12 12l8-4.5M12 12v9M12 12L4 7.5'/>"#;
const ICON_TABLE: &str =
    r#"<rect x='4' y='5' width='16' height='14' rx='2'/><path d='M4 10h16M4 14h16M10 5v14'/>"#;
const ICON_BOX3D: &str = r#"<path d='M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z'/><path d='M12 12l8-4.5M12 12v9M12 12L4 7.5'/>"#;
const ICON_GLOBE: &str =
    r#"<circle cx='12' cy='12' r='8'/><path d='M4 12h16M12 4c3 3 3 13 0 16M12 4c-3 3-3 13 0 16'/>"#;
const ICON_TRASH: &str = r#"<path d='M4 7h16M9 7V5h6v2M7 7l1 13h8l1-13'/>"#;
const ICON_FRONT: &str = r#"<path d='M12 4l7 4-7 4-7-4 7-4z'/><path d='M5 12l7 4 7-4'/>"#;
const ICON_BACK: &str = r#"<path d='M12 20l7-4-7-4-7 4 7 4z'/><path d='M5 12l7-4 7 4'/>"#;
const ICON_ROTATE_L: &str = r#"<path d='M8 7L4 11l4 4'/><path d='M4 11h9a5 5 0 0 1 0 10H9'/>"#;
const ICON_ROTATE_R: &str = r#"<path d='M16 7l4 4-4 4'/><path d='M20 11h-9a5 5 0 0 0 0 10h4'/>"#;
const ICON_EDIT: &str = r#"<path d='M4 20l4-1 11-11-3-3-11 11-1 4z'/><path d='M13 6l3 3'/>"#;
const ICON_INFO: &str = r#"<circle cx='12' cy='12' r='8'/><path d='M12 11v5M12 8h.01'/>"#;
const ICON_MINUS: &str = r#"<path d='M6 12h12'/>"#;
const ICON_PLUS: &str = r#"<path d='M12 6v12M6 12h12'/>"#;

fn ui_icon(body: &'static str, size: f32, color: u32) -> AnyElement {
    let data = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='#fff' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round'>{body}</svg>"
    );
    svg()
        .data(data.as_bytes())
        .size(px(size))
        .text_color(rgb(color))
        .into_any_element()
}

fn logo_mark(width: f32, height: f32, color: u32) -> AnyElement {
    svg()
        .data(include_bytes!("../../../assets/logo-mark.svg"))
        .w(px(width))
        .h(px(height))
        .text_color(rgb(color))
        .into_any_element()
}

/// The real lockup: charcoal tile + white F, exactly the app icon silhouette.
fn logo_badge(size: f32) -> AnyElement {
    div()
        .w(px(size))
        .h(px(size))
        .rounded(px(size * 0.28))
        .bg(rgb(0x141824))
        .shadow_sm()
        .flex()
        .items_center()
        .justify_center()
        .child(logo_mark(size * 0.61, size * 0.53, 0xffffff))
        .into_any_element()
}

fn tool_button(
    icon: &'static str,
    label: &'static str,
    tool: Tool,
    active: bool,
    cx: &mut Context<PlanEditor>,
) -> impl IntoElement {
    let state = cx.entity().downgrade();
    let danger = tool == Tool::Demolish;
    let fg = if active {
        if danger { DANGER } else { ACCENT }
    } else {
        PANEL_MUTED
    };
    let fill = if active {
        if danger { 0xbd4a411c } else { ACCENT_SOFT }
    } else {
        0x00000000
    };
    div()
        .w_full()
        .h(px(43.0))
        .rounded(px(9.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(3.0))
        .bg(rgba(fill))
        .border_1()
        .border_color(if active {
            if danger {
                rgba(0xbd4a4166)
            } else {
                rgba(ACCENT_LINE)
            }
        } else {
            rgba(0x00000000)
        })
        .text_color(rgb(fg))
        .hover(|style| {
            if active {
                style
            } else {
                style.bg(rgba(0x23283310)).text_color(rgb(PANEL_TEXT))
            }
        })
        .child(ui_icon(icon, 18.0, fg))
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(wgpui::FontWeight::MEDIUM)
                .child(SharedString::from(label)),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = state.upgrade() {
                entity.update(cx, |this, cx| {
                    this.state.lock().unwrap().set_tool(tool);
                    this.library_open = false;
                    this.layers_open = false;
                    this.more_open = false;
                    this.export_open = false;
                    this.scale_open = false;
                    cx.notify();
                });
            }
        })
}

/// The "家具" rail entry opens the library instead of arming an invisible
/// default item. Arming happens when the user picks a real item from the list.
fn library_button(
    icon: &'static str,
    label: &'static str,
    open: bool,
    armed: bool,
    cx: &mut Context<PlanEditor>,
) -> impl IntoElement {
    let state = cx.entity().downgrade();
    let fg = if armed || open { ACCENT } else { PANEL_MUTED };
    div()
        .w_full()
        .h(px(43.0))
        .rounded(px(9.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(3.0))
        .bg(if armed || open {
            rgba(ACCENT_SOFT)
        } else {
            rgba(0x00000000)
        })
        .border_1()
        .border_color(if armed || open {
            rgba(ACCENT_LINE)
        } else {
            rgba(0x00000000)
        })
        .text_color(rgb(fg))
        .hover(|style| {
            if open || armed {
                style
            } else {
                style.bg(rgba(0x23283310)).text_color(rgb(PANEL_TEXT))
            }
        })
        .child(ui_icon(icon, 18.0, fg))
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(wgpui::FontWeight::MEDIUM)
                .child(SharedString::from(label)),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = state.upgrade() {
                entity.update(cx, |this, cx| {
                    this.library_open = !this.library_open;
                    this.layers_open = false;
                    this.more_open = false;
                    this.export_open = false;
                    this.scale_open = false;
                    {
                        let mut s = this.state.lock().unwrap();
                        if this.library_open {
                            s.begin_furniture_browse();
                        } else if s.tool == Tool::Furniture {
                            s.set_tool(Tool::Select);
                        }
                    }
                    cx.notify();
                });
            }
        })
}

fn layers_button(
    icon: &'static str,
    label: &'static str,
    open: bool,
    cx: &mut Context<PlanEditor>,
) -> impl IntoElement {
    let state = cx.entity().downgrade();
    let fg = if open { ACCENT } else { PANEL_MUTED };
    div()
        .w_full()
        .h(px(43.0))
        .rounded(px(9.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(3.0))
        .bg(if open {
            rgba(ACCENT_SOFT)
        } else {
            rgba(0x00000000)
        })
        .border_1()
        .border_color(if open {
            rgba(ACCENT_LINE)
        } else {
            rgba(0x00000000)
        })
        .text_color(rgb(fg))
        .hover(|style| {
            if open {
                style
            } else {
                style.bg(rgba(0x23283310)).text_color(rgb(PANEL_TEXT))
            }
        })
        .child(ui_icon(icon, 18.0, fg))
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(wgpui::FontWeight::MEDIUM)
                .child(SharedString::from(label)),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = state.upgrade() {
                entity.update(cx, |this, cx| {
                    this.layers_open = !this.layers_open;
                    this.library_open = false;
                    this.more_open = false;
                    this.export_open = false;
                    this.scale_open = false;
                    cx.notify();
                });
            }
        })
}

fn rail_divider() -> impl IntoElement {
    div()
        .h(px(1.0))
        .w(px(36.0))
        .my(px(3.0))
        .bg(rgba(CHROME_BORDER))
}

fn chrome_button(
    icon: &'static str,
    active: bool,
    cx: &mut Context<PlanEditor>,
    action: fn(&mut PlanEditor, &mut Window, &mut Context<PlanEditor>),
) -> impl IntoElement {
    let state = cx.entity().downgrade();
    let fg = if active { ACCENT } else { PANEL_TEXT };
    div()
        .w(px(30.0))
        .h(px(30.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .bg(if active {
            rgba(ACCENT_SOFT)
        } else {
            rgba(0x00000000)
        })
        .border_1()
        .border_color(if active {
            rgba(ACCENT_LINE)
        } else {
            rgba(0x00000000)
        })
        .hover(|style| {
            if active {
                style.bg(rgba(0x23283314))
            } else {
                style.bg(rgba(0x23283318)).border_color(rgba(0x23283320))
            }
        })
        .child(ui_icon(icon, 17.0, fg))
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            if let Some(entity) = state.upgrade() {
                entity.update(cx, |this, cx| action(this, window, cx));
            }
        })
}

fn save_chrome_button(dirty: bool, cx: &mut Context<PlanEditor>) -> impl IntoElement {
    let state = cx.entity().downgrade();
    div()
        .relative()
        .w(px(30.0))
        .h(px(30.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .hover(|style| style.bg(rgba(0x23283318)).border_color(rgba(0x23283320)))
        .child(ui_icon(ICON_SAVE, 17.0, PANEL_TEXT))
        .child(
            div()
                .absolute()
                .top(px(5.0))
                .right(px(5.0))
                .w(px(6.0))
                .h(px(6.0))
                .rounded_full()
                .bg(if dirty { rgb(DOT_MUTED) } else { rgb(GOOD) })
                .border_1()
                .border_color(rgba(0xfdfcf9ff)),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = state.upgrade() {
                entity.update(cx, |this, cx| {
                    this.state.lock().unwrap().save_now();
                    cx.notify();
                });
            }
        })
}

fn brand_badge() -> impl IntoElement {
    div()
        .h(px(30.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .w(px(28.0))
                .h(px(28.0))
                .rounded(px(8.0))
                .bg(rgb(0x141824))
                .flex()
                .items_center()
                .justify_center()
                .shadow_sm()
                .child(logo_mark(18.0, 15.5, 0xffffff)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(wgpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(PANEL_TEXT))
                        .child(SharedString::from("Fathom")),
                )
                .child(
                    div()
                        .mt(px(1.0))
                        .text_size(px(9.0))
                        .text_color(rgb(PANEL_MUTED))
                        .child(SharedString::from("方寸")),
                ),
        )
}

fn doc_chip(
    name: String,
    area: f64,
    lang: i18n::Lang,
    cx: &mut Context<PlanEditor>,
) -> impl IntoElement {
    let weak = cx.entity().downgrade();
    let t = i18n::T(lang);
    let label = if name.trim().is_empty() {
        t.s("未命名方案", "Untitled").to_string()
    } else {
        name.trim().to_string()
    };
    let area_text = if area > 0.1 {
        format!(" · {area:.0} m²")
    } else {
        String::new()
    };
    div()
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(8.0))
        .bg(rgba(0xffffff9e))
        .border_1()
        .border_color(rgba(CHROME_BORDER))
        .flex()
        .items_center()
        .gap(px(7.0))
        .hover(|style| style.bg(rgba(0xffffffd6)))
        .child(
            div()
                .max_w(px(190.0))
                .truncate()
                .text_size(px(11.5))
                .font_weight(wgpui::FontWeight::MEDIUM)
                .text_color(rgb(PANEL_TEXT))
                .child(SharedString::from(format!("{label}{area_text}"))),
        )
        .child(ui_icon(ICON_CHEVRON, 11.0, PANEL_MUTED))
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            if let Some(entity) = weak.upgrade() {
                entity.update(cx, |this, cx| this.open_project_info(window, cx));
            }
        })
}

fn view_chip(
    label: &'static str,
    icon: &'static str,
    view: ViewMode,
    active: bool,
    lang: i18n::Lang,
    cx: &mut Context<PlanEditor>,
) -> impl IntoElement {
    let _ = lang;
    let weak = cx.entity().downgrade();
    let fg = if active { ACCENT } else { PANEL_MUTED };
    div()
        .h(px(26.0))
        .min_w(px(52.0))
        .px(px(11.0))
        .rounded(px(7.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(5.0))
        .text_size(px(11.0))
        .font_weight(wgpui::FontWeight::MEDIUM)
        .bg(if active {
            rgba(ACCENT_SOFT)
        } else {
            rgba(0x00000000)
        })
        .border_1()
        .border_color(if active {
            rgba(ACCENT_LINE)
        } else {
            rgba(0x00000000)
        })
        .text_color(rgb(fg))
        .hover(|style| {
            if active {
                style
            } else {
                style.bg(rgba(0x23283310)).text_color(rgb(PANEL_TEXT))
            }
        })
        .child(ui_icon(icon, 13.0, fg))
        .child(SharedString::from(label))
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = weak.upgrade() {
                entity.update(cx, |this, cx| {
                    this.state.lock().unwrap().set_view(view);
                    this.library_open = false;
                    this.layers_open = false;
                    this.more_open = false;
                    this.export_open = false;
                    this.scale_open = false;
                    cx.notify();
                });
            }
        })
}

fn chrome_undo(this: &mut PlanEditor, _window: &mut Window, cx: &mut Context<PlanEditor>) {
    let mut s = this.state.lock().unwrap();
    s.key("z", true, false);
    s.note = Some("已撤销上一步".into());
    drop(s);
    cx.notify();
}

fn chrome_redo(this: &mut PlanEditor, _window: &mut Window, cx: &mut Context<PlanEditor>) {
    let mut s = this.state.lock().unwrap();
    s.key("z", true, true);
    s.note = Some("已重做".into());
    drop(s);
    cx.notify();
}

fn chrome_export(this: &mut PlanEditor, _window: &mut Window, cx: &mut Context<PlanEditor>) {
    this.export_open = !this.export_open;
    this.more_open = false;
    this.scale_open = false;
    cx.notify();
}

fn chrome_more(this: &mut PlanEditor, _window: &mut Window, cx: &mut Context<PlanEditor>) {
    this.more_open = !this.more_open;
    this.export_open = false;
    this.scale_open = false;
    cx.notify();
}

fn chrome_theme(this: &mut PlanEditor, _window: &mut Window, cx: &mut Context<PlanEditor>) {
    let mut s = this.state.lock().unwrap();
    let next = !s.night;
    s.set_night(next);
    s.note = Some(
        if next {
            "已切换到夜间光照"
        } else {
            "已切换到日间光照"
        }
        .into(),
    );
    drop(s);
    cx.notify();
}

fn kbd_chip(label: &'static str) -> impl IntoElement {
    div()
        .px(px(5.0))
        .py(px(2.0))
        .rounded_md()
        .bg(rgba(0x30374312))
        .text_size(px(9.0))
        .text_color(rgb(PANEL_TEXT))
        .child(SharedString::from(label))
}

fn help_row(label: &'static str, keys: &'static str) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .items_center()
        .h(px(24.0))
        .child(
            div()
                .text_size(px(10.0))
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from(label)),
        )
        .child(kbd_chip(keys))
}

fn help_modal(lang: i18n::Lang, cx: &mut Context<PlanEditor>) -> impl IntoElement {
    let weak = cx.entity().downgrade();
    let t = i18n::T(lang);
    let left: Vec<(&'static str, &'static str)> = vec![
        (t.s("选择 / 框选", "Select / marquee"), "V"),
        (t.s("撤销", "Undo"), "⌘Z"),
        (t.s("重做", "Redo"), "⇧⌘Z"),
        (t.s("保存", "Save"), "⌘S"),
        (t.s("适配窗口", "Fit view"), "F"),
        (t.s("全屏", "Fullscreen"), "⇧F"),
        (t.s("家具库", "Furniture library"), "L"),
        (t.s("住宅信息", "Project info"), "⌘I"),
        (t.s("复制家具", "Duplicate"), "⌘D"),
    ];
    let right: Vec<(&'static str, &'static str)> = vec![
        (t.s("切换到 3D", "Switch to 3D"), "T"),
        (t.s("进入漫游", "Enter walk"), "G"),
        (t.s("3D 旋转", "3D orbit"), t.s("拖动", "drag")),
        (t.s("3D 缩放", "3D zoom"), t.s("滚轮", "wheel")),
        (t.s("漫游移动", "Walk move"), "WASD"),
        (t.s("漫游转向", "Walk look"), t.s("左键拖动", "drag LMB")),
        (t.s("开关门", "Toggle door"), "E"),
        (t.s("拆墙", "Demol."), "X"),
    ];
    let left_rows: Vec<AnyElement> = left
        .into_iter()
        .map(|(a, b)| help_row(a, b).into_any_element())
        .collect();
    let right_rows: Vec<AnyElement> = right
        .into_iter()
        .map(|(a, b)| help_row(a, b).into_any_element())
        .collect();
    div()
        .absolute()
        .inset_0()
        .bg(rgba(0x0b0d12b8))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, |_ev, _window, _cx| {})
        .child(
            div()
                .w(px(620.0))
                .rounded_lg()
                .bg(rgba(0xf7f8fbfa))
                .border_1()
                .border_color(rgba(CHROME_BORDER))
                .p(px(18.0))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(logo_badge(24.0))
                                .child(
                                    div()
                                        .text_size(px(15.0))
                                        .text_color(rgb(PANEL_TEXT))
                                        .child(t.s("快捷键与手势", "Shortcuts & gestures")),
                                ),
                        )
                        .child(
                            div()
                                .w(px(22.0))
                                .h(px(22.0))
                                .rounded_md()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.0))
                                .text_color(rgb(PANEL_MUTED))
                                .child(ui_icon(ICON_CLOSE, 12.0, PANEL_MUTED))
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    if let Some(entity) = weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.help_open = false;
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(rgb(PANEL_MUTED))
                        .child(t.s(
                            "所有操作都可以在画布上完成，快捷键只是加速。",
                            "Everything is reachable on the canvas; shortcuts only speed it up.",
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgb(PANEL_TEXT))
                                        .child(t.s("编辑", "Editing")),
                                )
                                .children(left_rows),
                        )
                        .child(
                            div()
                                .w(px(1.0))
                                .bg(rgba(CHROME_BORDER)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgb(PANEL_TEXT))
                                        .child(t.s("视图与漫游", "View & walk")),
                                )
                                .children(right_rows),
                        ),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(rgb(PANEL_MUTED))
                        .child(t.s(
                            "提示：2D 中拖动家具会遵循墙面与家具碰撞，3D 中拖动旋转视角。",
                            "Tip: furniture drag respects walls and other pieces in 2D; drag to orbit in 3D.",
                        )),
                ),
        )
}

fn scale_row(
    label: &'static str,
    denom: f64,
    current: f64,
    weak: WeakEntity<PlanEditor>,
) -> impl IntoElement {
    let active = (current - denom).abs() < 0.6;
    let pick = weak.clone();
    div()
        .w_full()
        .h(px(27.0))
        .px(px(8.0))
        .rounded_md()
        .cursor_pointer()
        .flex()
        .justify_between()
        .items_center()
        .bg(if active {
            rgba(0x2328330f)
        } else {
            rgba(0x00000000)
        })
        .text_size(px(10.0))
        .text_color(rgb(PANEL_TEXT))
        .hover(|style| style.bg(rgba(0x23283314)))
        .child(SharedString::from(label))
        .child(if active {
            ui_icon(ICON_CHECK, 10.0, ACCENT)
        } else {
            div().w(px(10.0)).h(px(10.0)).into_any_element()
        })
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = pick.upgrade() {
                entity.update(cx, |this, cx| {
                    this.state.lock().unwrap().set_scale_denominator(denom);
                    this.scale_open = false;
                    cx.notify();
                });
            }
        })
}

fn scale_menu(
    scale_denom: f64,
    weak: WeakEntity<PlanEditor>,
    lang: i18n::Lang,
) -> impl IntoElement {
    let fit_weak = weak.clone();
    let t = i18n::T(lang);
    div()
        .absolute()
        .bottom(px(52.0))
        .left(px(14.0))
        .w(px(190.0))
        .rounded(px(10.0))
        .bg(rgba(CHROME_BG))
        .border_1()
        .border_color(rgba(CHROME_BORDER))
        .shadow_sm()
        .p(px(6.0))
        .flex()
        .flex_col()
        .gap(px(1.0))
        .child(
            div()
                .h(px(28.0))
                .px(px(6.0))
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(rgb(PANEL_MUTED))
                        .child(t.s("图纸比例", "Drawing scale")),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(rgb(PANEL_MUTED))
                        .child(SharedString::from(format!("1:{scale_denom:.0}"))),
                ),
        )
        .child(scale_row("1:50", 50.0, scale_denom, weak.clone()))
        .child(scale_row("1:60", 60.0, scale_denom, weak.clone()))
        .child(scale_row("1:100", 100.0, scale_denom, weak.clone()))
        .child(scale_row("1:200", 200.0, scale_denom, weak.clone()))
        .child(
            div()
                .h(px(1.0))
                .mx(px(6.0))
                .my(px(2.0))
                .bg(rgba(CHROME_BORDER)),
        )
        .child(
            div()
                .w_full()
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .gap(px(7.0))
                .text_size(px(10.0))
                .text_color(rgb(PANEL_TEXT))
                .hover(|style| style.bg(rgba(0x2328331c)))
                .child(ui_icon(ICON_FIT, 12.0, PANEL_MUTED))
                .child(t.s("适应窗口", "Fit window"))
                .child(
                    div()
                        .ml_auto()
                        .text_size(px(9.0))
                        .text_color(rgb(PANEL_MUTED))
                        .child(SharedString::from("F")),
                )
                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                    if let Some(entity) = fit_weak.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.state.lock().unwrap().fit_view();
                            this.scale_open = false;
                            cx.notify();
                        });
                    }
                }),
        )
}

fn inspector_icon(kind: InspectorKind) -> &'static str {
    match kind {
        InspectorKind::Furniture => ICON_FURNITURE,
        InspectorKind::Wall => ICON_WALL,
        InspectorKind::OpeningDoor => ICON_DOOR,
        InspectorKind::OpeningWindow => ICON_WINDOW,
        InspectorKind::Room => ICON_LAYERS,
        InspectorKind::Multi => ICON_LAYERS,
    }
}

fn inspector_action_icon(action: &InspectorAction) -> &'static str {
    match action {
        InspectorAction::RemoveWall
        | InspectorAction::RemoveOpening
        | InspectorAction::RemoveFurniture => ICON_TRASH,
        InspectorAction::CycleWallKind => ICON_WALL,
        InspectorAction::RenameRoom => ICON_EDIT,
        InspectorAction::FurnitureBringFront => ICON_FRONT,
        InspectorAction::FurnitureSendBack => ICON_BACK,
        InspectorAction::OpeningFlipSwing => ICON_ROTATE_R,
        InspectorAction::OpeningFlipHinge => ICON_ROTATE_L,
        _ => ICON_LAYERS,
    }
}

fn inspector_action_is_danger(action: &InspectorAction) -> bool {
    matches!(
        action,
        InspectorAction::RemoveWall
            | InspectorAction::RemoveOpening
            | InspectorAction::RemoveFurniture
    )
}

fn hex_color(hex: &str) -> u32 {
    u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0x8f9299)
}

fn inspector_field(field: scene::InspectorField, cx: &mut Context<PlanEditor>) -> AnyElement {
    let edit = field.edit;
    let editable = edit.is_some();
    let state = cx.entity().downgrade();
    let value: SharedString = field.value.into();
    let unit: SharedString = field.unit.into();
    let mut chip = div()
        .w(px(119.0))
        .min_h(px(31.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(7.0))
        .bg(rgba(if editable { 0x2328330d } else { 0x23283308 }))
        .border_1()
        .border_color(rgba(if editable { 0x23283316 } else { 0x2328330a }))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(8.5))
                .font_weight(wgpui::FontWeight::SEMIBOLD)
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from(field.label)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(wgpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(PANEL_TEXT))
                        .child(value),
                )
                .child(
                    div()
                        .text_size(px(8.5))
                        .text_color(rgb(PANEL_MUTED))
                        .child(unit),
                )
                .when(editable, |el| {
                    el.child(div().ml_auto().child(ui_icon(ICON_EDIT, 9.5, PANEL_MUTED)))
                }),
        );
    if let Some(edit) = edit {
        chip = chip
            .hover(|style| style.bg(rgba(ACCENT_SOFT)).border_color(rgba(ACCENT_LINE)))
            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                if let Some(entity) = state.upgrade() {
                    entity.update(cx, |this, cx| {
                        this.start_inspector_edit(edit, window, cx);
                    });
                }
            });
    }
    chip.into_any_element()
}

fn inspector_action_tile(
    label: &'static str,
    action: InspectorAction,
    cx: &mut Context<PlanEditor>,
) -> AnyElement {
    let state = cx.entity().downgrade();
    let icon = inspector_action_icon(&action);
    let danger = inspector_action_is_danger(&action);
    let tint = if danger { DANGER } else { PANEL_TEXT };
    let icon_tint = if danger { DANGER } else { PANEL_MUTED };
    let label: SharedString = label.into();
    div()
        .flex_1()
        .h(px(32.0))
        .rounded(px(7.0))
        .bg(rgba(0x2328330a))
        .border_1()
        .border_color(rgba(0x23283312))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(5.0))
        .text_size(px(9.5))
        .font_weight(wgpui::FontWeight::MEDIUM)
        .text_color(rgb(tint))
        .hover(|style| style.bg(rgba(0x23283314)))
        .child(ui_icon(icon, 12.0, icon_tint))
        .child(label)
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            if let Some(entity) = state.upgrade() {
                entity.update(cx, |this, cx| {
                    if action == InspectorAction::RenameRoom {
                        this.start_rename(window, cx);
                    } else {
                        this.state.lock().unwrap().apply_action(action);
                        cx.notify();
                    }
                });
            }
        })
        .into_any_element()
}

fn inspector_panel(
    view: InspectorView,
    wall_kind_open: bool,
    cx: &mut Context<PlanEditor>,
) -> impl IntoElement {
    let t = i18n::T(i18n::load_lang());
    let close_weak = cx.entity().downgrade();
    let icon = inspector_icon(view.kind);
    let wall_kind = view.wall_kind;
    let mut body: Vec<AnyElement> = Vec::new();

    // Wall construction type: a compact dropdown instead of a cycling button.
    if let Some(current) = wall_kind {
        let toggle_weak = cx.entity().downgrade();
        let current_label = match current {
            WallKind::Bearing => t.s("承重墙", "Bearing wall"),
            WallKind::Exterior => t.s("外墙", "Exterior wall"),
            WallKind::Partition => t.s("隔墙", "Partition"),
            WallKind::Low => t.s("矮墙", "Low wall"),
        };
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(8.5))
                    .font_weight(wgpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(PANEL_MUTED))
                    .child(t.s("墙体类型 · TYPE", "Wall type · TYPE")),
            )
            .child(
                div()
                    .h(px(31.0))
                    .px(px(9.0))
                    .rounded(px(7.0))
                    .bg(rgba(0x23283308))
                    .border_1()
                    .border_color(rgba(if wall_kind_open {
                        ACCENT_LINE
                    } else {
                        0x23283312
                    }))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(10.5))
                    .text_color(rgb(PANEL_TEXT))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(0x23283312)))
                    .child(SharedString::from(current_label))
                    .child(ui_icon(
                        if wall_kind_open {
                            ICON_CHEVRON_UP
                        } else {
                            ICON_CHEVRON
                        },
                        11.0,
                        PANEL_MUTED,
                    ))
                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                        if let Some(entity) = toggle_weak.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.wall_kind_open = !this.wall_kind_open;
                                cx.notify();
                            });
                        }
                    }),
            );
        if wall_kind_open {
            let choices = [
                (WallKind::Bearing, t.s("承重墙", "Bearing wall")),
                (WallKind::Exterior, t.s("外墙", "Exterior wall")),
                (WallKind::Partition, t.s("隔墙", "Partition")),
                (WallKind::Low, t.s("矮墙", "Low wall")),
            ];
            let rows: Vec<AnyElement> = choices
                .into_iter()
                .map(|(kind, label)| {
                    let weak = cx.entity().downgrade();
                    let active = kind == current;
                    div()
                        .h(px(27.0))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .bg(rgba(if active { 0x2328330d } else { 0x00000000 }))
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(10.0))
                        .text_color(rgb(PANEL_TEXT))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(0x23283314)))
                        .child(SharedString::from(label))
                        .child(if active {
                            ui_icon(ICON_CHECK, 10.0, ACCENT).into_any_element()
                        } else {
                            div().into_any_element()
                        })
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = weak.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.state
                                        .lock()
                                        .unwrap()
                                        .apply_action(InspectorAction::SetWallKind(kind));
                                    this.wall_kind_open = false;
                                    cx.notify();
                                });
                            }
                        })
                        .into_any_element()
                })
                .collect();
            section = section.child(
                div()
                    .p(px(3.0))
                    .rounded(px(7.0))
                    .bg(rgba(0xf6f4eecc))
                    .border_1()
                    .border_color(rgba(0x23283312))
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .children(rows),
            );
        }
        body.push(section.into_any_element());
    }

    // Field groups, laid out exactly like the design demo: tiny uppercase
    // label, then two soft numeric cells per row.
    let mut groups: Vec<(String, Vec<scene::InspectorField>)> = Vec::new();
    for field in view.fields {
        if let Some((group, fields)) = groups.last_mut() {
            if *group == field.group {
                fields.push(field);
                continue;
            }
        }
        groups.push((field.group.clone(), vec![field]));
    }
    for (group, fields) in groups {
        let field_elems: Vec<AnyElement> = fields
            .into_iter()
            .map(|field| inspector_field(field, cx))
            .collect();
        body.push(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(8.5))
                        .font_weight(wgpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(PANEL_MUTED))
                        .child(SharedString::from(group)),
                )
                .child(div().flex().flex_wrap().gap(px(6.0)).children(field_elems))
                .into_any_element(),
        );
    }

    if let Some(rotation) = view.rotation {
        let state = cx.entity().downgrade();
        let left_state = state.clone();
        let plus_state = state;
        body.push(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(8.5))
                                .font_weight(wgpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(PANEL_MUTED))
                                .child(t.s("旋转", "Rotation")),
                        )
                        .child(
                            div()
                                .text_size(px(8.5))
                                .text_color(rgb(PANEL_MUTED))
                                .child(SharedString::from("±15°")),
                        ),
                )
                .child(
                    div()
                        .h(px(31.0))
                        .px(px(4.0))
                        .rounded(px(7.0))
                        .bg(rgba(0x2328330a))
                        .border_1()
                        .border_color(rgba(0x23283312))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .w(px(24.0))
                                .h(px(23.0))
                                .rounded(px(5.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|style| style.bg(rgba(0x23283314)))
                                .child(ui_icon(ICON_MINUS, 12.0, PANEL_MUTED))
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    if let Some(entity) = left_state.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.state.lock().unwrap().apply_action(
                                                InspectorAction::FurnitureRotateFine(-15),
                                            );
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_center()
                                .text_size(px(11.0))
                                .font_weight(wgpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(PANEL_TEXT))
                                .child(SharedString::from(format!("{rotation:.0}°"))),
                        )
                        .child(
                            div()
                                .w(px(24.0))
                                .h(px(23.0))
                                .rounded(px(5.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|style| style.bg(rgba(0x23283314)))
                                .child(ui_icon(ICON_PLUS, 12.0, PANEL_MUTED))
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    if let Some(entity) = plus_state.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.state.lock().unwrap().apply_action(
                                                InspectorAction::FurnitureRotateFine(15),
                                            );
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                )
                .into_any_element(),
        );
    }

    if !view.swatches.is_empty() {
        let swatch_elems: Vec<AnyElement> =
            view.swatches
                .into_iter()
                .map(|(name, hex, active)| {
                    let state = cx.entity().downgrade();
                    div()
                        .w(px(39.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .w(px(26.0))
                                .h(px(26.0))
                                .p(px(2.0))
                                .rounded_full()
                                .border_1()
                                .border_color(rgba(if active { ACCENT } else { 0x00000000 }))
                                .child(
                                    div()
                                        .w_full()
                                        .h_full()
                                        .rounded_full()
                                        .bg(rgb(hex_color(hex)))
                                        .border_1()
                                        .border_color(rgba(0x2328331f)),
                                )
                                .hover(|style| style.border_color(rgba(ACCENT_LINE)))
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    if let Some(entity) = state.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.state.lock().unwrap().apply_action(
                                                InspectorAction::FurnitureSetColor(hex),
                                            );
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(8.5))
                                .text_color(if active {
                                    rgb(ACCENT)
                                } else {
                                    rgb(PANEL_MUTED)
                                })
                                .child(SharedString::from(name)),
                        )
                        .into_any_element()
                })
                .collect();
        body.push(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(8.5))
                        .font_weight(wgpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(PANEL_MUTED))
                        .child("颜色 · COLOR"),
                )
                .child(div().flex().gap(px(4.0)).children(swatch_elems))
                .into_any_element(),
        );
    }

    if !view.material_options.is_empty() {
        let options: Vec<AnyElement> = view
            .material_options
            .into_iter()
            .map(|(name, id, active)| {
                let state = cx.entity().downgrade();
                div()
                    .w(px(119.0))
                    .h(px(28.0))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .bg(rgba(if active { ACCENT_SOFT } else { 0x23283308 }))
                    .border_1()
                    .border_color(rgba(if active { ACCENT_LINE } else { 0x2328330f }))
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .text_size(px(9.5))
                    .text_color(if active { rgb(ACCENT) } else { rgb(PANEL_TEXT) })
                    .child(
                        div()
                            .w(px(13.0))
                            .h(px(13.0))
                            .rounded_full()
                            .bg(rgba(if active { ACCENT } else { 0x23283318 }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(active, |el| el.child(ui_icon(ICON_CHECK, 8.0, 0xffffff))),
                    )
                    .child(div().truncate().child(SharedString::from(name)))
                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                        if let Some(entity) = state.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.state
                                    .lock()
                                    .unwrap()
                                    .apply_action(InspectorAction::SetMaterial(id));
                                cx.notify();
                            });
                        }
                    })
                    .into_any_element()
            })
            .collect();
        body.push(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(8.5))
                        .font_weight(wgpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(PANEL_MUTED))
                        .child("地面材质 · FLOORING"),
                )
                .child(div().flex().flex_wrap().gap(px(6.0)).children(options))
                .into_any_element(),
        );
    }

    if !view.lines.is_empty() {
        let notes: Vec<AnyElement> = view
            .lines
            .into_iter()
            .map(|line| {
                div()
                    .text_size(px(8.5))
                    .text_color(rgb(PANEL_MUTED))
                    .child(SharedString::from(line))
                    .into_any_element()
            })
            .collect();
        body.push(
            div()
                .rounded(px(7.0))
                .bg(rgba(0x23283308))
                .border_1()
                .border_color(rgba(0x2328330d))
                .p(px(7.0))
                .flex()
                .items_start()
                .gap(px(6.0))
                .child(ui_icon(ICON_INFO, 12.0, PANEL_MUTED))
                .child(div().flex().flex_col().gap(px(2.0)).children(notes))
                .into_any_element(),
        );
    }

    // Three actions per row keeps every tile readable, including the six-way
    // alignment set for multi-selection.
    for chunk in view.actions.chunks(3) {
        let row: Vec<AnyElement> = chunk
            .iter()
            .map(|(label, action)| inspector_action_tile(*label, action.clone(), cx))
            .collect();
        body.push(div().flex().gap(px(6.0)).children(row).into_any_element());
    }

    div()
        .w(px(272.0))
        .rounded(px(14.0))
        .bg(rgba(PANEL_BG))
        .shadow_lg()
        .border_1()
        .border_color(rgba(PANEL_BORDER))
        .text_color(rgb(PANEL_TEXT))
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .h(px(44.0))
                .px(px(12.0))
                .border_b_1()
                .border_color(rgba(PANEL_BORDER))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(26.0))
                        .h(px(26.0))
                        .rounded(px(7.0))
                        .bg(rgba(ACCENT_SOFT))
                        .border_1()
                        .border_color(rgba(ACCENT_LINE))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui_icon(icon, 14.0, ACCENT)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(wgpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(PANEL_TEXT))
                                .truncate()
                                .child(SharedString::from(view.title)),
                        )
                        .child(
                            div()
                                .text_size(px(9.0))
                                .text_color(rgb(PANEL_MUTED))
                                .truncate()
                                .child(SharedString::from(view.subtitle)),
                        ),
                )
                .child(
                    div()
                        .w(px(24.0))
                        .h(px(24.0))
                        .rounded(px(6.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|style| style.bg(rgba(0x2328330f)))
                        .child(ui_icon(ICON_CLOSE, 12.0, PANEL_MUTED))
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = close_weak.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.right_open = false;
                                    cx.notify();
                                });
                            }
                        }),
                ),
        )
        .child(
            div()
                .px(px(12.0))
                .pt(px(11.0))
                .pb(px(13.0))
                .flex()
                .flex_col()
                .gap(px(11.0))
                .children(body),
        )
}

fn export_button(lang: i18n::Lang, cx: &mut Context<PlanEditor>) -> impl IntoElement {
    let state = cx.entity().downgrade();
    div()
        .w_full()
        .h(px(33.0))
        .px(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(9.0))
        .text_size(px(10.5))
        .text_color(rgb(PANEL_TEXT))
        .hover(|style| style.bg(rgba(0x2328331c)))
        .child(ui_icon(ICON_IMAGE, 14.0, PANEL_MUTED))
        .child(SharedString::from(i18n::T(lang).s("导出 PNG", "PNG")))
        .child(
            div()
                .ml_auto()
                .text_size(px(8.5))
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from("2×")),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            let picked = rfd::FileDialog::new()
                .add_filter("PNG", &["png"])
                .set_file_name("floorplan.png")
                .save_file();
            let Some(path) = picked else {
                return;
            };
            let Some(entity) = state.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                let outcome = {
                    let s = this.state.lock().unwrap();
                    export::export_png(&s, &path)
                };
                this.state.lock().unwrap().note = Some(match outcome {
                    Ok(msg) => msg,
                    Err(err) => format!("导出失败 · {err}"),
                });
                cx.notify();
            });
        })
}

fn export_bom_button(lang: i18n::Lang, cx: &mut Context<PlanEditor>) -> impl IntoElement {
    let state = cx.entity().downgrade();
    div()
        .w_full()
        .h(px(33.0))
        .px(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(9.0))
        .text_size(px(10.5))
        .text_color(rgb(PANEL_TEXT))
        .hover(|style| style.bg(rgba(0x2328331c)))
        .child(ui_icon(ICON_TABLE, 14.0, PANEL_MUTED))
        .child(SharedString::from(i18n::T(lang).s("导出清单", "BOM")))
        .child(
            div()
                .ml_auto()
                .text_size(px(8.5))
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from(".csv")),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            let picked = rfd::FileDialog::new()
                .add_filter("CSV", &["csv"])
                .set_file_name("quote.csv")
                .save_file();
            let Some(path) = picked else {
                return;
            };
            let Some(entity) = state.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                let outcome = {
                    let s = this.state.lock().unwrap();
                    export::export_bom(&s, &path)
                };
                this.state.lock().unwrap().note = Some(match outcome {
                    Ok(msg) => msg,
                    Err(err) => format!("导出失败 · {err}"),
                });
                cx.notify();
            });
        })
}

fn export_3d_button(lang: i18n::Lang, cx: &mut Context<PlanEditor>) -> impl IntoElement {
    let state = cx.entity().downgrade();
    div()
        .w_full()
        .h(px(33.0))
        .px(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(9.0))
        .text_size(px(10.5))
        .text_color(rgb(PANEL_TEXT))
        .hover(|style| style.bg(rgba(0x2328331c)))
        .child(ui_icon(ICON_BOX3D, 14.0, PANEL_MUTED))
        .child(SharedString::from(i18n::T(lang).s("3D 截图", "3D")))
        .child(
            div()
                .ml_auto()
                .text_size(px(8.5))
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from(".png")),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            let picked = rfd::FileDialog::new()
                .add_filter("PNG", &["png"])
                .set_file_name("render.png")
                .save_file();
            let Some(path) = picked else {
                return;
            };
            let Some(entity) = state.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                let outcome = {
                    let s = this.state.lock().unwrap();
                    export::export_png_3d(&s, &path)
                };
                this.state.lock().unwrap().note = Some(match outcome {
                    Ok(msg) => msg,
                    Err(err) => format!("导出失败 · {err}"),
                });
                cx.notify();
            });
        })
}

fn export_pdf_button(lang: i18n::Lang, cx: &mut Context<PlanEditor>) -> impl IntoElement {
    let state = cx.entity().downgrade();
    div()
        .w_full()
        .h(px(33.0))
        .px(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(9.0))
        .text_size(px(10.5))
        .text_color(rgb(PANEL_TEXT))
        .hover(|style| style.bg(rgba(0x2328331c)))
        .child(ui_icon(ICON_PDF, 14.0, PANEL_MUTED))
        .child(SharedString::from(i18n::T(lang).s("PDF", "PDF")))
        .child(
            div()
                .ml_auto()
                .text_size(px(8.5))
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from("⌘P")),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            let picked = rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .set_file_name("fathom-plan.pdf")
                .save_file();
            let Some(path) = picked else {
                return;
            };
            let Some(entity) = state.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                let outcome = {
                    let s = this.state.lock().unwrap();
                    export::export_pdf(&s, &path)
                };
                this.state.lock().unwrap().note = Some(match outcome {
                    Ok(msg) => format!("{msg} · ⌘P 直接打印"),
                    Err(err) => format!("PDF 导出失败 · {err}"),
                });
                cx.notify();
            });
        })
}

fn export_svg_button(lang: i18n::Lang, cx: &mut Context<PlanEditor>) -> impl IntoElement {
    let state = cx.entity().downgrade();
    div()
        .w_full()
        .h(px(33.0))
        .px(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(9.0))
        .text_size(px(10.5))
        .text_color(rgb(PANEL_TEXT))
        .hover(|style| style.bg(rgba(0x2328331c)))
        .child(ui_icon(ICON_SVG, 14.0, PANEL_MUTED))
        .child(SharedString::from(i18n::T(lang).s("导出 SVG", "SVG")))
        .child(
            div()
                .ml_auto()
                .text_size(px(8.5))
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from(".svg")),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            let picked = rfd::FileDialog::new()
                .add_filter("SVG", &["svg"])
                .set_file_name("plan.svg")
                .save_file();
            let Some(path) = picked else {
                return;
            };
            let Some(entity) = state.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                let outcome = {
                    let s = this.state.lock().unwrap();
                    export::export_svg(&s, &path)
                };
                this.state.lock().unwrap().note = Some(match outcome {
                    Ok(msg) => msg,
                    Err(err) => format!("导出失败 · {err}"),
                });
                cx.notify();
            });
        })
}

fn new_button(label: &'static str, cx: &mut Context<PlanEditor>) -> impl IntoElement {
    let state = cx.entity().downgrade();
    div()
        .w_full()
        .h(px(33.0))
        .px(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(9.0))
        .text_size(px(10.5))
        .text_color(rgb(PANEL_TEXT))
        .hover(|style| style.bg(rgba(0x2328331c)))
        .child(ui_icon(ICON_NEW, 14.0, PANEL_MUTED))
        .child(SharedString::from(label))
        .child(
            div()
                .ml_auto()
                .text_size(px(8.5))
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from("⌘N")),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = state.upgrade() {
                entity.update(cx, |this, cx| this.open_new_menu(cx));
            }
        })
}

fn import_button(label: &'static str, cx: &mut Context<PlanEditor>) -> impl IntoElement {
    let state = cx.entity().downgrade();
    div()
        .w_full()
        .h(px(33.0))
        .px(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(9.0))
        .text_size(px(10.5))
        .text_color(rgb(PANEL_TEXT))
        .hover(|style| style.bg(rgba(0x2328331c)))
        .child(ui_icon(ICON_IMAGE, 14.0, PANEL_MUTED))
        .child(SharedString::from(label))
        .child(
            div()
                .ml_auto()
                .text_size(px(8.5))
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from("⌘I")),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            if let Some(entity) = state.upgrade() {
                entity.update(cx, |this, cx| this.pick_import(window, cx));
            }
        })
}

fn new_project_modal(
    lang: i18n::Lang,
    recents: Vec<recent::RecentPlan>,
    weak: WeakEntity<PlanEditor>,
    current_path: PathBuf,
) -> impl IntoElement {
    let t = i18n::T(lang);
    let cancel_weak = weak.clone();
    let mut rows: Vec<AnyElement> = Vec::new();
    for pair in templates::KINDS.chunks(2) {
        let mut cards = Vec::new();
        for &kind in pair {
            cards.push(template_card(kind, lang, weak.clone()));
        }
        rows.push(div().flex().gap_2().children(cards).into_any_element());
    }
    let recent_rows: Vec<AnyElement> = recents
        .into_iter()
        .take(5)
        .map(|entry| {
            let path = entry.path.clone();
            let open_path = path.clone();
            let archive_path = path.clone();
            let forget_path = path.clone();
            let is_current = path == current_path;
            let open_weak = weak.clone();
            let archive_weak = weak.clone();
            let forget_weak = weak.clone();
            let label = SharedString::from(entry.name.clone());
            let location = SharedString::from(scene::display_path(&path));
            let actions = if is_current {
                div()
                    .px_1()
                    .rounded_md()
                    .bg(rgba(ACCENT_SOFT))
                    .text_size(px(10.0))
                    .text_color(rgb(ACCENT))
                    .child(t.s("当前", "open"))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .gap_1()
                    .child(
                        div()
                            .px_1()
                            .rounded_md()
                            .bg(rgba(ACCENT_SOFT))
                            .text_size(px(10.0))
                            .text_color(rgb(ACCENT))
                            .child(t.s("归档", "Archive"))
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                if let Some(entity) = archive_weak.upgrade() {
                                    entity.update(cx, |this, cx| {
                                        this.archive_recent(archive_path.clone(), cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .px_1()
                            .rounded_md()
                            .bg(rgba(0xbd4a4114))
                            .text_size(px(10.0))
                            .text_color(rgb(DANGER))
                            .child(ui_icon(ICON_CLOSE, 11.0, DANGER))
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                if let Some(entity) = forget_weak.upgrade() {
                                    entity.update(cx, |this, cx| {
                                        this.forget_recent(forget_path.clone(), cx);
                                    });
                                }
                            }),
                    )
                    .into_any_element()
            };
            div()
                .px_2()
                .py_2()
                .rounded_md()
                .bg(rgba(0x2328330a))
                .flex()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_xs().text_color(rgb(PANEL_TEXT)).child(label))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgb(PANEL_MUTED))
                                .child(location),
                        )
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = open_weak.upgrade() {
                                entity
                                    .update(cx, |this, cx| this.open_recent(open_path.clone(), cx));
                            }
                        }),
                )
                .child(actions)
                .into_any_element()
        })
        .collect();

    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x0b0d1240))
        .on_mouse_down(MouseButton::Left, |_ev, _window, _cx| {})
        .child(
            div()
                .w(px(560.0))
                .rounded(px(14.0))
                .border_1()
                .border_color(rgba(CHROME_BORDER))
                .shadow_lg()
                .bg(rgba(0xfdfcf9f7))
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .items_end()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(logo_badge(24.0))
                                        .child(
                                            div()
                                                .text_size(px(16.0))
                                                .text_color(rgb(PANEL_TEXT))
                                                .child(t.s("新建方案", "New plan")),
                                        ),
                                )
                                .child(div().text_xs().text_color(rgb(PANEL_MUTED)).child(t.s(
                                    "从空白、常用户型或示例开始",
                                    "Start blank, from a template, or the demo",
                                ))),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0x2328330a))
                                .text_xs()
                                .text_color(rgb(PANEL_MUTED))
                                .child(SharedString::from("⌘N")),
                        ),
                )
                .children(rows)
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(PANEL_MUTED))
                        .child(t.s("最近方案", "Recent plans")),
                )
                .when(recent_rows.is_empty(), |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x6b7385))
                            .child(t.s("还没有最近方案", "No recent plans yet")),
                    )
                })
                .children(recent_rows)
                .child(
                    div().flex().justify_end().gap_2().child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgba(0x2328330a))
                            .text_xs()
                            .child(t.s("取消 (Esc)", "Cancel (Esc)"))
                            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                if let Some(entity) = cancel_weak.upgrade() {
                                    entity.update(cx, |this, cx| {
                                        this.new_menu_open = false;
                                        window.focus(&this.focus, cx);
                                        cx.notify();
                                    });
                                }
                            }),
                    ),
                ),
        )
}

fn template_card(
    kind: templates::TemplateKind,
    lang: i18n::Lang,
    weak: WeakEntity<PlanEditor>,
) -> AnyElement {
    let title = kind.title(lang);
    let note = kind.note(lang);
    div()
        .w(px(252.0))
        .p_3()
        .rounded_md()
        .bg(rgba(0x2328330a))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_sm()
                .text_color(rgb(PANEL_TEXT))
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from(note)),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = weak.upgrade() {
                entity.update(cx, |this, cx| this.create_new(kind, cx));
            }
        })
        .into_any_element()
}

fn file_button(label: &'static str, cx: &mut Context<PlanEditor>, open: bool) -> impl IntoElement {
    let state = cx.entity().downgrade();
    div()
        .w_full()
        .h(px(33.0))
        .px(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(9.0))
        .text_size(px(10.5))
        .text_color(rgb(PANEL_TEXT))
        .hover(|style| style.bg(rgba(0x2328331c)))
        .child(ui_icon(ICON_FOLDER, 14.0, PANEL_MUTED))
        .child(SharedString::from(label))
        .child(
            div()
                .ml_auto()
                .text_size(px(8.5))
                .text_color(rgb(PANEL_MUTED))
                .child(SharedString::from(if open { "⌘O" } else { "⌘S" })),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = state.upgrade() {
                entity.update(cx, |this, cx| {
                    this.more_open = false;
                    this.export_open = false;
                    this.scale_open = false;
                    cx.notify();
                });
            }
            let dialog = rfd::FileDialog::new().add_filter("Floorplan JSON", &["json"]);
            let picked = if open {
                dialog.pick_file()
            } else {
                dialog.set_file_name("plan.json").save_file()
            };
            let Some(path) = picked else {
                return;
            };
            let Some(entity) = state.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                {
                    let mut s = this.state.lock().unwrap();
                    if open {
                        s.open_path(path);
                    } else {
                        s.save_as(path);
                    }
                }
                cx.notify();
            });
        })
}

// --------------------------------------------------------------- render loop

fn render_loop(
    surface: WgpuSurfaceHandle,
    state: Arc<Mutex<SceneState>>,
    fps_tx: mpsc::Sender<f64>,
) {
    let device = surface.device().clone();
    let queue = surface.queue().clone();
    let format = surface.format();

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("plan shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("plan bgl"),
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
    let vp_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("plan viewport"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &vp_buf,
        0,
        bytemuck::bytes_of(&[VIEW_W as f32, VIEW_H as f32, 0.0, 0.0]),
    );
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("plan bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: vp_buf.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("plan layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("plan pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
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
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("plan vertices"),
        size: (MAX_VERTICES * std::mem::size_of::<Vertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut renderer3d = Renderer3d::new(&device, &queue, format, VIEW_W, VIEW_H);
    let mut underlay_renderer = underlay_gpu::UnderlayRenderer::new(&device, format);

    let mut frames = 0u32;
    let mut last = Instant::now();
    let mut tick = Instant::now();
    loop {
        if surface.is_closed() {
            break;
        }
        surface.wait_for_present();
        let Some((view, size)) = surface.back_view_with_size() else {
            thread::sleep(Duration::from_millis(2));
            continue;
        };

        #[allow(clippy::large_enum_variant)]
        enum Frame {
            Plan(Batch, [f32; 2], Option<underlay::UnderlayDraw>),
            Three(
                std::sync::Arc<fp_render3d::MeshParts>,
                u64,
                Mat4,
                [f32; 3],
                render3d::Look,
            ),
        }
        let frame = {
            let mut s = state.lock().unwrap();
            let dt = tick.elapsed().as_secs_f32();
            tick = Instant::now();
            s.walk_step(dt);
            s.clear_dirty();
            match s.view {
                ViewMode::Plan => {
                    let batch = s.build_batch();
                    let underlay = s.underlay_draw();
                    Frame::Plan(batch, s.view_size, underlay)
                }
                ViewMode::Three => {
                    let eye = s.cam3.eye();
                    let aspect = s.view_size[0] / s.view_size[1].max(1.0);
                    let (epoch, parts) = s.mesh3d_parts(false);
                    Frame::Three(
                        parts,
                        epoch,
                        s.cam3.view_proj(aspect),
                        [eye.x, eye.y, eye.z],
                        s.look3d(),
                    )
                }
                ViewMode::Walk => {
                    let eye = s.walk.pos;
                    let aspect = s.view_size[0] / s.view_size[1].max(1.0);
                    let (epoch, parts) = s.mesh3d_parts(true);
                    Frame::Three(
                        parts,
                        epoch,
                        s.walk.view_proj(aspect),
                        [eye.x, eye.y, eye.z],
                        s.look3d(),
                    )
                }
            }
        };

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("plan enc"),
        });
        match frame {
            Frame::Plan(batch, viewport, underlay) => {
                queue.write_buffer(
                    &vp_buf,
                    0,
                    bytemuck::bytes_of(&[viewport[0], viewport[1], 0.0, 0.0]),
                );
                let count = batch.vertices.len().min(MAX_VERTICES) as u32;
                if count > 0 {
                    queue.write_buffer(
                        &vbuf,
                        0,
                        bytemuck::cast_slice(&batch.vertices[..count as usize]),
                    );
                }
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("plan pass"),
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
                if let Some(image) = &underlay {
                    underlay_renderer.draw(&device, &queue, &mut pass, image, viewport);
                }
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.draw(0..count, 0..1);
            }
            Frame::Three(mesh, epoch, vp, eye, look) => {
                renderer3d.ensure_depth(&device, size);
                renderer3d.draw(
                    &queue,
                    &mut enc,
                    &view,
                    &mesh.vertices,
                    epoch,
                    render3d::Scene3dView {
                        view_proj: vp,
                        eye,
                        look,
                    },
                );
            }
        }
        queue.submit(Some(enc.finish()));
        surface.present();

        frames += 1;
        if last.elapsed() >= Duration::from_secs(1) {
            let _ = fps_tx.send(frames as f64 / last.elapsed().as_secs_f64());
            frames = 0;
            last = Instant::now();
        }
        thread::sleep(Duration::from_millis(4));
    }
}

fn view3d_panel(
    rooms: Vec<(u128, String, f64)>,
    cutaway: bool,
    night: bool,
    sun: f32,
    lang: i18n::Lang,
    cx: &mut Context<PlanEditor>,
) -> AnyElement {
    let t = i18n::T(lang);
    let weak = cx.entity().downgrade();
    let presets: [(&str, Preset3d); 3] = [
        (t.s("俯视", "Top"), Preset3d::Top),
        (t.s("鸟瞰", "Bird"), Preset3d::Bird),
        (t.s("斜视", "Oblique"), Preset3d::Oblique),
    ];
    let mut buttons: Vec<AnyElement> = Vec::new();
    for (label, preset) in presets {
        let pick = weak.clone();
        buttons.push(
            div()
                .flex_1()
                .px_1()
                .py_1()
                .rounded_md()
                .text_center()
                .text_xs()
                .border_1()
                .border_color(rgba(CHROME_BORDER))
                .bg(rgba(0x2328330d))
                .text_color(rgb(PANEL_MUTED))
                .child(label)
                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                    if let Some(entity) = pick.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.state.lock().unwrap().preset3d(preset);
                            cx.notify();
                        });
                    }
                })
                .into_any_element(),
        );
    }
    let cut_weak = weak.clone();
    let cut_row = div()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(if cutaway {
            rgba(ACCENT_LINE)
        } else {
            rgba(CHROME_BORDER)
        })
        .bg(if cutaway {
            rgba(ACCENT_SOFT)
        } else {
            rgba(0x2328330d)
        })
        .flex()
        .justify_between()
        .items_center()
        .child(
            div()
                .text_xs()
                .text_color(if cutaway {
                    rgb(ACCENT)
                } else {
                    rgb(PANEL_TEXT)
                })
                .child(t.s("剖切墙", "Cutaway")),
        )
        .child(
            div()
                .text_xs()
                .text_color(if cutaway {
                    rgb(ACCENT)
                } else {
                    rgb(PANEL_MUTED)
                })
                .child(if cutaway {
                    "1.35m"
                } else {
                    t.s("全高", "full")
                }),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = cut_weak.upgrade() {
                entity.update(cx, |this, cx| {
                    this.state.lock().unwrap().set_cutaway(!cutaway);
                    cx.notify();
                });
            }
        });

    let night_weak = weak.clone();
    let night_row = div()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(if night {
            rgba(ACCENT_LINE)
        } else {
            rgba(CHROME_BORDER)
        })
        .bg(if night {
            rgba(ACCENT_SOFT)
        } else {
            rgba(0x2328330d)
        })
        .flex()
        .justify_between()
        .items_center()
        .child(
            div()
                .text_xs()
                .text_color(if night { rgb(ACCENT) } else { rgb(PANEL_TEXT) })
                .child(t.s("夜景", "Night")),
        )
        .child(
            div()
                .text_xs()
                .text_color(if night { rgb(ACCENT) } else { rgb(PANEL_MUTED) })
                .child(t.s(
                    if night { "开" } else { "关" },
                    if night { "on" } else { "off" },
                )),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if let Some(entity) = night_weak.upgrade() {
                entity.update(cx, |this, cx| {
                    this.state.lock().unwrap().set_night(!night);
                    cx.notify();
                });
            }
        });

    let sun_weak = weak.clone();
    let sun_pct = sun.clamp(0.0, 1.0);
    let sun_minutes = (6 * 60 + (sun_pct * 720.0).round() as i32).clamp(360, 1080);
    let sun_clock = format!("{:02}:{:02}", sun_minutes / 60, sun_minutes % 60);
    let sun_deg = (std::f32::consts::PI * sun_pct).sin() * 62.0;
    let sun_down_weak = sun_weak.clone();
    let sun_move_weak = sun_weak.clone();
    let sun_bar = div()
        .h(px(10.0))
        .w_full()
        .rounded_full()
        .bg(rgba(0x1d223012))
        .relative()
        .child(
            div()
                .h_full()
                .w(px(174.0 * sun_pct))
                .rounded_full()
                .bg(rgb(0x2f6fed)),
        )
        .child(
            div()
                .absolute()
                .top(px(-3.0))
                .left(px((174.0 * sun_pct - 3.0).max(0.0)))
                .w(px(6.0))
                .h(px(16.0))
                .rounded_full()
                .bg(rgb(0x3a4150)),
        )
        .on_mouse_down(MouseButton::Left, move |ev, _window, cx| {
            let value =
                ((f32::from(ev.position.x) - SUN_SLIDER_LEFT) / SUN_SLIDER_WIDTH).clamp(0.0, 1.0);
            if let Some(entity) = sun_down_weak.upgrade() {
                entity.update(cx, |this, cx| {
                    this.sun_drag = true;
                    this.state.lock().unwrap().set_sun(value);
                    cx.notify();
                });
            }
        })
        // While the pointer is captured by this bar, the root only sees drag
        // events. Track the move here so the knob follows the pointer instead
        // of stopping at the initial click.
        .on_mouse_move(move |ev, _window, cx| {
            let value =
                ((f32::from(ev.position.x) - SUN_SLIDER_LEFT) / SUN_SLIDER_WIDTH).clamp(0.0, 1.0);
            if let Some(entity) = sun_move_weak.upgrade() {
                entity.update(cx, |this, cx| {
                    if this.sun_drag {
                        this.state.lock().unwrap().set_sun(value);
                        cx.notify();
                    }
                });
            }
        });

    let mut rows: Vec<AnyElement> = Vec::new();
    for (raw_id, name, area) in rooms {
        let pick = weak.clone();
        let id = fp_core::EntityId(raw_id);
        rows.push(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(rgba(ROW_BG))
                .flex()
                .justify_between()
                .gap_1()
                .child(div().text_xs().text_color(rgb(PANEL_TEXT)).child(name))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(PANEL_MUTED))
                        .child(format!("{area:.1}㎡")),
                )
                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                    if let Some(entity) = pick.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.state.lock().unwrap().fly_to_room(id);
                            cx.notify();
                        });
                    }
                })
                .into_any_element(),
        );
    }
    div()
        .absolute()
        .top(px(62.0))
        .left(px(72.0))
        .w(px(220.0))
        .rounded(px(14.0))
        .bg(rgba(PANEL_BG))
        .shadow_sm()
        .border_1()
        .border_color(rgba(PANEL_BORDER))
        .text_color(rgb(PANEL_TEXT))
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(PANEL_TEXT))
                .child(t.s("视角", "View")),
        )
        .child(div().flex().gap_1().children(buttons))
        .child(cut_row)
        .child(night_row)
        .child(
            div()
                .pt_1()
                .text_xs()
                .text_color(rgb(PANEL_MUTED))
                .child(t.f(
                    &format!("日照 {sun_clock} · {sun_deg:.0}°"),
                    &format!("Sun {sun_clock} · {sun_deg:.0}°"),
                )),
        )
        .child(sun_bar)
        .child(
            div()
                .flex()
                .justify_between()
                .pt_1()
                .text_xs()
                .text_color(rgb(PANEL_MUTED))
                .child(t.s("06:00", "06:00"))
                .child(t.s("18:00", "18:00")),
        )
        .child(
            div()
                .pt_1()
                .text_xs()
                .text_color(rgb(PANEL_MUTED))
                .child(t.s("房间 · 点击飞行", "Rooms · click to fly")),
        )
        .child(div().flex().flex_col().gap_1().children(rows))
        .into_any_element()
}

fn layers_panel(
    open: bool,
    flags: [bool; 6],
    measure_count: usize,
    furniture_count: usize,
    lang: i18n::Lang,
    cx: &mut Context<PlanEditor>,
) -> AnyElement {
    if !open {
        return div().into_any_element();
    }
    let weak = cx.entity().downgrade();
    let t = i18n::T(lang);
    let definitions: [(&str, &str, bool, Layer); 6] = [
        ("房间名", "Room names", flags[0], Layer::RoomNames),
        ("家具", "Furniture", flags[1], Layer::Furniture),
        ("网格", "Grid", flags[2], Layer::Grid),
        ("尺寸标注", "Dimensions", flags[3], Layer::Dimensions),
        ("承重墙标示", "Load bearing", flags[4], Layer::Bearing),
        ("户型底图", "Plan underlay", flags[5], Layer::Underlay),
    ];
    let mut rows: Vec<AnyElement> = Vec::new();
    for (zh, en, on, layer) in definitions {
        let toggle = weak.clone();
        rows.push(
            div()
                .h(px(27.0))
                .px(px(4.0))
                .rounded(px(6.0))
                .flex()
                .justify_between()
                .items_center()
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0x2328330a)))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(PANEL_TEXT))
                        .child(t.s(zh, en)),
                )
                .child(
                    div()
                        .relative()
                        .w(px(30.0))
                        .h(px(16.0))
                        .rounded_full()
                        .bg(rgba(if on { ACCENT } else { 0x2328331f }))
                        .child(
                            div()
                                .absolute()
                                .top(px(2.0))
                                .left(px(if on { 16.0 } else { 2.0 }))
                                .w(px(12.0))
                                .h(px(12.0))
                                .rounded_full()
                                .bg(rgb(0xfdfcf9))
                                .shadow_sm(),
                        ),
                )
                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                    if let Some(entity) = toggle.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.state.lock().unwrap().toggle_layer(layer);
                            cx.notify();
                        });
                    }
                })
                .into_any_element(),
        );
    }
    if measure_count > 0 {
        let clear = weak.clone();
        rows.push(
            div()
                .h(px(27.0))
                .px(px(4.0))
                .rounded(px(6.0))
                .flex()
                .justify_between()
                .items_center()
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0xd9534f0d)))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(0x8f3b35))
                        .child(SharedString::from(t.s("清除测量", "Clear measures"))),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(rgb(0xa0524a))
                        .child(SharedString::from(format!("({measure_count})"))),
                )
                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                    if let Some(entity) = clear.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.state.lock().unwrap().clear_measurements();
                            cx.notify();
                        });
                    }
                })
                .into_any_element(),
        );
    }
    if furniture_count > 0 {
        let clear = weak.clone();
        rows.push(
            div()
                .h(px(27.0))
                .px(px(4.0))
                .rounded(px(6.0))
                .flex()
                .justify_between()
                .items_center()
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0xd9534f0d)))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(0x8f3b35))
                        .child(SharedString::from(t.s("清空家具", "Clear furniture"))),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(rgb(0xa0524a))
                        .child(SharedString::from(format!("({furniture_count})"))),
                )
                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                    if let Some(entity) = clear.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.state.lock().unwrap().clear_furniture();
                            cx.notify();
                        });
                    }
                })
                .into_any_element(),
        );
    }
    let close = weak;
    div()
        .absolute()
        .bottom(px(104.0))
        .left(px(72.0))
        .w(px(194.0))
        .rounded(px(14.0))
        .bg(rgba(PANEL_BG))
        .shadow_sm()
        .border_1()
        .border_color(rgba(PANEL_BORDER))
        .text_color(rgb(PANEL_TEXT))
        .p(px(6.0))
        .flex()
        .flex_col()
        .gap(px(0.0))
        .child(
            div()
                .h(px(28.0))
                .px(px(4.0))
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(wgpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(PANEL_TEXT))
                        .child(t.s("图层", "Layers")),
                )
                .child(
                    div()
                        .h(px(20.0))
                        .px(px(4.0))
                        .rounded(px(4.0))
                        .flex()
                        .items_center()
                        .text_size(px(9.0))
                        .text_color(rgb(PANEL_MUTED))
                        .hover(|style| style.bg(rgba(0x2328330a)))
                        .child(t.s("收起", "Hide"))
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = close.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.layers_open = false;
                                    cx.notify();
                                });
                            }
                        }),
                ),
        )
        .children(rows)
        .into_any_element()
}

fn catalog_matches(
    spec: &fp_model::catalog::FurnitureSpec,
    query: &str,
    favorites: &[String],
    favorites_only: bool,
    category: FurnitureCategory,
) -> bool {
    if category != FurnitureCategory::All && spec.category != category {
        return false;
    }
    if favorites_only && !favorites::contains(favorites, spec.asset_id) {
        return false;
    }
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    spec.name.to_lowercase().contains(&q) || spec.asset_id.to_lowercase().contains(&q)
}

fn catalog_visible_count(
    category: FurnitureCategory,
    query: &str,
    favorites: &[String],
    favorites_only: bool,
) -> usize {
    CATALOG
        .iter()
        .filter(|spec| catalog_matches(spec, query, favorites, favorites_only, category))
        .count()
}

#[allow(clippy::too_many_arguments)]
fn library_panel(
    open: bool,
    category: FurnitureCategory,
    active_index: usize,
    armed: bool,
    scroll: usize,
    query: &str,
    favorites: &[String],
    favorites_only: bool,
    input: Entity<text_input::TextInput>,
    lang: i18n::Lang,
    cx: &mut Context<PlanEditor>,
) -> AnyElement {
    let weak = cx.entity().downgrade();
    let t = i18n::T(lang);
    if !open {
        return div().into_any_element();
    }

    let mut rows: Vec<AnyElement> = Vec::new();
    for chunk in TABS.chunks(4) {
        let mut cells: Vec<AnyElement> = Vec::new();
        for item in chunk {
            let is_active = *item == category;
            let pick = weak.clone();
            let value = *item;
            cells.push(
                div()
                    .flex_1()
                    .px_1()
                    .py_1()
                    .rounded_md()
                    .text_center()
                    .text_xs()
                    .border_1()
                    .border_color(if is_active {
                        rgba(ACCENT_LINE)
                    } else {
                        rgba(0x00000000)
                    })
                    .bg(if is_active {
                        rgba(ACCENT_SOFT)
                    } else {
                        rgba(ROW_BG)
                    })
                    .text_color(if is_active {
                        rgb(ACCENT)
                    } else {
                        rgb(PANEL_TEXT)
                    })
                    .child(match t.0 {
                        i18n::Lang::Zh => item.label(),
                        i18n::Lang::En => item.label_en(),
                    })
                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                        if let Some(entity) = pick.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.state.lock().unwrap().set_catalog_filter(value);
                                this.library_scroll = 0;
                                cx.notify();
                            });
                        }
                    })
                    .into_any_element(),
            );
        }
        rows.push(div().flex().gap_1().children(cells).into_any_element());
    }

    let total = catalog_visible_count(category, query, favorites, favorites_only);
    let start_row = scroll.min(total.saturating_sub(1));
    let mut items: Vec<AnyElement> = Vec::new();
    let mut listed = 0usize;
    for (index, spec) in CATALOG.iter().enumerate() {
        if !catalog_matches(spec, query, favorites, favorites_only, category) {
            continue;
        }
        if listed < start_row {
            listed += 1;
            continue;
        }
        if listed >= start_row + LIBRARY_ROWS {
            break;
        }
        listed += 1;
        let pick = weak.clone();
        let star_weak = weak.clone();
        let active = armed && index == active_index;
        let is_fav = favorites::contains(favorites, spec.asset_id);
        let asset_id = spec.asset_id.to_string();
        items.push(
            div()
                .id(("library-item", index))
                .cursor_grab()
                .on_drag(
                    FurnitureDrag { index },
                    |_data: &FurnitureDrag, _, _, cx| cx.new(|_| FurnitureDragPreview),
                )
                .rounded_md()
                .border_1()
                .border_color(if active {
                    rgba(ACCENT_LINE)
                } else {
                    rgba(0x00000000)
                })
                .bg(if active {
                    rgba(ACCENT_SOFT)
                } else {
                    rgba(ROW_BG)
                })
                .text_xs()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .px_2()
                        .py_1()
                        .flex()
                        .justify_between()
                        .gap_1()
                        .text_color(if active { rgb(ACCENT) } else { rgb(PANEL_TEXT) })
                        .child(spec.name)
                        .child(format!(
                            "{:.1}×{:.1}m",
                            spec.width_mm as f32 / 1000.0,
                            spec.depth_mm as f32 / 1000.0
                        ))
                        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                            if let Some(entity) = pick.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.state.lock().unwrap().choose_catalog_item(index);
                                    window.focus(&this.focus, cx);
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .px_1()
                        .text_color(if is_fav {
                            rgb(0xb7791d)
                        } else {
                            rgb(PANEL_MUTED)
                        })
                        .child(if is_fav { "★" } else { "☆" })
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = star_weak.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.toggle_favorite(&asset_id, cx);
                                });
                            }
                        }),
                )
                .into_any_element(),
        );
    }

    let empty = items.is_empty();
    let close = weak.clone();
    let fav_weak = weak.clone();
    let fav_count = favorites.len();
    div()
        .absolute()
        .top(px(102.0))
        .left(px(72.0))
        .w(px(256.0))
        .rounded(px(14.0))
        .bg(rgba(PANEL_BG))
        .shadow_sm()
        .border_1()
        .border_color(rgba(PANEL_BORDER))
        .text_color(rgb(PANEL_TEXT))
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(div().text_xs().text_color(rgb(PANEL_TEXT)).child(t.f(
                    &format!("家具库 · {total}/{}", CATALOG.len()),
                    &format!("Library · {total}/{}", CATALOG.len()),
                )))
                .child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(rgb(PANEL_MUTED))
                        .child(t.s("收起 (L)", "Hide (L)"))
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = close.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.library_open = false;
                                    cx.notify();
                                });
                            }
                        }),
                ),
        )
        .child(
            div()
                .h(px(28.0))
                .w_full()
                .px(px(6.0))
                .flex()
                .items_center()
                .rounded(px(7.0))
                .bg(rgba(0x2328330d))
                .border_1()
                .border_color(rgba(CHROME_BORDER))
                .overflow_hidden()
                .child(input),
        )
        .child(
            div()
                .flex()
                .gap_1()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_xs()
                        .bg(if favorites_only {
                            rgba(0xf0b24a2e)
                        } else {
                            rgba(ROW_BG)
                        })
                        .text_color(if favorites_only {
                            rgb(0x8c5c12)
                        } else {
                            rgb(PANEL_TEXT)
                        })
                        .child(format!("★ {fav_count}"))
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if let Some(entity) = fav_weak.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.favorites_only = !this.favorites_only;
                                    this.library_scroll = 0;
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_xs()
                        .text_color(rgb(PANEL_MUTED))
                        .child(t.s("点击 ☆ 收藏", "tap ☆ to favourite")),
                ),
        )
        .children(rows)
        .when(!empty, |el| {
            el.child(div().flex().flex_col().gap_1().children(items))
        })
        .when(empty, |el| {
            el.child(
                div()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(PANEL_MUTED))
                    .child(t.s("没有匹配的家具", "No matching furniture")),
            )
        })
        .child(
            div()
                .pt_1()
                .text_xs()
                .text_color(rgb(PANEL_MUTED))
                .child(format!(
                    "{}-{} / {} · {}",
                    (start_row + 1).min(total),
                    (start_row + LIBRARY_ROWS).min(total),
                    total,
                    t.s("滚轮翻页", "wheel to scroll"),
                )),
        )
        .into_any_element()
}

const LIBRARY_ROWS: usize = 12;

fn project_modal(
    inputs: [Entity<text_input::TextInput>; 4],
    weak: WeakEntity<PlanEditor>,
    lang: i18n::Lang,
) -> impl IntoElement {
    let t = i18n::T(lang);
    let cancel_weak = weak.clone();
    let save_weak = weak.clone();
    let close_weak = weak.clone();
    let labels = [
        t.s("方案名称", "Plan name"),
        t.s("客户", "Client"),
        t.s("地址", "Address"),
        t.s("备注", "Notes"),
    ];
    let mut rows: Vec<AnyElement> = Vec::new();
    for (label, input) in labels.into_iter().zip(inputs) {
        rows.push(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(9.5))
                        .text_color(rgb(PANEL_MUTED))
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .h(px(32.0))
                        .w_full()
                        .px(px(9.0))
                        .flex()
                        .items_center()
                        .rounded(px(7.0))
                        .bg(rgba(0x2328330a))
                        .border_1()
                        .border_color(rgba(0x23283316))
                        .child(input),
                )
                .into_any_element(),
        );
    }
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x0b0d1238))
        .on_mouse_down(MouseButton::Left, |_ev, _window, _cx| {})
        .child(
            div()
                .w(px(460.0))
                .rounded(px(14.0))
                .bg(rgba(0xfdfcf9f7))
                .border_1()
                .border_color(rgba(CHROME_BORDER))
                .shadow_lg()
                .p(px(20.0))
                .flex()
                .flex_col()
                .gap(px(13.0))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(logo_badge(24.0))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .text_color(rgb(PANEL_TEXT))
                                        .child(t.s("方案信息", "Project info")),
                                ),
                        )
                        .child(
                            div()
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded(px(6.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|style| style.bg(rgba(0x2328330f)))
                                .child(ui_icon(ICON_CLOSE, 13.0, PANEL_MUTED))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if let Some(entity) = close_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.close_project_info(window, cx);
                                        });
                                    }
                                }),
                        ),
                )
                .children(rows)
                .child(
                    div()
                        .pt(px(2.0))
                        .flex()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            div()
                                .h(px(30.0))
                                .px(px(13.0))
                                .rounded(px(7.0))
                                .border_1()
                                .border_color(rgba(CHROME_BORDER))
                                .flex()
                                .items_center()
                                .text_size(px(11.0))
                                .text_color(rgb(PANEL_TEXT))
                                .hover(|style| style.bg(rgba(0x2328331c)))
                                .child(t.s("取消", "Cancel"))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if let Some(entity) = cancel_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.close_project_info(window, cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .h(px(30.0))
                                .px(px(14.0))
                                .rounded(px(7.0))
                                .bg(rgb(ACCENT))
                                .flex()
                                .items_center()
                                .text_size(px(11.0))
                                .text_color(rgb(0xffffff))
                                .hover(|style| style.bg(rgb(0x245ed0)))
                                .child(t.s("保存", "Save"))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if let Some(entity) = save_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.save_project_info(window, cx);
                                        });
                                    }
                                }),
                        ),
                ),
        )
}

fn import_modal(
    input: Entity<text_input::TextInput>,
    file_name: String,
    weak: WeakEntity<PlanEditor>,
    lang: i18n::Lang,
) -> impl IntoElement {
    let t = i18n::T(lang);
    let cancel_weak = weak.clone();
    let underlay_weak = weak.clone();
    let detect_weak = weak.clone();
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x0b0d1240))
        .on_mouse_down(MouseButton::Left, |_ev, _window, _cx| {})
        .child(
            div()
                .w(px(430.0))
                .rounded(px(14.0))
                .border_1()
                .border_color(rgba(CHROME_BORDER))
                .shadow_lg()
                .bg(rgba(0xfdfcf9f7))
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(PANEL_TEXT))
                        .child(t.s("导入户型图", "Import floor plan")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(PANEL_MUTED))
                        .child(SharedString::from(file_name)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(PANEL_MUTED))
                        .child(t.s(
                            "图纸主体实际宽度（毫米）",
                            "Real width of the drawing content (mm)",
                        )),
                )
                .child(
                    div()
                        .h(px(32.0))
                        .w_full()
                        .px(px(9.0))
                        .flex()
                        .items_center()
                        .rounded(px(7.0))
                        .bg(rgba(0x2328330a))
                        .border_1()
                        .border_color(rgba(0x23283316))
                        .child(input),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(PANEL_MUTED))
                        .child(t.s(
                            "识别只提取粗横竖墙线，门窗和弧线请手动补。",
                            "Only thick horizontal/vertical walls are detected; add doors and arcs by hand.",
                        )),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0x2328330d))
                                .text_xs()
                                .child(t.s("取消 (Esc)", "Cancel (Esc)"))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if let Some(entity) = cancel_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.close_import(window, cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0x2328330d))
                                .text_xs()
                                .child(t.s("仅作底图", "Underlay only"))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if let Some(entity) = underlay_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.confirm_import(false, window, cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(0x2f6fed))
                                .text_xs()
                                .text_color(rgb(0xffffff))
                                .child(t.s("识别并新建", "Detect & create"))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if let Some(entity) = detect_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.confirm_import(true, window, cx);
                                        });
                                    }
                                }),
                        ),
                ),
        )
}

fn inspector_edit_modal(
    input: Entity<text_input::TextInput>,
    edit: InspectorEdit,
    weak: WeakEntity<PlanEditor>,
    lang: i18n::Lang,
) -> impl IntoElement {
    let t = i18n::T(lang);
    let title = match edit {
        InspectorEdit::FurnitureWidth => t.s("编辑家具宽度", "Edit furniture width"),
        InspectorEdit::FurnitureDepth => t.s("编辑家具深度", "Edit furniture depth"),
        InspectorEdit::FurnitureX => t.s("编辑家具左右位置", "Edit furniture X"),
        InspectorEdit::FurnitureY => t.s("编辑家具上下位置", "Edit furniture Y"),
        InspectorEdit::WallLength => t.s("编辑墙体长度", "Edit wall length"),
        InspectorEdit::WallThickness => t.s("编辑墙体厚度", "Edit wall thickness"),
        InspectorEdit::WallHeight => t.s("编辑墙体高度", "Edit wall height"),
        InspectorEdit::OpeningWidth => t.s("编辑开口宽度", "Edit opening width"),
        InspectorEdit::OpeningHeight => t.s("编辑开口高度", "Edit opening height"),
        InspectorEdit::OpeningSill => t.s("编辑开口台高", "Edit opening sill"),
    };
    let hint = match edit {
        InspectorEdit::FurnitureWidth | InspectorEdit::FurnitureDepth => t.s(
            "范围 200–6000 mm · 回车应用",
            "Range 200–6000 mm · Enter to apply",
        ),
        InspectorEdit::FurnitureX | InspectorEdit::FurnitureY => t.s(
            "平面坐标，单位 mm · 回车应用",
            "Plan coordinate in mm · Enter to apply",
        ),
        InspectorEdit::WallLength => t.s(
            "范围 300–30000 mm · 起点固定 · 回车应用",
            "Range 300–30000 mm · start fixed · Enter to apply",
        ),
        InspectorEdit::WallThickness => t.s(
            "范围 60–500 mm · 回车应用",
            "Range 60–500 mm · Enter to apply",
        ),
        InspectorEdit::WallHeight => t.s(
            "范围 1000–4000 mm · 回车应用",
            "Range 1000–4000 mm · Enter to apply",
        ),
        InspectorEdit::OpeningWidth => t.s(
            "范围 300–4000 mm · 回车应用",
            "Range 300–4000 mm · Enter to apply",
        ),
        InspectorEdit::OpeningHeight => t.s(
            "范围 300–3000 mm · 回车应用",
            "Range 300–3000 mm · Enter to apply",
        ),
        InspectorEdit::OpeningSill => t.s(
            "范围 0–2000 mm · 回车应用",
            "Range 0–2000 mm · Enter to apply",
        ),
    };
    let cancel_weak = weak.clone();
    let ok_weak = weak.clone();
    let ok_input = input.clone();
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x0b0d1240))
        .on_mouse_down(MouseButton::Left, |_ev, _window, _cx| {})
        .child(
            div()
                .w(px(350.0))
                .rounded(px(14.0))
                .border_1()
                .border_color(rgba(CHROME_BORDER))
                .shadow_lg()
                .bg(rgba(0xfdfcf9f7))
                .p(px(18.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded(px(6.0))
                                .bg(rgba(ACCENT_SOFT))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(ui_icon(ICON_EDIT, 13.0, ACCENT)),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(wgpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(PANEL_TEXT))
                                .child(SharedString::from(title)),
                        ),
                )
                .child(
                    div()
                        .h(px(36.0))
                        .w_full()
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .rounded(px(7.0))
                        .bg(rgba(0x2328330a))
                        .border_1()
                        .border_color(rgba(0x23283316))
                        .child(div().flex_1().min_w(px(0.0)).child(input))
                        .child(
                            div()
                                .ml_auto()
                                .text_size(px(9.0))
                                .text_color(rgb(PANEL_MUTED))
                                .child(SharedString::from("mm")),
                        ),
                )
                .child(
                    div()
                        .text_size(px(8.5))
                        .text_color(rgb(PANEL_MUTED))
                        .child(SharedString::from(hint)),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            div()
                                .h(px(28.0))
                                .px(px(12.0))
                                .rounded(px(7.0))
                                .flex()
                                .items_center()
                                .bg(rgba(0x2328330d))
                                .text_size(px(10.0))
                                .text_color(rgb(PANEL_TEXT))
                                .hover(|style| style.bg(rgba(0x23283316)))
                                .child(t.s("取消", "Cancel"))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if let Some(entity) = cancel_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.close_inspector_edit(window, cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .h(px(28.0))
                                .px(px(12.0))
                                .rounded(px(7.0))
                                .flex()
                                .items_center()
                                .bg(rgb(ACCENT))
                                .text_size(px(10.0))
                                .font_weight(wgpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(0xffffff))
                                .hover(|style| style.opacity(0.92))
                                .child(t.s("应用", "Apply"))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    let text = ok_input.read(cx).text();
                                    if let Some(entity) = ok_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.commit_inspector_edit(text, window, cx);
                                        });
                                    }
                                }),
                        ),
                ),
        )
}

fn rename_modal(
    input: Entity<text_input::TextInput>,
    weak: WeakEntity<PlanEditor>,
    lang: i18n::Lang,
) -> impl IntoElement {
    let t = i18n::T(lang);
    let cancel_weak = weak.clone();
    let ok_weak = weak.clone();
    let ok_input = input.clone();
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x0b0d1240))
        .on_mouse_down(MouseButton::Left, |_ev, _window, _cx| {})
        .child(
            div()
                .w(px(360.0))
                .rounded(px(14.0))
                .border_1()
                .border_color(rgba(CHROME_BORDER))
                .shadow_lg()
                .bg(rgba(0xfdfcf9f7))
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(PANEL_TEXT))
                        .child(t.s("房间名称", "Room name")),
                )
                .child(
                    div()
                        .h(px(34.0))
                        .w_full()
                        .px(px(9.0))
                        .flex()
                        .items_center()
                        .rounded(px(7.0))
                        .bg(rgba(0x2328330a))
                        .border_1()
                        .border_color(rgba(0x23283316))
                        .child(input),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0x2328330d))
                                .text_xs()
                                .child(t.s("取消 (Esc)", "Cancel (Esc)"))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if let Some(entity) = cancel_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.renaming = None;
                                            window.focus(&this.focus, cx);
                                            cx.notify();
                                        });
                                    }
                                }),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(0x2f6fed))
                                .text_xs()
                                .text_color(rgb(0xffffff))
                                .child(t.s("确定 (Enter)", "OK (Enter)"))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    let text = ok_input.read(cx).text();
                                    if let Some(entity) = ok_weak.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.commit_rename(text, window, cx);
                                        });
                                    }
                                }),
                        ),
                ),
        )
}

/// Write panics to a log file: a panic inside an IME callback crosses the
/// Objective-C boundary, so console output is often lost entirely.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let path = std::env::var("HOME")
            .map(|home| format!("{home}/Library/Logs/Fathom-panic.log"))
            .unwrap_or_else(|_| "/tmp/fp-panic.log".to_string());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let text = format!("[{now}] {info}\n");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write;
            let _ = file.write_all(text.as_bytes());
        }
    }));
}

/// macOS: extend the content view under the titlebar and hide the title text,
/// so Fathom's dark canvas replaces the system's white strip.
#[cfg(target_os = "macos")]
fn install_transparent_titlebar(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = <Window as HasWindowHandle>::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    let view = appkit.ns_view.as_ptr() as *mut objc2::runtime::AnyObject;
    if view.is_null() {
        return;
    }
    unsafe {
        let nswindow: *mut objc2::runtime::AnyObject = objc2::msg_send![view, window];
        if nswindow.is_null() {
            return;
        }
        // NSWindowStyleMaskFullSizeContentView is 1 << 15 (objc2-app-kit).
        // The old 1 << 4 value was a reserved bit, so AppKit kept the 44 pt
        // titlebar gutter even though it looked transparent.
        let full_size_content: u64 = 1 << 15;
        let mut mask: u64 = objc2::msg_send![nswindow, styleMask];
        mask |= full_size_content;
        let _: () = objc2::msg_send![nswindow, setStyleMask: mask];
        let _: () = objc2::msg_send![nswindow, setTitlebarAppearsTransparent: true];
        let _: () = objc2::msg_send![nswindow, setTitleVisibility: 1u64];
        // The system still draws a hairline / opaque backing on some macOS
        // versions even when the titlebar "appears" transparent. Clear the
        // backing and drop the separator so the canvas reaches y=0.
        let _: () = objc2::msg_send![nswindow, setOpaque: false];
        let _: () = objc2::msg_send![nswindow, setTitlebarSeparatorStyle: 0u64];
        let clear: *mut objc2::runtime::AnyObject =
            objc2::msg_send![objc2::class!(NSColor), clearColor];
        let _: () = objc2::msg_send![nswindow, setBackgroundColor: clear];
    }
}

#[cfg(not(target_os = "macos"))]
fn install_transparent_titlebar(_window: &Window) {}

#[cfg(not(target_os = "macos"))]
fn begin_window_drag(_window: &Window) {}

#[cfg(target_os = "macos")]
fn begin_window_drag(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = <Window as HasWindowHandle>::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    let view = appkit.ns_view.as_ptr() as *mut objc2::runtime::AnyObject;
    if view.is_null() {
        return;
    }
    unsafe {
        let nswindow: *mut objc2::runtime::AnyObject = objc2::msg_send![view, window];
        if nswindow.is_null() {
            return;
        }
        let app: *mut objc2::runtime::AnyObject =
            objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
        let event: *mut objc2::runtime::AnyObject = objc2::msg_send![app, currentEvent];
        if !event.is_null() {
            let _: () = objc2::msg_send![nswindow, performWindowDragWithEvent: event];
        }
    }
}

fn main() {
    install_panic_hook();
    Application::new().run(|cx: &mut App| {
        text_input::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(VIEW_W as f32), px(VIEW_H as f32)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Fathom 方寸".into()),
                    // Full-size content under a transparent titlebar: the dark
                    // canvas replaces the old white system strip.
                    appears_transparent: true,
                    traffic_light_position: Some(Point::new(px(14.0), px(14.0))),
                }),
                ..Default::default()
            },
            |window: &mut Window, cx: &mut App| {
                install_transparent_titlebar(window);
                // Re-apply once the platform has finished its own window setup;
                // otherwise the native titlebar can come back after this frame.
                window.defer(cx, |window, _| install_transparent_titlebar(window));
                let surface = window
                    .create_wgpu_surface(VIEW_W, VIEW_H, wgpu::TextureFormat::Rgba8UnormSrgb)
                    .expect("WGPUI WGPU surface unavailable");
                let first_run = !scene::user_plan_path().is_file()
                    || std::env::var("FP_NEW_MENU_SMOKE").is_ok();
                let state = Arc::new(Mutex::new(SceneState::new([VIEW_W as f32, VIEW_H as f32])));
                let surface_for_thread = surface.clone();
                let state_for_thread = state.clone();
                let (fps_tx, fps_rx) = mpsc::channel();
                thread::spawn(move || render_loop(surface_for_thread, state_for_thread, fps_tx));
                let view = cx.new(|cx| {
                    let (rename_tx, rename_rx) = mpsc::channel();
                    let rename_input = cx.new(|cx| {
                        text_input::TextInput::new_compact(cx, "房间名称", Some(rename_tx)).light()
                    });
                    let (inspector_tx, inspector_rx) = mpsc::channel();
                    let inspector_input = cx.new(|cx| {
                        text_input::TextInput::new_compact(cx, "mm", Some(inspector_tx)).light()
                    });
                    let (project_tx, project_rx) = mpsc::channel();
                    let project_labels = ["方案名称", "客户", "地址", "备注"];
                    let project_inputs = project_labels.map(|placeholder| {
                        cx.new(|cx| {
                            text_input::TextInput::new_compact(
                                cx,
                                placeholder,
                                Some(project_tx.clone()),
                            )
                            .light()
                        })
                    });
                    let (library_tx, library_rx) = mpsc::channel();
                    let library_input = cx.new(|cx| {
                        text_input::TextInput::new_compact(
                            cx,
                            "搜索家具 / search",
                            Some(library_tx),
                        )
                        .light()
                    });
                    let (import_tx, import_rx) = mpsc::channel();
                    let import_input = cx.new(|cx| {
                        text_input::TextInput::new_compact(cx, "图纸宽度 mm", Some(import_tx))
                            .light()
                    });
                    PlanEditor {
                        surface,
                        state,
                        focus: cx.focus_handle(),
                        focused: false,
                        fps: 0.0,
                        fps_rx,
                        last_autosave: std::time::Instant::now(),
                        rename_input,
                        rename_rx,
                        renaming: None,
                        inspector_edit: None,
                        inspector_input,
                        inspector_rx,
                        project_open: false,
                        project_inputs,
                        project_rx,
                        import_open: false,
                        import_input,
                        import_rx,
                        import_path: None,
                        library_open: false,
                        library_input,
                        library_rx,
                        library_query: String::new(),
                        favorites: favorites::load(),
                        favorites_only: false,
                        layers_open: false,
                        right_open: true,
                        more_open: false,
                        export_open: false,
                        scale_open: false,
                        wall_kind_open: false,
                        help_open: false,
                        toast_msg: None,
                        toast_visible: false,
                        toast_gen: 0,
                        viewport: [VIEW_W as f32, VIEW_H as f32],
                        inspector_shown: false,
                        drag_pos: None,
                        sun_drag: false,
                        library_scroll: 0,
                        new_menu_open: first_run,
                        recent_plans: recent::load(),
                        window_title: "Fathom 方寸".into(),
                    }
                });
                #[cfg(debug_assertions)]
                if std::env::var("FP_RENAME_SMOKE").is_ok() {
                    view.update(cx, |this, cx| {
                        this.renaming = Some(fp_core::EntityId(1));
                        this.rename_input
                            .update(cx, |input, cx| input.set_text("客厅", cx));
                    });
                    let handle = view.read(cx).rename_input.read(cx).handle();
                    window.focus(&handle, cx);
                }
                if let Ok(name) = std::env::var("FP_TEMPLATE_SMOKE") {
                    let kind = match name.as_str() {
                        "studio" => Some(templates::TemplateKind::Studio),
                        "two" => Some(templates::TemplateKind::TwoBed),
                        "blank" => Some(templates::TemplateKind::Blank),
                        "demo" => Some(templates::TemplateKind::Demo),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        view.update(cx, |this, cx| this.create_new(kind, cx));
                    }
                }
                view
            },
        )
        .expect("failed to open the editor window");
        cx.activate(true);
    });
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
