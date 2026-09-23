//! Culling actions: moving the current image to the trash, starring it,
//! and showing only the starred images.
//!
//! The trash move itself lives in `crate::trash_bin` and the star file in
//! `crate::stars`. This module decides which panes take part, keeps every
//! pane that lists the file in step, and shows the outcome to the user.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::pane::Pane;
use crate::trash_bin;

use super::{App, DualPaneMode};

/// How long the outcome stays on screen.
const TOAST_DURATION: Duration = Duration::from_millis(2500);

/// How long the star after S stays fully visible, then how long it takes
/// to fade out.
const STAR_FLASH_HOLD: Duration = Duration::from_millis(400);
const STAR_FLASH_FADE: Duration = Duration::from_millis(400);
/// Size of the star glyph after S.
const STAR_FLASH_SIZE: f32 = 28.0;

/// The star painted for a moment in the corner of a pane after S, when the
/// footer is not on screen to show the change.
#[derive(Clone)]
pub(crate) struct StarFlash {
    /// The panes whose image S changed.
    panes: Vec<usize>,
    /// The images got a star (true) or lost it.
    starred: bool,
    shown_at: Instant,
}

/// A short message painted over the image area, e.g. "Moved to Trash".
pub(crate) struct Toast {
    text: String,
    shown_at: Instant,
    is_error: bool,
}

impl App {
    /// Move the current image of every active pane to the trash. In synced
    /// dual-pane mode both panes show the same file, so it is moved once.
    /// On Windows, a location without a Recycle Bin gets a confirmation
    /// first because the shell would delete permanently.
    pub(super) fn trash_current_images(&mut self, ctx: &egui::Context) {
        let use_selection = self.dual_pane_mode == DualPaneMode::Independent;
        let mut paths: Vec<PathBuf> = Vec::new();
        for pane in &self.panes {
            if use_selection && !pane.selected {
                continue;
            }
            if let Some(path) = pane.image_paths.get(pane.current_index) {
                if !paths.contains(path) {
                    paths.push(path.clone());
                }
            }
        }
        self.trash_or_confirm(paths, ctx);
    }

    /// The footer button of one pane: move that pane's current image to
    /// the trash, whatever the pane selection is.
    pub(super) fn trash_pane_image(&mut self, pane_idx: usize, ctx: &egui::Context) {
        let Some(path) = self
            .panes
            .get(pane_idx)
            .and_then(|p| p.image_paths.get(p.current_index).cloned())
        else {
            return;
        };
        self.trash_or_confirm(vec![path], ctx);
    }

    fn trash_or_confirm(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        if paths.is_empty() {
            return;
        }
        if paths.iter().any(|p| trash_bin::lacks_recycle_bin(p)) {
            self.pending_permanent_delete = Some(paths);
            return;
        }
        self.trash_paths(paths, ctx);
    }

    /// Move `paths` to the trash and drop each one that succeeded from every
    /// pane that lists it, whichever pane triggered the action. The pane
    /// showing the file does the move through `Pane::remove_current`, so a
    /// failed move leaves it untouched. The other panes drop the path from
    /// their list, or from the whole list a filtered pane keeps aside. The
    /// file's star entry goes too.
    pub(super) fn trash_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        for path in paths {
            let showing = self
                .panes
                .iter()
                .position(|p| p.image_paths.get(p.current_index) == Some(&path));
            let result = match showing {
                Some(i) => self.panes[i]
                    .remove_current(ctx, trash_bin::move_to_trash)
                    .map(|_| ()),
                None => trash_bin::move_to_trash(&path),
            };
            match result {
                Ok(()) => {
                    log::info!("Moved to trash: {}", path.display());
                    for pane in &mut self.panes {
                        pane.remove_path(&path, ctx);
                    }
                    self.stars.forget(&path);
                    self.show_toast(format!("Moved to Trash: {}", file_name(&path)), false);
                }
                Err(e) => {
                    log::error!("Could not move {} to trash: {}", path.display(), e);
                    self.show_toast(
                        format!("Could not move {} to Trash: {}", file_name(&path), e),
                        true,
                    );
                }
            }
        }
        self.refresh_starred_panes();
        ctx.request_repaint();
    }

    /// S: star the current image of every active pane, the panes Delete
    /// acts on. A file shown in both panes counts once. If any of them has
    /// no star, all of them get one. Otherwise all of them lose it.
    pub(super) fn toggle_star_current_images(&mut self, ctx: &egui::Context) {
        let use_selection = self.dual_pane_mode == DualPaneMode::Independent;
        let panes: Vec<usize> = (0..self.panes.len())
            .filter(|&i| !use_selection || self.panes[i].selected)
            .collect();
        self.toggle_star(panes, true, ctx);
    }

    /// The metadata panel's star button: that pane's image, whatever the
    /// pane selection is. The button shows the change itself, so no star
    /// flashes over the image.
    pub(super) fn toggle_star_pane_image(&mut self, pane_idx: usize, ctx: &egui::Context) {
        self.toggle_star(vec![pane_idx], false, ctx);
    }

    /// `flash`: when the footer is hidden, show the change with a star in
    /// the corner of each pane, since nothing else on screen would.
    fn toggle_star(&mut self, pane_indices: Vec<usize>, flash: bool, ctx: &egui::Context) {
        let mut panes = Vec::new();
        let mut paths: Vec<PathBuf> = Vec::new();
        for i in pane_indices {
            let Some(path) = self.panes.get(i).and_then(|p| p.image_paths.get(p.current_index)) else {
                continue;
            };
            panes.push(i);
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
        if paths.is_empty() {
            return;
        }
        let star = paths.iter().any(|p| !self.stars.is_starred(p));
        let mut changed = false;
        for path in &paths {
            if self.stars.is_starred(path) == star {
                continue;
            }
            let result = if star {
                // The size lets a later load tell this file from another
                // one that takes its name.
                std::fs::metadata(path)
                    .map_err(|e| format!("Could not star {}: {e}", file_name(path)))
                    .and_then(|m| self.stars.star(path, m.len()))
            } else {
                self.stars.unstar(path)
            };
            match result {
                Ok(()) => changed = true,
                Err(message) => self.show_toast(message, true),
            }
        }
        self.refresh_starred_panes();
        if flash && changed && !self.footer_visible(ctx) {
            self.star_flash = Some(StarFlash { panes, starred: star, shown_at: Instant::now() });
        }
        ctx.request_repaint();
    }

    /// F: turn the starred-only filter on or off in every active pane. On
    /// if no active pane has it on, off otherwise.
    pub(super) fn toggle_starred_only(&mut self, ctx: &egui::Context) {
        let use_selection = self.dual_pane_mode == DualPaneMode::Independent;
        let is_active = |p: &Pane| (!use_selection || p.selected) && !p.image_paths.is_empty();
        let turn_on = !self.panes.iter().any(|p| is_active(p) && p.starred_only());
        let mut active = 0;
        let mut refused = Vec::new();
        for (i, pane) in self.panes.iter_mut().enumerate() {
            if !is_active(pane) {
                continue;
            }
            active += 1;
            if !pane.set_starred_only(turn_on, &self.stars, ctx) {
                refused.push(i);
            }
        }
        if refused.is_empty() {
            return;
        }
        let message = if refused.len() == active {
            "No starred images in this folder".to_string()
        } else {
            format!("No starred images in pane {}", refused[0] + 1)
        };
        self.show_toast(message, false);
        ctx.request_repaint();
    }

    /// The Edit menu's star row reads "Unstar" when every image S acts on
    /// has a star. The View menu's switch shows whether the filter is on
    /// in an active pane.
    pub(super) fn star_menu_state(&self) -> (bool, bool) {
        let use_selection = self.dual_pane_mode == DualPaneMode::Independent;
        let active: Vec<&Pane> = self
            .panes
            .iter()
            .filter(|p| (!use_selection || p.selected) && !p.image_paths.is_empty())
            .collect();
        let all_starred = !active.is_empty() && active.iter().all(|p| p.is_current_starred());
        let starred_only = active.iter().any(|p| p.starred_only());
        (all_starred, starred_only)
    }

    pub(super) fn refresh_starred_panes(&mut self) {
        for pane in &mut self.panes {
            pane.refresh_starred(&self.stars);
        }
    }

    /// Show the saves that failed since the last frame and take their
    /// stars back on screen.
    pub(super) fn show_star_failures(&mut self) {
        let failures = self.stars.take_failures();
        if failures.is_empty() {
            return;
        }
        self.refresh_starred_panes();
        for message in failures {
            self.show_toast(message, true);
        }
    }

    /// The flash after S for the pane at `pane_idx`: whether the image got
    /// a star, and the opacity now. Drops the flash once it has faded.
    pub(super) fn star_flash_for(&mut self, pane_idx: usize) -> Option<(bool, f32)> {
        let flash = self.star_flash.as_ref()?;
        let elapsed = flash.shown_at.elapsed();
        if elapsed >= STAR_FLASH_HOLD + STAR_FLASH_FADE {
            self.star_flash = None;
            return None;
        }
        if !flash.panes.contains(&pane_idx) {
            return None;
        }
        let fade = elapsed.saturating_sub(STAR_FLASH_HOLD).as_secs_f32() / STAR_FLASH_FADE.as_secs_f32();
        Some((flash.starred, 1.0 - fade.min(1.0)))
    }

    pub(super) fn show_toast(&mut self, text: String, is_error: bool) {
        self.toast = Some(Toast {
            text,
            shown_at: Instant::now(),
            is_error,
        });
    }

    /// Paint the toast at the bottom center of the window until it expires.
    pub(super) fn paint_toast(&mut self, ctx: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        let elapsed = toast.shown_at.elapsed();
        if elapsed >= TOAST_DURATION {
            self.toast = None;
            return;
        }
        let fade_start = TOAST_DURATION.as_secs_f32() - 0.4;
        let alpha = ((TOAST_DURATION.as_secs_f32() - elapsed.as_secs_f32()) / 0.4).clamp(0.0, 1.0);
        let alpha = if elapsed.as_secs_f32() < fade_start { 1.0 } else { alpha };

        let text_color = egui::Color32::from_rgba_unmultiplied(230, 230, 230, (alpha * 255.0) as u8);
        let bg = if toast.is_error {
            egui::Color32::from_rgba_unmultiplied(120, 40, 40, (alpha * 220.0) as u8)
        } else {
            egui::Color32::from_rgba_unmultiplied(20, 20, 20, (alpha * 200.0) as u8)
        };
        let text = toast.text.clone();

        egui::Area::new(egui::Id::new("culling_toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -72.0])
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(bg)
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(text).size(14.0).color(text_color));
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    /// Confirmation shown when the trash move would be a permanent delete
    /// (Windows network shares and removable media). Enter confirms,
    /// Escape cancels.
    pub(super) fn show_permanent_delete_modal(&mut self, ctx: &egui::Context) {
        let Some(paths) = self.pending_permanent_delete.clone() else {
            return;
        };
        let (backdrop, card_bg, card_stroke) =
            (self.theme.backdrop, self.theme.card_bg, self.theme.card_stroke);
        let screen = ctx.screen_rect();
        let mut decision: Option<bool> = None;

        egui::Area::new(egui::Id::new("permanent_delete_backdrop"))
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let response = ui.allocate_response(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(screen, 0.0, backdrop);
                if response.clicked() {
                    decision = Some(false);
                }
            });

        egui::Area::new(egui::Id::new("permanent_delete_modal"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                let max_width = (screen.width() * 0.8).min(460.0);
                egui::Frame::default()
                    .fill(card_bg)
                    .stroke(egui::Stroke::new(1.0_f32, card_stroke))
                    .corner_radius(8.0)
                    .inner_margin(20.0)
                    .show(ui, |ui| {
                        ui.set_max_width(max_width);
                        ui.label(egui::RichText::new("No Recycle Bin here").size(18.0).strong());
                        ui.add_space(10.0);
                        ui.label(
                            "This location has no Recycle Bin, so Windows will delete the \
                             file permanently instead of moving it to the trash.",
                        );
                        ui.add_space(8.0);
                        for path in &paths {
                            ui.label(egui::RichText::new(file_name(path)).monospace());
                        }
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if ui.button("Cancel").clicked() {
                                decision = Some(false);
                            }
                            ui.add_space(8.0);
                            let delete = egui::Button::new(
                                egui::RichText::new("Delete permanently").color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(160, 50, 50));
                            if ui.add(delete).clicked() {
                                decision = Some(true);
                            }
                        });
                    });
            });

        let (enter, escape) = ctx.input(|i| {
            (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape))
        });
        if enter {
            decision = Some(true);
        } else if escape {
            decision = Some(false);
        }

        match decision {
            Some(true) => {
                self.pending_permanent_delete = None;
                self.trash_paths(paths, ctx);
            }
            Some(false) => self.pending_permanent_delete = None,
            None => {}
        }
    }
}

/// Paint the star after S in the bottom right corner of `rect`: filled in
/// the accent colour when the image got a star, an outline when it lost
/// it, on a dark backing so it shows on any image.
pub(super) fn paint_star_flash(
    ctx: &egui::Context,
    rect: egui::Rect,
    starred: bool,
    alpha: f32,
    accent: egui::Color32,
) {
    let (glyph, color) = if starred { ("★", accent) } else { ("☆", egui::Color32::from_gray(220)) };
    let color = color.gamma_multiply(alpha);
    let backing = egui::Color32::from_black_alpha((140.0 * alpha) as u8);
    let margin = 16.0;
    let side = STAR_FLASH_SIZE + 12.0;
    let backing_rect = egui::Rect::from_min_size(
        rect.right_bottom() - egui::vec2(margin + side, margin + side),
        egui::vec2(side, side),
    );
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("star_flash"),
    ));
    painter.rect_filled(backing_rect, 6.0, backing);
    painter.text(
        backing_rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(STAR_FLASH_SIZE),
        color,
    );
    ctx.request_repaint();
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
