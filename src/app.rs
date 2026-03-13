use std::path::PathBuf;

use eframe::egui;

use crate::about;
use crate::menu::{self, MenuAction};
use crate::pane::Pane;
use crate::perf;
use crate::settings::{self, AppSettings};
use crate::theme::UiTheme;

/// Target window size in physical pixels (matches iced version behavior).
const DEFAULT_WINDOW_WIDTH: f32 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 720.0;

pub struct App {
    panes: Vec<Pane>,
    perf: perf::ImagePerfTracker,
    divider_fraction: f32,
    settings: AppSettings,
    theme: UiTheme,
    show_settings: bool,
    show_about: bool,
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
            app.perf.record_image_load(0.0);
        }

        app
    }

    fn set_single_pane(&mut self) {
        if self.panes.len() >= 2 {
            self.panes.truncate(1);
        }
    }

    fn set_dual_pane(&mut self, ctx: &egui::Context) {
        if self.panes.len() < 2 {
            let mut pane = Pane::new(self.settings.cache_count, self.settings.lru_capacity);
            if !self.panes[0].image_paths.is_empty() {
                if let Some(dir) = self.panes[0].image_paths[0].parent() {
                    pane.open_path(dir, ctx);
                    pane.jump_to(self.panes[0].current_index, ctx);
                }
            }
            self.panes.push(pane);
        }
    }

    fn open_folder_dialog(&mut self, pane_idx: usize, ctx: &egui::Context) {
        if let Some(pane) = self.panes.get_mut(pane_idx) {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                pane.open_path(&dir, ctx);
            }
        }
    }

    fn open_file_dialog(&mut self, pane_idx: usize, ctx: &egui::Context) {
        if let Some(pane) = self.panes.get_mut(pane_idx) {
            if let Some(file) = rfd::FileDialog::new()
                .add_filter("Images", &["jpg", "jpeg", "png", "bmp", "webp", "gif", "tiff", "tif", "qoi", "tga"])
                .pick_file()
            {
                pane.open_path(&file, ctx);
            }
        }
    }

    fn close_images(&mut self) {
        for pane in &mut self.panes {
            pane.close();
        }
    }

    fn handle_menu_action(&mut self, action: MenuAction, ctx: &egui::Context) {
        match action {
            MenuAction::None => {}
            MenuAction::OpenFolder(idx) => self.open_folder_dialog(idx, ctx),
            MenuAction::OpenFile(idx) => self.open_file_dialog(idx, ctx),
            MenuAction::Close => self.close_images(),
            MenuAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            MenuAction::SetSinglePane => self.set_single_pane(),
            MenuAction::SetDualPane => self.set_dual_pane(ctx),
            MenuAction::ShowAbout => self.show_about = true,
            MenuAction::ShowSettings => self.show_settings = true,
        }
    }

    fn apply_settings_to_caches(&mut self) {
        for pane in &mut self.panes {
            if let Some(cache) = &mut pane.cache {
                cache.set_cache_count(
                    self.settings.cache_count,
                    pane.current_index,
                    &pane.image_paths,
                );
            }
            pane.decode_cache.set_capacity(self.settings.lru_capacity);
            pane.cache_count = self.settings.cache_count;
            pane.lru_capacity = self.settings.lru_capacity;
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let (home, end, shift, nav_right_pressed, nav_left_pressed,
             nav_right_held, nav_left_held, toggle_dual, set_single, set_dual,
             toggle_footer, open_folder, open_file, close, quit) =
            ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Home),
                    i.key_pressed(egui::Key::End),
                    i.modifiers.shift,
                    i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::D),
                    i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::A),
                    i.key_down(egui::Key::ArrowRight) || i.key_down(egui::Key::D),
                    i.key_down(egui::Key::ArrowLeft) || i.key_down(egui::Key::A),
                    i.key_pressed(egui::Key::Tab),
                    i.key_pressed(egui::Key::Num1) && i.modifiers.command,
                    i.key_pressed(egui::Key::Num2) && i.modifiers.command,
                    i.key_pressed(egui::Key::Tab),
                    i.key_pressed(egui::Key::O) && i.modifiers.command && i.modifiers.shift,
                    i.key_pressed(egui::Key::O) && i.modifiers.command && !i.modifiers.shift,
                    i.key_pressed(egui::Key::W) && i.modifiers.command,
                    i.key_pressed(egui::Key::Q) && i.modifiers.command,
                )
            });

        if quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if open_folder {
            self.open_folder_dialog(0, ctx);
            return;
        }
        if open_file {
            self.open_file_dialog(0, ctx);
            return;
        }
        if close {
            self.close_images();
            return;
        }
        if toggle_footer {
            self.settings.show_footer = !self.settings.show_footer;
            self.settings.save();
            return;
        }

        // Skate mode (Shift held): advance every frame while key is down
        // Normal mode: advance once per key press/repeat event (~30hz)
        let nav_right = if shift { nav_right_held } else { nav_right_pressed };
        let nav_left = if shift { nav_left_held } else { nav_left_pressed };

        if set_single && self.panes.len() >= 2 {
            self.set_single_pane();
            return;
        }
        if set_dual && self.panes.len() == 1 {
            self.set_dual_pane(ctx);
            return;
        }

        if toggle_dual {
            if self.panes.len() >= 2 {
                self.set_single_pane();
            } else if !self.panes.is_empty() {
                self.set_dual_pane(ctx);
            }
            return;
        }

        if home {
            for pane in &mut self.panes {
                pane.jump_to(0, ctx);
            }
            self.perf.record_image_load(0.0);
        } else if end {
            for pane in &mut self.panes {
                let last = pane.image_paths.len().saturating_sub(1);
                pane.jump_to(last, ctx);
            }
            self.perf.record_image_load(0.0);
        } else if nav_right {
            // Only advance if ALL panes have the next image cached (synced nav)
            let all_ready = self.panes.iter().all(|p| p.is_next_cached(1));
            if all_ready {
                // fold instead of any() to avoid short-circuit — call navigate on every pane
                let any_advanced = self.panes.iter_mut().fold(false, |acc, p| p.navigate(1) || acc);
                if any_advanced {
                    self.perf.record_image_load(0.0);
                }
            }
            let any_can = self.panes.iter().any(|p| p.can_navigate_forward());
            if any_can {
                ctx.request_repaint();
            }
        } else if nav_left {
            let all_ready = self.panes.iter().all(|p| p.is_next_cached(-1));
            if all_ready {
                let any_advanced = self.panes.iter_mut().fold(false, |acc, p| p.navigate(-1) || acc);
                if any_advanced {
                    self.perf.record_image_load(0.0);
                }
            }
            let any_can = self.panes.iter().any(|p| p.can_navigate_backward());
            if any_can {
                ctx.request_repaint();
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.first() {
            if let Some(path) = &file.path {
                if self.panes.len() >= 2 {
                    let hover = ctx.input(|i| i.pointer.hover_pos());
                    let latest = ctx.input(|i| i.pointer.latest_pos());
                    let screen = ctx.screen_rect();
                    let divider_x =
                        screen.min.x + screen.width() * self.divider_fraction;
                    log::debug!(
                        "DnD drop: hover_pos={:?}, latest_pos={:?}, screen={:?}, divider_x={}, fraction={}",
                        hover, latest, screen, divider_x, self.divider_fraction
                    );
                    let target = hover
                        .or(latest)
                        .map(|pos| {
                            log::debug!("DnD using pos x={}, divider_x={} → pane {}", pos.x, divider_x, if pos.x < divider_x { 0 } else { 1 });
                            if pos.x < divider_x { 0 } else { 1 }
                        })
                        .unwrap_or(0);
                    self.panes[target].open_path(path, ctx);
                } else {
                    self.panes[0].open_path(path, ctx);
                }
            }
        }
    }

    fn update_title(&self, ctx: &egui::Context) {
        let parts: Vec<String> = self
            .panes
            .iter()
            .filter_map(|pane| {
                pane.image_paths.get(pane.current_index).map(|path| {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    format!("{} ({}/{})", name, pane.current_index + 1, pane.image_paths.len())
                })
            })
            .collect();

        let title = if parts.is_empty() {
            "viewskater-egui".to_string()
        } else {
            format!("{} - viewskater-egui", parts.join(" | "))
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    fn show_slider_panel(&mut self, ctx: &egui::Context) {
        let max_images = self
            .panes
            .iter()
            .map(|p| p.image_paths.len())
            .max()
            .unwrap_or(0);
        if max_images <= 1 {
            return;
        }

        let current_idx = self.panes.first().map_or(0, |p| p.current_index);
        let mut slider_target = None;
        let mut slider_released = false;

        egui::TopBottomPanel::bottom("nav").show(ctx, |ui| {
            let mut idx = current_idx;
            let max = max_images - 1;

            // Custom slider: accent handle + two-tone rail (full width)
            let slider_width = ui.available_width();
            let thickness = ui
                .text_style_height(&egui::TextStyle::Body)
                .max(ui.spacing().interact_size.y);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(slider_width, thickness), egui::Sense::drag());

            let handle_radius = rect.height() / 2.5;
            let rail_radius = 4.0_f32;
            let cy = rect.center().y;
            let handle_range =
                (rect.left() + handle_radius)..=(rect.right() - handle_radius);

            // Handle dragging
            if let Some(pos) = response.interact_pointer_pos() {
                let usable = rect.x_range().shrink(handle_radius);
                let drag_t =
                    ((pos.x - usable.min) / (usable.max - usable.min)).clamp(0.0, 1.0);
                idx = (max as f32 * drag_t).round() as usize;
                if idx != current_idx {
                    slider_target = Some(idx);
                }
            }
            if response.drag_stopped() {
                slider_released = true;
            }

            // Paint: unfilled rail → filled rail → handle (back to front)
            let rail = egui::Rect::from_min_max(
                egui::pos2(rect.left(), cy - rail_radius),
                egui::pos2(rect.right(), cy + rail_radius),
            );
            let t = if max > 0 { idx as f32 / max as f32 } else { 0.0 };
            let handle_x = egui::lerp(handle_range, t);

            ui.painter()
                .rect_filled(rail, rail_radius, egui::Color32::from_gray(60));
            let filled =
                egui::Rect::from_min_max(rail.min, egui::pos2(handle_x, rail.max.y));
            ui.painter()
                .rect_filled(filled, rail_radius, self.theme.accent);
            ui.painter().circle(
                egui::pos2(handle_x, cy),
                handle_radius,
                self.theme.accent,
                egui::Stroke::NONE,
            );
        });

        if let Some(idx) = slider_target {
            for pane in &mut self.panes {
                let clamped = idx.min(pane.image_paths.len().saturating_sub(1));
                if clamped != pane.current_index {
                    pane.current_index = clamped;
                    pane.zoom = 1.0;
                    pane.pan = egui::Vec2::ZERO;

                    // Try the sliding window cache first (free if already cached)
                    let found_in_cache = pane
                        .cache
                        .as_ref()
                        .and_then(|c| c.current_texture_for(clamped));

                    if let Some(tex) = found_in_cache {
                        pane.current_texture = Some(tex);
                        self.perf.record_image_load(0.0);
                    } else if let Some(loader) = &mut pane.slider_loader {
                        // Throttled sync decode — like iced, only decode when
                        // enough time has passed. Previous texture stays on screen.
                        if loader.should_load() {
                            pane.load_sync(ctx);
                            self.perf.record_image_load(0.0);
                        }
                    }
                }
            }
            ctx.request_repaint();
        }

        if slider_released {
            for pane in &mut self.panes {
                if let Some(loader) = &mut pane.slider_loader {
                    loader.cancel();
                }
                if let Some(cache) = &mut pane.cache {
                    cache.jump_to(pane.current_index, &pane.image_paths);
                    if let Some(t) = cache.current_texture_for(pane.current_index) {
                        pane.current_texture = Some(t);
                    }
                }
            }
        }
    }

    fn show_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_gray(20)))
            .show(ctx, |ui| {
                if self.panes.len() <= 1 {
                    if let Some(pane) = self.panes.first_mut() {
                        pane.show_content(ui);
                    }
                } else {
                    let available = ui.available_rect_before_wrap();
                    let divider_w = 4.0;
                    let grab_w = 12.0; // wider hit area for easy grabbing
                    let left_w = (available.width() - divider_w) * self.divider_fraction;

                    let left_rect = egui::Rect::from_min_size(
                        available.min,
                        egui::vec2(left_w, available.height()),
                    );
                    let right_rect = egui::Rect::from_min_size(
                        egui::pos2(available.min.x + left_w + divider_w, available.min.y),
                        egui::vec2(available.width() - left_w - divider_w, available.height()),
                    );

                    // Divider interaction — wide grab area centered on the visual line
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

                    // Double-click resets to 50/50
                    if divider_response.double_clicked() {
                        self.divider_fraction = 0.5;
                    }

                    // Resize cursor when hovering or dragging
                    if divider_response.hovered() || divider_response.dragged() {
                        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }

                    // Visual divider line — highlighted when interacting
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

                    let (first, rest) = self.panes.split_at_mut(1);
                    ui.allocate_new_ui(
                        egui::UiBuilder::new().max_rect(left_rect),
                        |ui| first[0].show_content(ui),
                    );
                    ui.allocate_new_ui(
                        egui::UiBuilder::new().max_rect(right_rect),
                        |ui| rest[0].show_content(ui),
                    );
                }
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
            menu::show_footer(ctx, &self.panes);
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
