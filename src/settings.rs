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
    default: usize,
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
    let (rect, response) =
        ui.allocate_exact_size(desired, egui::Sense::click_and_drag());

    // Double-click resets to default.
    if response.double_clicked() {
        *value = default;
    } else if let Some(pos) = response.interact_pointer_pos() {
        // Handle dragging.
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

/// Custom radio row for GPU memory mode: an accent-colored circle indicator,
/// a primary label, and a muted description on the next line.
fn gpu_memory_radio(
    ui: &mut egui::Ui,
    current: &mut GpuMemoryMode,
    value: GpuMemoryMode,
    label: &str,
    description: &str,
    theme: &UiTheme,
) {
    let selected = *current == value;
    ui.horizontal(|ui| {
        let radius = 7.0_f32;
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(radius * 2.0 + 4.0, radius * 2.0 + 4.0),
            egui::Sense::click(),
        );
        let center = rect.center();
        ui.painter().circle_stroke(
            center,
            radius,
            egui::Stroke::new(1.5, egui::Color32::from_gray(140)),
        );
        if selected {
            ui.painter()
                .circle_filled(center, radius - 3.0, theme.accent);
        }
        if response.clicked() {
            *current = value;
        }

        ui.vertical(|ui| {
            let label_response = ui.add(
                egui::Label::new(egui::RichText::new(label).size(13.0))
                    .sense(egui::Sense::click()),
            );
            if label_response.clicked() {
                *current = value;
            }
            ui.label(
                egui::RichText::new(description)
                    .size(11.0)
                    .color(theme.muted),
            );
        });
    });
    ui.add_space(4.0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuMemoryMode {
    /// gpu_allocator default (~256 MB blocks). Highest navigation speed,
    /// largest GPU memory footprint.
    Performance,
    /// 64 MB device / 32 MB host blocks. Recommended balance.
    Balanced,
    /// 8 MB device / 4 MB host blocks. Lowest GPU memory, but a 4K texture
    /// no longer fits in a single block — degrades navigation performance.
    LowMemory,
}

impl Default for GpuMemoryMode {
    fn default() -> Self {
        Self::Balanced
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub show_footer: bool,
    pub show_fps: bool,
    pub show_cache_overlay: bool,
    pub sync_zoom_pan: bool,
    pub cache_count: usize,
    pub lru_budget_mb: usize,
    pub gpu_memory_mode: GpuMemoryMode,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_footer: true,
            show_fps: true,
            show_cache_overlay: false,
            sync_zoom_pan: true,
            cache_count: 5,
            lru_budget_mb: 1024,
            gpu_memory_mode: GpuMemoryMode::default(),
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

/// Show the settings modal. Returns true if performance settings (cache_count or lru_budget_mb) changed.
pub fn show_settings_modal(
    ctx: &egui::Context,
    settings: &mut AppSettings,
    show: &mut bool,
    theme: &UiTheme,
) -> bool {
    if !*show {
        return false;
    }

    // Snapshot at start of frame; if anything changes we save immediately
    // and stamp the save time so the "Saved" indicator can fade in.
    let snapshot = settings.clone();

    let saved_at_id = egui::Id::new("settings_saved_at");
    let now = ctx.input(|i| i.time);

    let prev_cache_count = settings.cache_count;
    let prev_lru_budget = settings.lru_budget_mb;

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

    // Cap the modal height so it fits even on very short windows.
    let max_modal_height = (screen.height() - 60.0).max(200.0);

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
                    ui.set_max_height(max_modal_height);

                    // Title (outside the scroll area so it stays pinned)
                    ui.label(egui::RichText::new("Preferences").size(20.0).strong());
                    ui.separator();
                    ui.add_space(8.0);

                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .show(ui, |ui| {

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
                            ui.horizontal(|ui| {
                                toggle_switch(
                                    ui,
                                    &mut settings.sync_zoom_pan,
                                    "Sync Zoom/Pan",
                                    theme,
                                );
                            });
                        });

                    ui.add_space(12.0);

                    // Graphics section
                    ui.label(
                        egui::RichText::new("Graphics")
                            .size(14.0)
                            .color(theme.heading),
                    );
                    ui.add_space(2.0);
                    egui::Frame::default()
                        .fill(theme.section_bg)
                        .corner_radius(6.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("GPU Memory Mode")
                                    .size(12.0)
                                    .color(theme.muted),
                            );
                            ui.add_space(4.0);
                            gpu_memory_radio(
                                ui,
                                &mut settings.gpu_memory_mode,
                                GpuMemoryMode::Performance,
                                "Performance",
                                "Highest nav speed, largest GPU memory",
                                theme,
                            );
                            gpu_memory_radio(
                                ui,
                                &mut settings.gpu_memory_mode,
                                GpuMemoryMode::Balanced,
                                "Balanced",
                                "Recommended for most users",
                                theme,
                            );
                            gpu_memory_radio(
                                ui,
                                &mut settings.gpu_memory_mode,
                                GpuMemoryMode::LowMemory,
                                "Low Memory",
                                "Lowest GPU memory, slower navigation",
                                theme,
                            );
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new("⚠ Restart required to apply")
                                    .size(11.0)
                                    .color(theme.muted),
                            );
                        });

                    ui.add_space(12.0);

                    // Performance section
                    ui.label(
                        egui::RichText::new("Performance")
                            .size(14.0)
                            .color(theme.heading),
                    );
                    ui.label(
                        egui::RichText::new("Double-click to reset")
                            .size(11.0)
                            .color(theme.muted),
                    );
                    ui.add_space(2.0);
                    egui::Frame::default()
                        .fill(theme.section_bg)
                        .corner_radius(6.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Cache Size");
                                let defaults = AppSettings::default();
                                accent_slider(ui, &mut settings.cache_count, 1..=20, defaults.cache_count, theme);
                            });
                            ui.horizontal(|ui| {
                                ui.label("LRU Budget (MB)");
                                let defaults = AppSettings::default();
                                accent_slider(ui, &mut settings.lru_budget_mb, 128..=4096, defaults.lru_budget_mb, theme);
                            });
                        });

                    ui.add_space(10.0);

                        }); // close ScrollArea

                    // "Saved" indicator pinned below the scroll area
                    let saved_at: Option<f64> = ctx.data(|d| d.get_temp(saved_at_id));
                    if let Some(t) = saved_at {
                        let elapsed = now - t;
                        if elapsed < 2.0 {
                            let alpha = ((1.0 - elapsed / 2.0) as f32).clamp(0.0, 1.0);
                            let green = egui::Color32::from_rgba_unmultiplied(
                                120,
                                220,
                                120,
                                (alpha * 255.0) as u8,
                            );
                            ui.label(
                                egui::RichText::new("✓ Saved")
                                    .size(11.0)
                                    .color(green),
                            );
                            ctx.request_repaint();
                        }
                    }
                });
        });

    // Escape to close
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *show = false;
    }

    // Auto-save on any change inside the modal and stamp the save time so
    // the green "✓ Saved" indicator can show.
    if *settings != snapshot {
        settings.save();
        ctx.data_mut(|d| d.insert_temp(saved_at_id, now));
    }

    settings.cache_count != prev_cache_count || settings.lru_budget_mb != prev_lru_budget
}
