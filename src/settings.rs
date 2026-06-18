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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
enum SettingsTab {
    #[default]
    General,
    Performance,
}

impl SettingsTab {
    const ALL: [Self; 2] = [Self::General, Self::Performance];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Performance => "Performance",
        }
    }
}

fn tab_bar(ui: &mut egui::Ui, active: &mut SettingsTab, theme: &UiTheme) {
    let font = egui::FontId::proportional(14.0);
    let padding = egui::vec2(6.0, 4.0);
    let underline_gap = 3.0;

    ui.horizontal(|ui| {
        for tab in SettingsTab::ALL {
            let is_active = *active == tab;

            let galley = ui.painter().layout_no_wrap(
                tab.label().to_string(),
                font.clone(),
                egui::Color32::PLACEHOLDER,
            );
            let desired = galley.size() + padding * 2.0 + egui::vec2(0.0, underline_gap);
            let (rect, response) =
                ui.allocate_exact_size(desired, egui::Sense::click());

            let color = if is_active {
                theme.accent
            } else if response.hovered() {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_gray(160)
            };

            ui.painter().text(
                rect.min + padding,
                egui::Align2::LEFT_TOP,
                tab.label(),
                font.clone(),
                color,
            );

            if is_active {
                ui.painter().hline(
                    (rect.min.x + padding.x)..=(rect.max.x - padding.x),
                    rect.max.y - 1.0,
                    egui::Stroke::new(2.0, theme.accent),
                );
            }

            if response.clicked() {
                *active = tab;
            }
        }
    });
    ui.add_space(2.0);
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuMemoryMode {
    /// gpu_allocator default (~256 MB blocks). Highest navigation speed,
    /// largest GPU memory footprint.
    Performance,
    /// 64 MB device / 32 MB host blocks. Recommended balance.
    #[default]
    Balanced,
    /// 8 MB device / 4 MB host blocks. Lowest GPU memory, but a 4K texture
    /// no longer fits in a single block — degrades navigation performance.
    LowMemory,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSortKey {
    #[default]
    Name,
    Modified,
    Created,
    Size,
    Extension,
}

impl ImageSortKey {
    pub(crate) const ALL: [Self; 5] = [
        Self::Name,
        Self::Modified,
        Self::Created,
        Self::Size,
        Self::Extension,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Modified => "Modified Date",
            Self::Created => "Created Date",
            Self::Size => "File Size",
            Self::Extension => "Extension",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

impl SortDirection {
    pub(crate) const ALL: [Self; 2] = [Self::Ascending, Self::Descending];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ascending => "Ascending",
            Self::Descending => "Descending",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSortOrder {
    pub key: ImageSortKey,
    pub direction: SortDirection,
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
    pub decode_threads: usize,
    pub gpu_memory_mode: GpuMemoryMode,
    pub mouse_wheel_zoom: bool,
    pub reset_zoom_pan_on_navigation: bool,
    pub image_sort_order: ImageSortOrder,
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
            decode_threads: 10,
            gpu_memory_mode: GpuMemoryMode::default(),
            mouse_wheel_zoom: false,
            reset_zoom_pan_on_navigation: true,
            image_sort_order: ImageSortOrder::default(),
        }
    }
}

#[derive(Default)]
pub struct SettingsChanges {
    pub pane_settings: bool,
}

impl SettingsChanges {
    pub(crate) fn between(before: &AppSettings, after: &AppSettings) -> Self {
        Self {
            pane_settings: after.cache_count != before.cache_count
                || after.lru_budget_mb != before.lru_budget_mb
                || after.decode_threads != before.decode_threads
                || after.mouse_wheel_zoom != before.mouse_wheel_zoom
                || after.reset_zoom_pan_on_navigation != before.reset_zoom_pan_on_navigation,
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

/// Show the settings modal and report which settings changed.
pub fn show_settings_modal(
    ctx: &egui::Context,
    settings: &mut AppSettings,
    show: &mut bool,
    theme: &UiTheme,
) -> SettingsChanges {
    if !*show {
        return SettingsChanges::default();
    }

    // Snapshot at start of frame; if anything changes we save immediately
    // and stamp the save time so the "Saved" indicator can fade in.
    let snapshot = settings.clone();

    let saved_at_id = egui::Id::new("settings_saved_at");
    let now = ctx.input(|i| i.time);

    // Semi-transparent backdrop
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("settings_backdrop"))
        .fixed_pos(screen.min)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let response = ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter().rect_filled(screen, 0.0, theme.backdrop);
            if response.clicked() {
                *show = false;
            }
        });

    let max_modal_height = (screen.height() * 0.75).clamp(200.0, 600.0);

    // Modal card
    egui::Area::new(egui::Id::new("settings_modal"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .order(egui::Order::Foreground)
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
                    ui.add_space(4.0);

                    let tab_id = egui::Id::new("settings_active_tab");
                    let mut active_tab: SettingsTab =
                        ctx.data(|d| d.get_temp(tab_id)).unwrap_or_default();
                    tab_bar(ui, &mut active_tab, theme);
                    ui.separator();
                    ui.add_space(4.0);

                    let max_h_id = egui::Id::new("settings_max_tab_h");
                    let mut target_h: f32 =
                        ctx.data(|d| d.get_temp(max_h_id)).unwrap_or(0.0);

                    if target_h == 0.0 {
                        for tab in SettingsTab::ALL {
                            let rect = egui::Rect::from_min_size(
                                ui.cursor().min,
                                egui::vec2(ui.available_width(), 10000.0),
                            );
                            #[allow(deprecated)]
                            let mut child = ui.child_ui_with_id_source(
                                rect,
                                *ui.layout(),
                                ("settings_measure", tab as u8),
                                None,
                            );
                            child.set_invisible();
                            let mut tmp = settings.clone();
                            match tab {
                                SettingsTab::General => render_general_tab(&mut child, &mut tmp, theme),
                                SettingsTab::Performance => render_performance_tab(&mut child, &mut tmp, theme),
                            }
                            target_h = target_h.max(child.min_rect().height());
                        }
                        ctx.data_mut(|d| d.insert_temp(max_h_id, target_h));
                    }

                    egui::ScrollArea::vertical()
                        .id_salt(egui::Id::new("settings_scroll").with(active_tab))
                        .max_height(target_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            match active_tab {
                                SettingsTab::General => {
                                    render_general_tab(ui, settings, theme);
                                }
                                SettingsTab::Performance => {
                                    render_performance_tab(ui, settings, theme);
                                }
                            }
                        });

                    ctx.data_mut(|d| d.insert_temp(tab_id, active_tab));

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
                                egui::RichText::new("✔ Saved")
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

    SettingsChanges::between(&snapshot, settings)
}

fn section(
    ui: &mut egui::Ui,
    heading: &str,
    subtitle: Option<&str>,
    theme: &UiTheme,
    content: impl FnOnce(&mut egui::Ui),
) {
    ui.label(
        egui::RichText::new(heading)
            .size(14.0)
            .color(theme.heading),
    );
    if let Some(sub) = subtitle {
        ui.label(
            egui::RichText::new(sub)
                .size(11.0)
                .color(theme.muted),
        );
    }
    ui.add_space(4.0);
    egui::Frame::default()
        .fill(theme.section_bg)
        .corner_radius(6.0)
        .inner_margin(10.0)
        .show(ui, content);
}

fn render_general_tab(ui: &mut egui::Ui, settings: &mut AppSettings, theme: &UiTheme) {
    section(ui, "Control", None, theme, |ui| {
        ui.horizontal(|ui| {
            toggle_switch(ui, &mut settings.mouse_wheel_zoom, "Mouse Wheel Zoom", theme);
        });
    });

    ui.add_space(12.0);

    section(ui, "Files", Some("Default sorting for newly opened folders"), theme, |ui| {
        ui.horizontal(|ui| {
            ui.label("Sort By");
            egui::ComboBox::from_id_salt("image_sort_order_key")
                .selected_text(settings.image_sort_order.key.label())
                .show_ui(ui, |ui| {
                    for sort_key in ImageSortKey::ALL {
                        ui.selectable_value(
                            &mut settings.image_sort_order.key,
                            sort_key,
                            sort_key.label(),
                        );
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Direction");
            egui::ComboBox::from_id_salt("image_sort_order_direction")
                .selected_text(settings.image_sort_order.direction.label())
                .show_ui(ui, |ui| {
                    for direction in SortDirection::ALL {
                        ui.selectable_value(
                            &mut settings.image_sort_order.direction,
                            direction,
                            direction.label(),
                        );
                    }
                });
        });
    });

    ui.add_space(12.0);

    section(ui, "Display", None, theme, |ui| {
        ui.horizontal(|ui| {
            toggle_switch(ui, &mut settings.show_footer, "Footer", theme);
        });
        ui.horizontal(|ui| {
            toggle_switch(ui, &mut settings.show_fps, "FPS Overlay", theme);
        });
        ui.horizontal(|ui| {
            toggle_switch(ui, &mut settings.show_cache_overlay, "Cache Overlay", theme);
        });
        ui.horizontal(|ui| {
            toggle_switch(ui, &mut settings.sync_zoom_pan, "Sync Zoom/Pan", theme);
        });
        ui.horizontal(|ui| {
            toggle_switch(
                ui,
                &mut settings.reset_zoom_pan_on_navigation,
                "Reset Zoom/Pan on Navigation",
                theme,
            );
        });
    });

    ui.add_space(10.0);
}

fn render_performance_tab(ui: &mut egui::Ui, settings: &mut AppSettings, theme: &UiTheme) {
    section(ui, "Graphics", None, theme, |ui| {
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

    section(ui, "Performance", Some("Double-click to reset"), theme, |ui| {
        let defaults = AppSettings::default();

        ui.horizontal(|ui| {
            ui.label("Cache Size");
            accent_slider(ui, &mut settings.cache_count, 1..=20, defaults.cache_count, theme);
        });
        ui.label(
            egui::RichText::new("Images prefetched in each direction. Higher = smoother keyboard nav, more GPU memory.")
                .size(11.0)
                .color(theme.muted),
        );
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label("LRU Budget (MB)");
            accent_slider(ui, &mut settings.lru_budget_mb, 128..=4096, defaults.lru_budget_mb, theme);
        });
        ui.label(
            egui::RichText::new("GPU memory for caching slider-visited images. Higher = faster revisits, more VRAM.")
                .size(11.0)
                .color(theme.muted),
        );
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label("Decode Threads");
            accent_slider(ui, &mut settings.decode_threads, 1..=16, defaults.decode_threads, theme);
        });
        ui.label(
            egui::RichText::new("Concurrent image decodes. Higher = faster cache fill, larger memory spikes.")
                .size(11.0)
                .color(theme.muted),
        );
    });

    ui.add_space(10.0);
}
