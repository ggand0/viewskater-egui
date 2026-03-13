use std::path::PathBuf;

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::menu::toggle_switch;
use crate::theme::UiTheme;

/// Custom slider with accent-colored handle and two-tone rail.
///
/// egui's built-in Slider ties the idle handle color to the rail
/// background (`widgets.inactive.bg_fill`), making it impossible to
/// theme them independently.  This draws everything from scratch.
fn accent_slider(
    ui: &mut egui::Ui,
    value: &mut usize,
    range: std::ops::RangeInclusive<usize>,
    theme: &UiTheme,
) {
    let lo = *range.start();
    let hi = *range.end();

    let slider_width = ui.spacing().slider_width;
    let thickness = ui
        .text_style_height(&egui::TextStyle::Body)
        .max(ui.spacing().interact_size.y);

    // Allocate rail + handle area, then value text to the right.
    let desired = egui::vec2(slider_width, thickness);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::drag());

    // Handle dragging.
    if let Some(pos) = response.interact_pointer_pos() {
        let handle_radius = rect.height() / 2.5;
        let usable = rect.x_range().shrink(handle_radius);
        let t = ((pos.x - usable.min) / (usable.max - usable.min)).clamp(0.0, 1.0);
        *value = lo + ((hi - lo) as f64 * t as f64).round() as usize;
    }

    // Paint.
    let handle_radius = rect.height() / 2.5;
    let rail_radius = 4.0_f32;
    let cy = rect.center().y;
    let rail = egui::Rect::from_min_max(
        egui::pos2(rect.left(), cy - rail_radius),
        egui::pos2(rect.right(), cy + rail_radius),
    );

    let t = if hi > lo {
        (*value - lo) as f32 / (hi - lo) as f32
    } else {
        0.0
    };
    let handle_x = egui::lerp(
        (rect.left() + handle_radius)..=(rect.right() - handle_radius),
        t,
    );

    // Unfilled rail (full width, painted first).
    ui.painter()
        .rect_filled(rail, rail_radius, egui::Color32::from_gray(60));
    // Filled rail (left edge → handle center).
    let filled = egui::Rect::from_min_max(rail.min, egui::pos2(handle_x, rail.max.y));
    ui.painter().rect_filled(filled, rail_radius, theme.accent);
    // Handle circle.
    let center = egui::pos2(handle_x, cy);
    ui.painter().circle(
        center,
        handle_radius,
        theme.accent,
        egui::Stroke::NONE,
    );

    // Value text to the right.
    let text_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() + ui.spacing().item_spacing.x, rect.top()),
        egui::vec2(40.0, rect.height()),
    );
    ui.put(
        text_rect,
        egui::Label::new(
            egui::RichText::new(format!("{value}"))
                .monospace()
                .color(egui::Color32::from_gray(200)),
        ),
    );
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub show_footer: bool,
    pub show_fps: bool,
    pub show_cache_overlay: bool,
    pub cache_count: usize,
    pub lru_capacity: usize,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_footer: true,
            show_fps: true,
            show_cache_overlay: false,
            cache_count: 5,
            lru_capacity: 50,
        }
    }
}

impl AppSettings {
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("viewskater-egui").join("settings.yaml"))
    }

    pub fn load() -> Self {
        let settings = Self::config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_yaml::from_str(&s).ok())
            .unwrap_or_default();
        log::debug!("Loaded settings: {:?}", settings);
        settings
    }

    pub fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match serde_yaml::to_string(self) {
                Ok(yaml) => {
                    if let Err(e) = std::fs::write(&path, yaml) {
                        log::error!("Failed to save settings to {}: {}", path.display(), e);
                    } else {
                        log::debug!("Settings saved to {}", path.display());
                    }
                }
                Err(e) => log::error!("Failed to serialize settings: {}", e),
            }
        }
    }
}

/// Show the settings modal. Returns true if performance settings (cache_count or lru_capacity) changed.
pub fn show_settings_modal(
    ctx: &egui::Context,
    settings: &mut AppSettings,
    show: &mut bool,
    theme: &UiTheme,
) -> bool {
    if !*show {
        return false;
    }

    let prev_cache_count = settings.cache_count;
    let prev_lru_capacity = settings.lru_capacity;

    // Semi-transparent backdrop
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("settings_backdrop"))
        .fixed_pos(screen.min)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let response = ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter().rect_filled(screen, 0.0, theme.backdrop);
            if response.clicked() {
                *show = false;
            }
        });

    // Modal card
    egui::Area::new(egui::Id::new("settings_modal"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(theme.card_bg)
                .stroke(egui::Stroke::new(1.0, theme.card_stroke))
                .corner_radius(8.0)
                .inner_margin(20.0)
                .show(ui, |ui| {
                    ui.set_width(360.0);

                    // Title
                    ui.label(egui::RichText::new("Preferences").size(20.0).strong());
                    ui.separator();
                    ui.add_space(8.0);

                    // Display section
                    ui.label(
                        egui::RichText::new("Display")
                            .size(14.0)
                            .color(theme.heading),
                    );
                    ui.add_space(4.0);
                    egui::Frame::default()
                        .fill(theme.section_bg)
                        .corner_radius(6.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                toggle_switch(ui, &mut settings.show_footer, "Footer", theme);
                            });
                            ui.horizontal(|ui| {
                                toggle_switch(ui, &mut settings.show_fps, "FPS Overlay", theme);
                            });
                            ui.horizontal(|ui| {
                                toggle_switch(
                                    ui,
                                    &mut settings.show_cache_overlay,
                                    "Cache Overlay",
                                    theme,
                                );
                            });
                        });

                    ui.add_space(12.0);

                    // Performance section
                    ui.label(
                        egui::RichText::new("Performance")
                            .size(14.0)
                            .color(theme.heading),
                    );
                    ui.add_space(4.0);
                    egui::Frame::default()
                        .fill(theme.section_bg)
                        .corner_radius(6.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Cache Size");
                                accent_slider(ui, &mut settings.cache_count, 1..=20, theme);
                            });
                            ui.horizontal(|ui| {
                                ui.label("LRU Capacity");
                                accent_slider(ui, &mut settings.lru_capacity, 10..=200, theme);
                            });
                        });
                });
        });

    // Escape to close
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *show = false;
    }

    settings.cache_count != prev_cache_count || settings.lru_capacity != prev_lru_capacity
}
