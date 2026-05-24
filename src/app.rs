mod handlers;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui;

use crate::about;
use crate::menu;
use crate::pane::Pane;
use crate::perf;
use crate::settings::{self, AppSettings, ImageSortOrder};
use crate::theme::UiTheme;

/// Target window size in physical pixels (matches iced version behavior).
const DEFAULT_WINDOW_WIDTH: f32 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 720.0;

/// Cursor proximity zones for revealing UI in fullscreen mode (logical pixels).
const FULLSCREEN_TOP_ZONE: f32 = 50.0;
const FULLSCREEN_BOTTOM_ZONE: f32 = 100.0;

/// Preview UI screen size ratio
const SCREEN_PREVIEW_UI_RATIO: f32 = 5.0;

/// Scroll options
const SCROLL_ZOOM_SPEED: f32 = 1.0 / 200.0;

#[cfg(target_os = "windows")]
const CJK_PATHS: [&str; 2] = [
    "C:\\Windows\\Fonts\\msyh.ttc",
    "C:\\Windows\\Fonts\\simsun.ttc",
];

#[cfg(target_os = "linux")]
const CJK_PATHS: [&str; 3] = [
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
];

#[cfg(target_os = "macos")]
const CJK_PATHS: [&str; 3] = [
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
];

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DualPaneMode {
    Synced,
    Independent,
}

pub(crate) struct SliderResult {
    pub target: Option<usize>,
    pub released: bool,
    pub preview_active: bool,
    pub preview_cursor_index: Option<usize>,
    /// Whether the painted preview thumbnail was the exact one for the
    /// hovered index (false while a nearest-neighbor placeholder shows).
    pub preview_exact: bool,
}
struct PreviewLayout {
    hover_pos: egui::Pos2,
    slider_rect: egui::Rect,
    screen_rect: egui::Rect,
    width: f32,
    height: f32,
}

fn paint_preview_popup(
    ui: &egui::Ui,
    tc: &mut crate::cache::ThumbnailCache,
    cursor_index: usize,
    max_images: usize,
    path: &std::path::Path,
    layout: &PreviewLayout,
    stale_since: &mut Option<(usize, Instant)>,
) -> bool {
    let (tex_opt, is_exact) = tc.current_thumbnail_for(cursor_index, path);
    let ui_width = layout.width;
    let ui_height = layout.height;

    let tex_size = match &tex_opt {
        Some(tex) => tex.size_vec2(),
        None => egui::Vec2::new(1.0, 1.0),
    };

    let style = ui.ctx().style();
    let margin = style.spacing.menu_margin;
    let corner_radius = style.visuals.menu_corner_radius;
    let fill = style.visuals.window_fill();
    let stroke = style.visuals.window_stroke();

    let ratio = (ui_width / tex_size.x).min(ui_height / tex_size.y);
    let scaled = tex_size * ratio;

    let label_text = format!("{} / {max_images}", cursor_index + 1);
    let label_font = egui::FontId::proportional(14.0);
    let label_galley = ui.ctx().fonts(|f| {
        f.layout_no_wrap(label_text, label_font, style.visuals.text_color())
    });

    let inner_w = ui_width.max(label_galley.size().x);
    let inner_h = ui_height + 4.0 + label_galley.size().y;
    let ml = margin.left as f32;
    let mr = margin.right as f32;
    let mt = margin.top as f32;
    let mb = margin.bottom as f32;
    let frame_w = ml + inner_w + mr;
    let frame_h = mt + inner_h + mb;

    let preview_x = (layout.hover_pos.x - frame_w / 2.0)
        .clamp(layout.screen_rect.left(), layout.screen_rect.right() - frame_w);
    let preview_y = (layout.slider_rect.top() - frame_h - 8.0)
        .clamp(layout.screen_rect.top(), layout.screen_rect.bottom() - frame_h);

    let frame_rect = egui::Rect::from_min_size(
        egui::pos2(preview_x, preview_y),
        egui::vec2(frame_w, frame_h),
    );

    // Paint on a tooltip layer to avoid hit-testing interference
    let layer_id = egui::LayerId::new(
        egui::Order::Tooltip, egui::Id::new("slider_preview"),
    );
    let painter = ui.ctx().layer_painter(layer_id);

    painter.rect(frame_rect, corner_radius, fill, stroke, egui::epaint::StrokeKind::Outside);

    let content_min = frame_rect.min + egui::vec2(ml, mt);
    let img_center = egui::pos2(
        content_min.x + ui_width / 2.0,
        content_min.y + ui_height / 2.0,
    );
    let img_rect = egui::Rect::from_center_size(img_center, scaled);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

    if let Some(tex) = tex_opt {
        painter.image(tex.id(), img_rect, uv, egui::Color32::WHITE);

        if is_exact {
            *stale_since = None;
        } else {
            let since = match stale_since {
                Some((stale_idx, inst)) if *stale_idx == cursor_index => inst,
                _ => {
                    *stale_since = Some((cursor_index, Instant::now()));
                    &mut stale_since.as_mut().unwrap().1
                }
            };
            let elapsed = since.elapsed().as_secs_f32();
            const GRACE: f32 = 0.15;
            const FADE_DURATION: f32 = 0.3;
            const MAX_ALPHA: f32 = 120.0;
            if elapsed >= GRACE {
                let t = ((elapsed - GRACE) / FADE_DURATION).min(1.0);
                let alpha = (MAX_ALPHA * t) as u8;
                painter.rect_filled(img_rect, 0.0, egui::Color32::from_black_alpha(alpha));
            }
            if elapsed < GRACE + FADE_DURATION {
                ui.ctx().request_repaint();
            }
        }
    }

    let label_pos = egui::pos2(content_min.x, content_min.y + ui_height + 4.0);
    painter.galley(label_pos, label_galley, style.visuals.text_color());

    is_exact
}

/// Render a custom navigation slider (accent handle + two-tone rail).
/// Returns the drag target index and whether the drag was released.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_nav_slider(
    ui: &mut egui::Ui,
    current_idx: usize,
    max_images: usize,
    accent: egui::Color32,
    panes: &mut [Pane],
    preview_stale_since: &mut Option<(usize, Instant)>,
    show_preview: bool,
    bench_hover_t: Option<f32>,
) -> SliderResult {
    if max_images <= 1 {
        return SliderResult {
            target: None,
            released: false,
            preview_active: false,
            preview_cursor_index: None,
            preview_exact: false,
        };
    }
    let mut preview_active = false;
    let mut preview_cursor_index = None;
    let mut preview_exact = false;

    let max = max_images - 1;
    let mut idx = current_idx;
    let mut target = None;

    let slider_width = ui.available_width();
    let thickness = ui
        .text_style_height(&egui::TextStyle::Body)
        .max(ui.spacing().interact_size.y);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(slider_width, thickness), egui::Sense::drag());

    let handle_radius = rect.height() / 2.5;
    let rail_radius = 4.0_f32;
    let cy = rect.center().y;
    let handle_range = (rect.left() + handle_radius)..=(rect.right() - handle_radius);

    if let Some(pos) = response.interact_pointer_pos() {
        let usable = rect.x_range().shrink(handle_radius);
        let drag_t = ((pos.x - usable.min) / (usable.max - usable.min)).clamp(0.0, 1.0);
        idx = (max as f32 * drag_t).round() as usize;
        if idx != current_idx {
            target = Some(idx);
        }
    }
    let released = response.drag_stopped();

    let rail = egui::Rect::from_min_max(
        egui::pos2(rect.left(), cy - rail_radius),
        egui::pos2(rect.right(), cy + rail_radius),
    );
    let t = if max > 0 {
        idx as f32 / max as f32
    } else {
        0.0
    };
    let handle_x = egui::lerp(handle_range, t);

    ui.painter()
        .rect_filled(rail, rail_radius, egui::Color32::from_gray(60));
    let filled = egui::Rect::from_min_max(rail.min, egui::pos2(handle_x, rail.max.y));
    ui.painter().rect_filled(filled, rail_radius, accent);
    ui.painter().circle(
        egui::pos2(handle_x, cy),
        handle_radius,
        accent,
        egui::Stroke::NONE,
    );
    let screen_rect = ui.ctx().screen_rect();
    let ui_width = screen_rect.width() / SCREEN_PREVIEW_UI_RATIO;
    let ui_height = screen_rect.height() / SCREEN_PREVIEW_UI_RATIO;
    // --bench-preview injects a synthetic hover position along the rail so
    // the whole preview path below runs exactly as with a real cursor.
    let hover_pos = bench_hover_t
        .map(|t| {
            let usable = rect.x_range().shrink(handle_radius);
            egui::pos2(egui::lerp(usable.min..=usable.max, t), cy)
        })
        .or_else(|| response.hover_pos());
    if let Some(pos) = hover_pos {
        let usable = rect.x_range().shrink(handle_radius);
        let cursor_t = ((pos.x - usable.min) / (usable.max - usable.min)).clamp(0.0, 1.0);
        let cursor_index = (max as f32 * cursor_t).round() as usize;
        if let Some(pane) = panes.get_mut(0) {
            let nav_active = ui.ctx().input(|i| {
                i.key_down(egui::Key::ArrowLeft) || i.key_down(egui::Key::ArrowRight)
                || i.key_down(egui::Key::A) || i.key_down(egui::Key::D)
            });
            if cursor_index < pane.image_paths.len() && !response.dragged() && !nav_active && show_preview {
                if let Some(tc) = pane.thumbnail_cache.as_mut() {
                    preview_active = true;
                    preview_cursor_index = Some(cursor_index);
                    let layout = PreviewLayout {
                        hover_pos: pos, slider_rect: rect, screen_rect,
                        width: ui_width, height: ui_height,
                    };
                    preview_exact = paint_preview_popup(
                        ui, tc, cursor_index, max_images,
                        &pane.image_paths[cursor_index],
                        &layout, preview_stale_since,
                    );
                }
            }
        }
    }

    SliderResult { target, released, preview_active, preview_cursor_index, preview_exact }
}

pub struct App {
    pub(crate) panes: Vec<Pane>,
    pub(crate) perf: perf::ImagePerfTracker,
    pub(crate) divider_fraction: f32,
    pub(crate) dual_pane_mode: DualPaneMode,
    /// persistent settings
    pub(crate) settings: AppSettings,
    /// Currently applied sort order settings (via View -> Sort By menu)
    pub(crate) current_sort: ImageSortOrder,
    pub(crate) theme: UiTheme,
    pub(crate) show_settings: bool,
    pub(crate) show_about: bool,
    pub(crate) is_fullscreen: bool,
    pub(crate) menu_open: bool,
    pub(crate) log_buffer: Arc<Mutex<VecDeque<String>>>,
    initial_size_set: bool,
    title: Option<String>,
    file_receiver: Receiver<PathBuf>,
    last_preview_idx: Option<usize>,
    preview_stale_since: Option<(usize, Instant)>,
    preview_bench: Option<crate::bench::preview::PreviewBench>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: Vec<PathBuf>,
        log_buffer: Arc<Mutex<VecDeque<String>>>,
        settings: AppSettings,
        file_receiver: Receiver<PathBuf>,
        bench_preview: bool,
    ) -> Self {
        let theme = UiTheme::teal_dark();
        theme.apply_to_visuals(&cc.egui_ctx);
        let mut app = Self {
            panes: vec![Pane::new(
                &cc.egui_ctx,
                settings.cache_count,
                settings.lru_budget_mb,
                settings.decode_threads,
                settings.mouse_wheel_zoom,
                settings.reset_zoom_pan_on_navigation,
                settings.preview_budget_mb,
            )],
            perf: perf::ImagePerfTracker::new(),
            divider_fraction: 0.5,
            dual_pane_mode: DualPaneMode::Synced,
            current_sort: settings.image_discovery_options.sort_order,
            settings,
            theme,
            show_settings: false,
            show_about: false,
            is_fullscreen: false,
            menu_open: false,
            log_buffer,
            initial_size_set: false,
            title: None,
            file_receiver,
            last_preview_idx: None,
            preview_stale_since: None,
            preview_bench: None,
        };

        if !paths.is_empty() {
            app.panes[0].open_path(
                &paths[0],
                &cc.egui_ctx,
                app.settings.image_discovery_options,
            );
        }
        if paths.len() >= 2 {
            let mut pane1 = Pane::new(
                &cc.egui_ctx,
                app.settings.cache_count,
                app.settings.lru_budget_mb,
                app.settings.decode_threads,
                app.settings.mouse_wheel_zoom,
                app.settings.reset_zoom_pan_on_navigation,
                app.settings.preview_budget_mb,
            );
            pane1.open_path(
                &paths[1],
                &cc.egui_ctx,
                app.settings.image_discovery_options,
            );
            app.panes.push(pane1);
        }

        if app.panes[0].current_texture.is_some() {
            app.perf.record_image_load();
        }

        if bench_preview {
            let n = app.panes[0].image_paths.len();
            if n > 1 {
                log::info!("preview bench: starting on {n} images");
                app.preview_bench = Some(crate::bench::preview::PreviewBench::new(n));
            } else {
                log::error!("--bench-preview requires a folder with at least 2 images");
            }
        }

        let mut fonts = egui::FontDefinitions::default();
        let mut cjk_loaded = false;
        for path in &CJK_PATHS {
            if let Ok(data) = std::fs::read(path) {
                fonts.font_data.insert(
                    "cjk_fallback".to_string(),
                    egui::FontData::from_owned(data).into(),
                );
                fonts.families.entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push("cjk_fallback".to_string());
                fonts.families.entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push("cjk_fallback".to_string());
                cjk_loaded = true;
                break;
            }
        }
        if !cjk_loaded {
            log::info!("No CJK font found — CJK characters will render as missing glyphs");
        } else {
            cc.egui_ctx.set_fonts(fonts);
        }
        cc.egui_ctx.options_mut(|o| o.scroll_zoom_speed = SCROLL_ZOOM_SPEED);

        app
    }

    fn current_title(&self) -> String {
        let name = |pane: &Pane| -> Option<String> {
            pane.image_paths.get(pane.current_index).map(|path| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
        };

        if self.panes.len() >= 2 {
            let left = name(&self.panes[0]).unwrap_or_default();
            let right = name(&self.panes[1]).unwrap_or_default();
            if left.is_empty() && right.is_empty() {
                "ViewSkater".to_string()
            } else {
                format!("Left: {} | Right: {}", left, right)
            }
        } else {
            self.panes.first().and_then(name)
                .unwrap_or_else(|| "ViewSkater".to_string())
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = self.current_title();
        if self.title.as_deref() != Some(title.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = Some(title);
        }
    }

    fn show_slider_panel(&mut self, ctx: &egui::Context) {
        // In independent dual-pane mode, sliders are rendered per-pane
        if self.panes.len() >= 2 && self.dual_pane_mode == DualPaneMode::Independent {
            return;
        }

        let max_images = self
            .panes
            .iter()
            .map(|p| p.image_paths.len())
            .max()
            .unwrap_or(0);
        if max_images <= 1 {
            return;
        }

        let current_idx = self
            .panes
            .iter()
            .filter(|p| !p.image_paths.is_empty())
            .map(|p| p.current_index)
            .max()
            .unwrap_or(0);

        let accent = self.theme.accent;
        let mut stale_since = self.preview_stale_since;
        let preview = (self.settings.slider_preview || self.preview_bench.is_some())
            && self.panes.len() < 2;
        let bench_t = self.preview_bench.as_ref().and_then(|b| b.hover_t());
        let result = egui::TopBottomPanel::bottom("nav")
            .show(ctx, |ui| paint_nav_slider(ui, current_idx, max_images, accent, &mut self.panes, &mut stale_since, preview, bench_t))
            .inner;
        self.preview_stale_since = stale_since;

        if let Some(bench) = &mut self.preview_bench {
            // At each phase end, sample the overlay's Preview FPS counter
            // and reset its 2s window so the next phase's sample is pure.
            if let Some(phase) = bench.tick(result.preview_cursor_index, result.preview_exact) {
                bench.set_overlay_fps(&phase, self.perf.frame_fps());
                self.perf.clear_frames();
            }
            if bench.is_done() {
                log::info!("{}", bench.report());
                self.preview_bench = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint();
            }
        }
        if result.preview_active {
            if result.preview_cursor_index != self.last_preview_idx {
                self.perf.record_frame();
                self.last_preview_idx = result.preview_cursor_index;
            }
        } else {
            self.perf.clear_frames();
            self.last_preview_idx = None;
            self.preview_stale_since = None;
        }

        self.apply_slider_result_all(result, ctx);
    }

    fn show_central_panel(&mut self, ctx: &egui::Context) {
        let independent =
            self.panes.len() >= 2 && self.dual_pane_mode == DualPaneMode::Independent;
        let accent = self.theme.accent;
        let mut preview_stale_since = self.preview_stale_since;

        let slider_results = egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_gray(20)))
            .show(ctx, |ui| {
                let mut results: Vec<(usize, SliderResult)> = Vec::new();

                if self.panes.len() <= 1 {
                    if let Some(pane) = self.panes.first_mut() {
                        pane.show_content(ui);
                    }
                } else {
                    let available = ui.available_rect_before_wrap();
                    let divider_w = 4.0;
                    let grab_w = 12.0;
                    let left_w = (available.width() - divider_w) * self.divider_fraction;

                    // Reserve space for selection strip and per-pane sliders
                    let strip_h = if independent { 18.0 } else { 0.0 };
                    let slider_h = if independent {
                        ui.text_style_height(&egui::TextStyle::Body)
                            .max(ui.spacing().interact_size.y)
                    } else {
                        0.0
                    };

                    let content_h = available.height() - strip_h - slider_h;
                    let right_x = available.min.x + left_w + divider_w;
                    let right_w = available.width() - left_w - divider_w;
                    let content_y = available.min.y + strip_h;

                    let left_rect = egui::Rect::from_min_size(
                        egui::pos2(available.min.x, content_y),
                        egui::vec2(left_w, content_h),
                    );
                    let right_rect = egui::Rect::from_min_size(
                        egui::pos2(right_x, content_y),
                        egui::vec2(right_w, content_h),
                    );

                    // Divider interaction
                    let divider_center_x = available.min.x + left_w + divider_w / 2.0;
                    let grab_rect = egui::Rect::from_center_size(
                        egui::pos2(divider_center_x, available.center().y),
                        egui::vec2(grab_w, available.height()),
                    );
                    let divider_response =
                        ui.allocate_rect(grab_rect, egui::Sense::click_and_drag());

                    if divider_response.dragged() {
                        let usable = available.width() - divider_w;
                        if usable > 0.0 {
                            let delta = divider_response.drag_delta().x;
                            self.divider_fraction =
                                (self.divider_fraction + delta / usable).clamp(0.1, 0.9);
                        }
                    }
                    if divider_response.double_clicked() {
                        self.divider_fraction = 0.5;
                    }
                    if divider_response.hovered() || divider_response.dragged() {
                        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }

                    let divider_color = if divider_response.dragged() {
                        egui::Color32::from_gray(140)
                    } else if divider_response.hovered() {
                        egui::Color32::from_gray(100)
                    } else {
                        egui::Color32::from_gray(60)
                    };
                    ui.painter().vline(
                        divider_center_x,
                        available.y_range(),
                        egui::Stroke::new(divider_w, divider_color),
                    );

                    let sync_zp = self.settings.sync_zoom_pan;
                    let (first, rest) = self.panes.split_at_mut(1);

                    // Pane images
                    let left_interacted = ui
                        .allocate_new_ui(
                            egui::UiBuilder::new().max_rect(left_rect),
                            |ui| first[0].show_content(ui),
                        )
                        .inner;
                    let right_interacted = ui
                        .allocate_new_ui(
                            egui::UiBuilder::new().max_rect(right_rect),
                            |ui| rest[0].show_content(ui),
                        )
                        .inner;

                    // Sync zoom/pan across panes
                    if sync_zp {
                        if right_interacted {
                            first[0].zoom = rest[0].zoom;
                            first[0].pan = rest[0].pan;
                        } else if left_interacted {
                            rest[0].zoom = first[0].zoom;
                            rest[0].pan = first[0].pan;
                        }
                    }

                    // Clickable selection strips at top of each pane (independent mode)
                    if independent {
                        let muted = egui::Color32::from_gray(50);
                        for (i, (pane, x, w)) in [
                            (&first[0], available.min.x, left_w),
                            (&rest[0], right_x, right_w),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let strip_rect = egui::Rect::from_min_size(
                                egui::pos2(x, available.min.y),
                                egui::vec2(w, strip_h),
                            );
                            let color = if pane.selected { accent } else { muted };
                            ui.painter().rect_filled(strip_rect, 0.0, color);

                            let label = format!("{}", i + 1);
                            let text_color = if pane.selected {
                                egui::Color32::BLACK
                            } else {
                                egui::Color32::from_gray(120)
                            };
                            ui.painter().text(
                                strip_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                label,
                                egui::FontId::monospace(11.0),
                                text_color,
                            );
                        }

                        // Handle clicks on strips (use raw pointer to avoid
                        // conflicting with divider/pane interactions)
                        if ui.input(|i| i.pointer.any_click()) {
                            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                                let strip_area = egui::Rect::from_min_size(
                                    available.min,
                                    egui::vec2(available.width(), strip_h),
                                );
                                if strip_area.contains(pos) {
                                    let divider_center =
                                        available.min.x + left_w + divider_w / 2.0;
                                    if pos.x < divider_center {
                                        first[0].selected = !first[0].selected;
                                    } else {
                                        rest[0].selected = !rest[0].selected;
                                    }
                                }
                            }
                        }
                    }

                    // Per-pane sliders (independent mode)
                    if independent {
                        let slider_y = available.max.y - slider_h;

                        let left_slider_rect = egui::Rect::from_min_size(
                            egui::pos2(available.min.x, slider_y),
                            egui::vec2(left_w, slider_h),
                        );
                        let mut stale_l = preview_stale_since;
                        let left_result = ui
                            .allocate_new_ui(
                                egui::UiBuilder::new().max_rect(left_slider_rect),
                                |ui| {
                                    paint_nav_slider(
                                        ui,
                                        first[0].current_index,
                                        first[0].image_paths.len(),
                                        accent,
                                        first,
                                        &mut stale_l,
                                        false,
                                        None,
                                    )
                                },
                            )
                            .inner;
                        preview_stale_since = stale_l;
                        results.push((0, left_result));

                        let right_slider_rect = egui::Rect::from_min_size(
                            egui::pos2(right_x, slider_y),
                            egui::vec2(right_w, slider_h),
                        );
                        let mut stale_r = preview_stale_since;
                        let right_result = ui
                            .allocate_new_ui(
                                egui::UiBuilder::new().max_rect(right_slider_rect),
                                |ui| {
                                    paint_nav_slider(
                                        ui,
                                        rest[0].current_index,
                                        rest[0].image_paths.len(),
                                        accent,
                                        rest,
                                        &mut stale_r,
                                        false,
                                        None,
                                    )
                                },
                            )
                            .inner;
                        preview_stale_since = stale_r;
                        results.push((1, right_result));
                    }
                }

                results
            })
            .inner;

        self.preview_stale_since = preview_stale_since;
        for (pane_idx, result) in slider_results {
            self.apply_slider_result_one(pane_idx, result, ctx);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Synchronize with the GPU before building the next frame.
        // Without this, wgpu's multi-stage pipeline (staging buffer → copy →
        // submit → present) can finish at variable times, causing irregular
        // frame spacing during rapid keyboard navigation of 4K images.
        // Blocking here until the previous frame's GPU work completes gives
        // deterministic frame pacing.
        if let Some(render_state) = frame.wgpu_render_state() {
            render_state.device.poll(eframe::wgpu::Maintain::Wait);
            self.perf.sample_gpu_memory(&render_state.device);
        }

        // Force dark theme every frame (egui_winit can reapply system theme on macOS)
        self.theme.apply_to_visuals(ctx);

        // On first frame, resize to achieve the target physical pixel size.
        // egui's with_inner_size uses logical points, so on scaled displays
        // (e.g. 1.25x) 1280x720 logical becomes 1600x900 physical. The iced
        // version uses PhysicalSize directly, so it doesn't have this issue.
        if !self.initial_size_set {
            if let Some(ppp) = ctx.input(|i| i.viewport().native_pixels_per_point) {
                if (ppp - 1.0).abs() > 0.01 {
                    let logical = egui::vec2(
                        DEFAULT_WINDOW_WIDTH / ppp,
                        DEFAULT_WINDOW_HEIGHT / ppp,
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(logical));
                }
            }
            self.initial_size_set = true;
        }

        for pane in &mut self.panes {
            pane.poll_cache();
            pane.poll_animation();
        }

        self.handle_external_open_requests(ctx);
        self.handle_dropped_files(ctx);
        self.handle_keyboard(ctx);
        self.update_title(ctx);

        // Detect cursor proximity to screen edges for fullscreen UI reveal
        let (cursor_near_top, cursor_near_bottom) = if self.is_fullscreen {
            let screen = ctx.screen_rect();
            ctx.input(|i| {
                if let Some(pos) = i.pointer.hover_pos() {
                    (
                        pos.y - screen.min.y < FULLSCREEN_TOP_ZONE,
                        screen.max.y - pos.y < FULLSCREEN_BOTTOM_ZONE,
                    )
                } else {
                    (false, false)
                }
            })
        } else {
            (false, false)
        };

        // Compute cache memory breakdown for FPS overlay
        let cache_mb = if self.settings.show_fps {
            let (lru, sw) = self.panes.first().map_or((0.0, 0.0), |p| p.cache_memory_mb());
            Some((lru, sw))
        } else {
            None
        };

        // Menu bar (top) — in fullscreen, revealed when cursor near top edge
        // or when a menu dropdown is open (so user can interact with items)
        let show_menu = !self.is_fullscreen || cursor_near_top || self.menu_open;
        if show_menu {
            let fps_text = if self.settings.show_fps && !self.is_fullscreen {
                Some(self.perf.fps_text(cache_mb))
            } else {
                None
            };
            let settings_snapshot = self.settings.clone();
            let sort_snapshot = self.current_sort;
            let mut menu_state = menu::MenuBarState {
                settings: &mut self.settings,
                current_sort: &mut self.current_sort,
                is_fullscreen: self.is_fullscreen,
            };
            let (action, menu_is_open) = menu::show_menu_bar(
                ctx,
                &self.panes,
                self.dual_pane_mode,
                &mut menu_state,
                &self.theme,
                fps_text.as_deref(),
            );
            self.menu_open = menu_is_open;
            if self.settings != settings_snapshot {
                self.settings.save();
            }
            if self.current_sort != sort_snapshot {
                self.reload_sorted_panes(ctx);
            }
            self.handle_menu_action(action, ctx);
        } else {
            self.menu_open = false;
        }

        // Footer — in fullscreen, revealed when cursor near bottom edge
        if self.settings.show_footer && (!self.is_fullscreen || cursor_near_bottom) {
            menu::show_footer(ctx, &self.panes, self.divider_fraction);
        }

        // Slider panel — in fullscreen, revealed when cursor near bottom edge
        if !self.is_fullscreen || cursor_near_bottom {
            self.show_slider_panel(ctx);
        }

        // Central panel (must be last — fills remaining space)
        self.show_central_panel(ctx);

        // FPS overlay in fullscreen (painted over central panel, top-right corner)
        if self.is_fullscreen && self.settings.show_fps {
            let fps = self.perf.fps_text(cache_mb);
            let screen = ctx.screen_rect();
            let font = egui::FontId::monospace(14.0);
            let color = egui::Color32::from_rgba_unmultiplied(220, 220, 220, 200);
            let bg = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 140);
            let galley = ctx.fonts(|f| f.layout_no_wrap(fps, font, color));
            let text_size = galley.size();
            let margin = 8.0;
            let pos = egui::pos2(
                screen.max.x - text_size.x - margin * 2.0,
                screen.min.y + margin,
            );
            let bg_rect = egui::Rect::from_min_size(
                pos,
                text_size + egui::vec2(margin * 2.0, margin),
            );
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("fullscreen_fps"),
            ));
            painter.rect_filled(bg_rect, 4.0, bg);
            painter.galley(pos + egui::vec2(margin, margin * 0.5), galley, color);
        }

        // Overlays
        if self.settings.show_cache_overlay {
            if let Some(pane) = self.panes.first() {
                if let Some(cache) = &pane.cache {
                    cache.show_debug_overlay(ctx, pane.current_index, pane.image_paths.len());
                }
            }
        }

        // Settings modal — auto-saves on any change inside the modal.
        let settings_changes =
            settings::show_settings_modal(ctx, &mut self.settings, &mut self.show_settings, &self.theme);
        if settings_changes.pane_settings {
            self.apply_settings_to_caches();
            // also reload panes but ensure image discovery options are updated
            self.current_sort = self.settings.image_discovery_options.sort_order;
            self.reload_sorted_panes(ctx);
        }

        // About modal (on top of everything)
        about::show_about_modal(ctx, &mut self.show_about, &self.theme);
    }
}
