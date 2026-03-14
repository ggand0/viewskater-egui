mod handlers;

use std::path::PathBuf;

use eframe::egui;

use crate::about;
use crate::menu;
use crate::pane::Pane;
use crate::perf;
use crate::settings::{self, AppSettings};
use crate::theme::UiTheme;

/// Target window size in physical pixels (matches iced version behavior).
const DEFAULT_WINDOW_WIDTH: f32 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 720.0;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DualPaneMode {
    Synced,
    Independent,
}

pub(crate) struct SliderResult {
    pub target: Option<usize>,
    pub released: bool,
}

/// Render a custom navigation slider (accent handle + two-tone rail).
/// Returns the drag target index and whether the drag was released.
pub(crate) fn paint_nav_slider(
    ui: &mut egui::Ui,
    current_idx: usize,
    max_images: usize,
    accent: egui::Color32,
) -> SliderResult {
    if max_images <= 1 {
        return SliderResult {
            target: None,
            released: false,
        };
    }

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

    SliderResult { target, released }
}

pub struct App {
    pub(crate) panes: Vec<Pane>,
    pub(crate) perf: perf::ImagePerfTracker,
    pub(crate) divider_fraction: f32,
    pub(crate) dual_pane_mode: DualPaneMode,
    pub(crate) settings: AppSettings,
    pub(crate) theme: UiTheme,
    pub(crate) show_settings: bool,
    pub(crate) show_about: bool,
    initial_size_set: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        let settings = AppSettings::load();
        let theme = UiTheme::teal_dark();
        theme.apply_to_visuals(&cc.egui_ctx);
        let mut app = Self {
            panes: vec![Pane::new(settings.cache_count, settings.lru_capacity)],
            perf: perf::ImagePerfTracker::new(),
            divider_fraction: 0.5,
            dual_pane_mode: DualPaneMode::Synced,
            settings,
            theme,
            show_settings: false,
            show_about: false,
            initial_size_set: false,
        };

        if !paths.is_empty() {
            app.panes[0].open_path(&paths[0], &cc.egui_ctx);
        }
        if paths.len() >= 2 {
            let mut pane1 = Pane::new(app.settings.cache_count, app.settings.lru_capacity);
            pane1.open_path(&paths[1], &cc.egui_ctx);
            app.panes.push(pane1);
        }

        if app.panes[0].current_texture.is_some() {
            app.perf.record_image_load();
        }

        app
    }

    fn update_title(&self, ctx: &egui::Context) {
        let name = |pane: &Pane| -> Option<String> {
            pane.image_paths.get(pane.current_index).map(|path| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
        };

        let title = if self.panes.len() >= 2 {
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
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
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
        let result = egui::TopBottomPanel::bottom("nav")
            .show(ctx, |ui| paint_nav_slider(ui, current_idx, max_images, accent))
            .inner;

        self.apply_slider_result_all(result, ctx);
    }

    fn show_central_panel(&mut self, ctx: &egui::Context) {
        let independent =
            self.panes.len() >= 2 && self.dual_pane_mode == DualPaneMode::Independent;
        let accent = self.theme.accent;

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
                        let left_result = ui
                            .allocate_new_ui(
                                egui::UiBuilder::new().max_rect(left_slider_rect),
                                |ui| {
                                    paint_nav_slider(
                                        ui,
                                        first[0].current_index,
                                        first[0].image_paths.len(),
                                        accent,
                                    )
                                },
                            )
                            .inner;
                        results.push((0, left_result));

                        let right_slider_rect = egui::Rect::from_min_size(
                            egui::pos2(right_x, slider_y),
                            egui::vec2(right_w, slider_h),
                        );
                        let right_result = ui
                            .allocate_new_ui(
                                egui::UiBuilder::new().max_rect(right_slider_rect),
                                |ui| {
                                    paint_nav_slider(
                                        ui,
                                        rest[0].current_index,
                                        rest[0].image_paths.len(),
                                        accent,
                                    )
                                },
                            )
                            .inner;
                        results.push((1, right_result));
                    }
                }

                results
            })
            .inner;

        for (pane_idx, result) in slider_results {
            self.apply_slider_result_one(pane_idx, result, ctx);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        }

        self.handle_dropped_files(ctx);
        self.handle_keyboard(ctx);
        self.update_title(ctx);

        // Menu bar (top)
        let fps_text = if self.settings.show_fps {
            Some(self.perf.fps_text())
        } else {
            None
        };
        let settings_snapshot = self.settings.clone();
        let action = menu::show_menu_bar(
            ctx,
            &self.panes,
            self.dual_pane_mode,
            &mut self.settings,
            &self.theme,
            fps_text.as_deref(),
        );
        if self.settings != settings_snapshot {
            self.settings.save();
        }
        self.handle_menu_action(action, ctx);

        // Footer (bottom, before slider so it's below the slider)
        if self.settings.show_footer {
            menu::show_footer(ctx, &self.panes, self.divider_fraction);
        }

        // Slider panel (bottom)
        self.show_slider_panel(ctx);

        // Central panel (must be last — fills remaining space)
        self.show_central_panel(ctx);

        // Overlays
        if self.settings.show_cache_overlay {
            if let Some(pane) = self.panes.first() {
                if let Some(cache) = &pane.cache {
                    cache.show_debug_overlay(ctx, pane.current_index, pane.image_paths.len());
                }
            }
        }

        // Settings modal
        let prev_show_settings = self.show_settings;
        let perf_changed =
            settings::show_settings_modal(ctx, &mut self.settings, &mut self.show_settings, &self.theme);
        if perf_changed {
            self.apply_settings_to_caches();
        }
        if prev_show_settings && !self.show_settings {
            self.settings.save();
        }

        // About modal (on top of everything)
        about::show_about_modal(ctx, &mut self.show_about, &self.theme);
    }
}
