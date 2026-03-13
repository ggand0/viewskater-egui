use eframe::egui;

use crate::pane::Pane;
use crate::settings::AppSettings;
use crate::theme::UiTheme;

/// Toggle switch widget (iOS/Steam style).
/// Returns true if the value changed.
pub(crate) fn toggle_switch(ui: &mut egui::Ui, on: &mut bool, label: &str, theme: &UiTheme) -> bool {
    let desired_size = egui::vec2(32.0, 16.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
    }

    let how_on = ui.ctx().animate_bool_with_time(response.id, *on, 0.15);
    let radius = rect.height() / 2.0;
    let bg = if *on {
        theme.accent
    } else {
        theme.toggle_off
    };
    let knob_color = theme.toggle_knob;

    ui.painter().rect_filled(rect, radius, bg);

    let knob_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
    let knob_center = egui::pos2(knob_x, rect.center().y);
    ui.painter()
        .circle_filled(knob_center, radius - 2.0, knob_color);

    ui.label(label);

    response.clicked()
}

/// Full-width menu row with hover background highlight.
///
/// Uses a Noop shape placeholder so the background is painted behind
/// content that's drawn afterwards.
fn hover_row(
    ui: &mut egui::Ui,
    id_salt: &str,
    theme: &UiTheme,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let available_width = ui.available_width();
    let row = ui.horizontal(|ui| {
        ui.set_min_width(available_width);
        add_contents(ui);
    });
    let rect = row.response.rect;
    if ui
        .interact(rect, ui.id().with(id_salt), egui::Sense::hover())
        .hovered()
    {
        ui.painter().set(
            bg_idx,
            egui::Shape::rect_filled(rect, 2.0, theme.menu_hover),
        );
    }
}

pub fn show_menu_bar(
    ctx: &egui::Context,
    panes: &[Pane],
    settings: &mut AppSettings,
    theme: &UiTheme,
) -> MenuAction {
    let mut action = MenuAction::None;
    let is_dual = panes.len() >= 2;
    let has_images = panes.first().is_some_and(|p| !p.image_paths.is_empty());

    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                ui.menu_button("Open Folder  Ctrl+Shift+O", |ui| {
                    if ui.button("Pane 1").clicked() {
                        action = MenuAction::OpenFolder(0);
                        ui.close_menu();
                    }
                    if is_dual && ui.button("Pane 2").clicked() {
                        action = MenuAction::OpenFolder(1);
                        ui.close_menu();
                    }
                });
                ui.menu_button("Open File  Ctrl+O", |ui| {
                    if ui.button("Pane 1").clicked() {
                        action = MenuAction::OpenFile(0);
                        ui.close_menu();
                    }
                    if is_dual && ui.button("Pane 2").clicked() {
                        action = MenuAction::OpenFile(1);
                        ui.close_menu();
                    }
                });
                ui.separator();
                if ui.add_enabled(has_images, egui::Button::new("Close  Ctrl+W")).clicked() {
                    action = MenuAction::Close;
                    ui.close_menu();
                }
                if ui.button("Quit  Ctrl+Q").clicked() {
                    action = MenuAction::Quit;
                    ui.close_menu();
                }
            });

            ui.menu_button("Edit", |ui| {
                if ui.button("Preferences").clicked() {
                    action = MenuAction::ShowSettings;
                    ui.close_menu();
                }
            });

            ui.menu_button("View", |ui| {
                let pane_count = panes.len();
                hover_row(ui, "single_pane", theme, |ui| {
                    if ui.radio(pane_count == 1, "Single Pane  Ctrl+1").clicked() {
                        if pane_count != 1 {
                            action = MenuAction::SetSinglePane;
                        }
                        ui.close_menu();
                    }
                });
                hover_row(ui, "dual_pane", theme, |ui| {
                    if ui.radio(pane_count >= 2, "Dual Pane  Ctrl+2").clicked() {
                        if pane_count < 2 {
                            action = MenuAction::SetDualPane;
                        }
                        ui.close_menu();
                    }
                });
                ui.separator();
                hover_row(ui, "footer", theme, |ui| {
                    toggle_switch(ui, &mut settings.show_footer, "Footer  Tab", theme);
                });
                hover_row(ui, "fps", theme, |ui| {
                    toggle_switch(ui, &mut settings.show_fps, "FPS Overlay", theme);
                });
                hover_row(ui, "cache", theme, |ui| {
                    toggle_switch(ui, &mut settings.show_cache_overlay, "Cache Overlay", theme);
                });
            });

            ui.menu_button("Help", |ui| {
                if ui.button("About").clicked() {
                    action = MenuAction::ShowAbout;
                    ui.close_menu();
                }
            });
        });
    });

    action
}

pub fn show_footer(ctx: &egui::Context, panes: &[Pane]) {
    egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
        ui.horizontal(|ui| {
            if let Some(pane) = panes.first() {
                if let Some(path) = pane.image_paths.get(pane.current_index) {
                    // Filename
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    ui.label(
                        egui::RichText::new(name.as_ref())
                            .monospace()
                            .color(egui::Color32::from_gray(200))
                            .size(13.0),
                    );

                    // Resolution (from current texture)
                    if let Some(tex) = &pane.current_texture {
                        let size = tex.size();
                        ui.separator();
                        ui.label(
                            egui::RichText::new(format!("{}x{}", size[0], size[1]))
                                .monospace()
                                .color(egui::Color32::from_gray(160))
                                .size(13.0),
                        );
                    }

                    // File size
                    if let Ok(meta) = std::fs::metadata(path) {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(format_file_size(meta.len()))
                                .monospace()
                                .color(egui::Color32::from_gray(160))
                                .size(13.0),
                        );
                    }
                }
            }
        });
    });
}

fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[derive(PartialEq)]
pub enum MenuAction {
    None,
    OpenFolder(usize),
    OpenFile(usize),
    Close,
    Quit,
    SetSinglePane,
    SetDualPane,
    ShowAbout,
    ShowSettings,
}
