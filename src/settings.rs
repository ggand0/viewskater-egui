use std::path::PathBuf;

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::menu::toggle_switch;

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
            ui.painter().rect_filled(
                screen,
                0.0,
                egui::Color32::from_black_alpha(200),
            );
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
                .fill(egui::Color32::from_gray(40))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(80)))
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
                            .color(egui::Color32::from_gray(180)),
                    );
                    ui.add_space(4.0);
                    egui::Frame::default()
                        .fill(egui::Color32::from_gray(30))
                        .corner_radius(6.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                toggle_switch(ui, &mut settings.show_footer, "Footer");
                            });
                            ui.horizontal(|ui| {
                                toggle_switch(ui, &mut settings.show_fps, "FPS Overlay");
                            });
                            ui.horizontal(|ui| {
                                toggle_switch(
                                    ui,
                                    &mut settings.show_cache_overlay,
                                    "Cache Overlay",
                                );
                            });
                        });

                    ui.add_space(12.0);

                    // Performance section
                    ui.label(
                        egui::RichText::new("Performance")
                            .size(14.0)
                            .color(egui::Color32::from_gray(180)),
                    );
                    ui.add_space(4.0);
                    egui::Frame::default()
                        .fill(egui::Color32::from_gray(30))
                        .corner_radius(6.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Cache Size");
                                ui.add(egui::Slider::new(&mut settings.cache_count, 1..=20));
                            });
                            ui.horizontal(|ui| {
                                ui.label("LRU Capacity");
                                ui.add(egui::Slider::new(&mut settings.lru_capacity, 10..=200));
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
