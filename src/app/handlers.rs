use eframe::egui;

use crate::menu::MenuAction;
use crate::pane::Pane;

use super::{App, DualPaneMode, SliderResult};

impl App {
    pub(super) fn set_single_pane(&mut self) {
        if self.panes.len() >= 2 {
            self.panes.truncate(1);
        }
    }

    pub(super) fn set_dual_pane(&mut self, ctx: &egui::Context) {
        if self.panes.len() < 2 {
            let mut pane = Pane::new(self.settings.cache_count, self.settings.lru_budget_mb);
            if !self.panes[0].image_paths.is_empty() {
                if let Some(dir) = self.panes[0].image_paths[0].parent() {
                    pane.open_path(dir, ctx);
                    pane.jump_to(self.panes[0].current_index, ctx);
                }
            }
            self.panes.push(pane);
        }
    }

    pub(super) fn open_folder_dialog(&mut self, pane_idx: usize, ctx: &egui::Context) {
        if let Some(pane) = self.panes.get_mut(pane_idx) {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                pane.open_path(&dir, ctx);
            }
        }
    }

    pub(super) fn open_file_dialog(&mut self, pane_idx: usize, ctx: &egui::Context) {
        if let Some(pane) = self.panes.get_mut(pane_idx) {
            if let Some(file) = rfd::FileDialog::new()
                .add_filter("Images", &["jpg", "jpeg", "png", "bmp", "webp", "gif", "tiff", "tif", "qoi", "tga"])
                .pick_file()
            {
                pane.open_path(&file, ctx);
            }
        }
    }

    pub(super) fn close_images(&mut self) {
        for pane in &mut self.panes {
            pane.close();
        }
    }

    pub(super) fn handle_menu_action(&mut self, action: MenuAction, ctx: &egui::Context) {
        match action {
            MenuAction::None => {}
            MenuAction::OpenFolder(idx) => self.open_folder_dialog(idx, ctx),
            MenuAction::OpenFile(idx) => self.open_file_dialog(idx, ctx),
            MenuAction::Close => self.close_images(),
            MenuAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            MenuAction::SetSinglePane => self.set_single_pane(),
            MenuAction::SetDualPane => {
                self.set_dual_pane(ctx);
                self.dual_pane_mode = DualPaneMode::Synced;
            }
            MenuAction::SetDualPaneIndependent => {
                self.set_dual_pane(ctx);
                self.dual_pane_mode = DualPaneMode::Independent;
            }
            MenuAction::ResetZoom => {
                for pane in &mut self.panes {
                    pane.zoom = 1.0;
                    pane.pan = egui::Vec2::ZERO;
                }
            }
            MenuAction::ToggleFullscreen => self.toggle_fullscreen(ctx),
            MenuAction::ShowAbout => self.show_about = true,
            MenuAction::ShowSettings => self.show_settings = true,
            MenuAction::ShowLogs => {
                let log_dir = crate::file_io::get_log_directory();
                let _ = std::fs::create_dir_all(&log_dir);
                crate::file_io::open_in_file_explorer(&log_dir.to_string_lossy());
            }
            MenuAction::ExportDebugLogs => {
                crate::file_io::export_and_open_debug_logs(&self.log_buffer);
            }
        }
    }

    /// Apply slider result to all panes (synced mode).
    pub(super) fn apply_slider_result_all(&mut self, result: SliderResult, ctx: &egui::Context) {
        if let Some(idx) = result.target {
            for pane in &mut self.panes {
                if pane.apply_slider_target(idx, ctx) {
                    self.perf.record_image_load();
                }
            }
            ctx.request_repaint();
        }

        if result.released {
            for pane in &mut self.panes {
                pane.apply_slider_release();
            }
        }
    }

    /// Apply slider result to a single pane (independent mode).
    pub(super) fn apply_slider_result_one(
        &mut self,
        pane_idx: usize,
        result: SliderResult,
        ctx: &egui::Context,
    ) {
        if let Some(idx) = result.target {
            if let Some(pane) = self.panes.get_mut(pane_idx) {
                if pane.apply_slider_target(idx, ctx) {
                    self.perf.record_image_load();
                }
            }
            ctx.request_repaint();
        }

        if result.released {
            if let Some(pane) = self.panes.get_mut(pane_idx) {
                pane.apply_slider_release();
            }
        }
    }

    pub(super) fn apply_settings_to_caches(&mut self) {
        for pane in &mut self.panes {
            if let Some(cache) = &mut pane.cache {
                cache.set_cache_count(
                    self.settings.cache_count,
                    pane.current_index,
                    &pane.image_paths,
                );
            }
            pane.decode_cache.set_budget_mb(self.settings.lru_budget_mb);
            pane.cache_count = self.settings.cache_count;
            pane.lru_budget_mb = self.settings.lru_budget_mb;
        }
    }

    pub(super) fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.is_fullscreen = !self.is_fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
    }

    pub(super) fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let (home, end, shift, nav_right_pressed, nav_left_pressed,
             nav_right_held, nav_left_held, set_single, set_dual,
             set_independent, select_pane1, select_pane2,
             toggle_footer, open_folder, open_file, close, quit,
             toggle_fullscreen, escape) =
            ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Home),
                    i.key_pressed(egui::Key::End),
                    i.modifiers.shift,
                    i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::D),
                    i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::A),
                    i.key_down(egui::Key::ArrowRight) || i.key_down(egui::Key::D),
                    i.key_down(egui::Key::ArrowLeft) || i.key_down(egui::Key::A),
                    i.key_pressed(egui::Key::Num1) && i.modifiers.command,
                    i.key_pressed(egui::Key::Num2) && i.modifiers.command,
                    i.key_pressed(egui::Key::Num3) && i.modifiers.command,
                    i.key_pressed(egui::Key::Num1) && !i.modifiers.command,
                    i.key_pressed(egui::Key::Num2) && !i.modifiers.command,
                    i.key_pressed(egui::Key::Tab),
                    i.key_pressed(egui::Key::O) && i.modifiers.command && i.modifiers.shift,
                    i.key_pressed(egui::Key::O) && i.modifiers.command && !i.modifiers.shift,
                    i.key_pressed(egui::Key::W) && i.modifiers.command,
                    i.key_pressed(egui::Key::Q) && i.modifiers.command,
                    i.key_pressed(egui::Key::F11),
                    i.key_pressed(egui::Key::Escape),
                )
            });

        if quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if toggle_fullscreen {
            self.toggle_fullscreen(ctx);
            return;
        }
        if escape && self.is_fullscreen {
            self.toggle_fullscreen(ctx);
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

        // Pane selection toggle (bare 1/2 keys, only in independent dual-pane mode)
        if self.panes.len() >= 2 && self.dual_pane_mode == DualPaneMode::Independent {
            if select_pane1 {
                self.panes[0].selected = !self.panes[0].selected;
                return;
            }
            if select_pane2 {
                self.panes[1].selected = !self.panes[1].selected;
                return;
            }
        }

        // Skate mode (Shift held): advance every frame while key is down
        // Normal mode: advance once per key press/repeat event (~30hz)
        let nav_right = if shift { nav_right_held } else { nav_right_pressed };
        let nav_left = if shift { nav_left_held } else { nav_left_pressed };

        if set_single && self.panes.len() >= 2 {
            self.set_single_pane();
            return;
        }
        if set_dual {
            self.set_dual_pane(ctx);
            self.dual_pane_mode = DualPaneMode::Synced;
            return;
        }
        if set_independent {
            self.set_dual_pane(ctx);
            self.dual_pane_mode = DualPaneMode::Independent;
            return;
        }

        // In independent mode, only navigate selected panes;
        // in synced mode, navigate all panes.
        let use_selection = self.dual_pane_mode == DualPaneMode::Independent;
        let is_active = |p: &Pane| !use_selection || p.selected;

        if home {
            for pane in &mut self.panes {
                if is_active(pane) {
                    pane.jump_to(0, ctx);
                }
            }
            self.perf.record_image_load();
        } else if end {
            for pane in &mut self.panes {
                if is_active(pane) {
                    let last = pane.image_paths.len().saturating_sub(1);
                    pane.jump_to(last, ctx);
                }
            }
            self.perf.record_image_load();
        } else if nav_right {
            let all_ready = self.panes.iter().all(|p| {
                !is_active(p) || p.image_paths.is_empty() || p.is_next_cached(1)
            });
            if all_ready {
                let any_advanced = self.panes.iter_mut().fold(false, |acc, p| {
                    if is_active(p) { p.navigate(1) || acc } else { acc }
                });
                if any_advanced {
                    self.perf.record_image_load();
                }
            }
            let any_can = self.panes.iter().any(|p| is_active(p) && p.can_navigate_forward());
            if any_can {
                ctx.request_repaint();
            }
        } else if nav_left {
            let all_ready = self.panes.iter().all(|p| {
                !is_active(p) || p.image_paths.is_empty() || p.is_next_cached(-1)
            });
            if all_ready {
                let any_advanced = self.panes.iter_mut().fold(false, |acc, p| {
                    if is_active(p) { p.navigate(-1) || acc } else { acc }
                });
                if any_advanced {
                    self.perf.record_image_load();
                }
            }
            let any_can = self.panes.iter().any(|p| is_active(p) && p.can_navigate_backward());
            if any_can {
                ctx.request_repaint();
            }
        }
    }

    pub(super) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
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
}
