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

/// Disable egui's built-in button hover/active/open backgrounds inside
/// a menu popup (so `hover_row` can draw its own full-width highlight),
/// and return the full popup rect coordinates including margins.
fn setup_menu_hover(ui: &mut egui::Ui) -> (f32, f32) {
    let style = ui.style_mut();
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::TRANSPARENT;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.active.weak_bg_fill = egui::Color32::TRANSPARENT;
    style.visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.open.weak_bg_fill = egui::Color32::TRANSPARENT;
    style.visuals.widgets.open.bg_stroke = egui::Stroke::NONE;
    let margin = style.spacing.menu_margin;
    let ml = margin.left as f32;
    let mr = margin.right as f32;
    (
        ui.cursor().min.x - ml,
        ui.available_width() + ml + mr,
    )
}

/// Full-width menu row with hover background highlight.
///
/// Uses a Noop shape placeholder so the background is painted behind
/// content that's drawn afterwards. The highlight rect spans the full
/// popup width (including margins) so it reaches the popup border.
/// Hover is detected via raw pointer position to avoid stealing
/// interaction from child widgets (e.g. sub-menu buttons).
fn hover_row(
    ui: &mut egui::Ui,
    _id_salt: &str,
    theme: &UiTheme,
    menu_left: f32,
    menu_width: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let row = ui.scope(|ui| {
        add_contents(ui);
    });
    let highlight = egui::Rect::from_min_size(
        egui::pos2(menu_left, row.response.rect.min.y),
        egui::vec2(menu_width, row.response.rect.height()),
    );
    let hovered = ui
        .ctx()
        .input(|i| i.pointer.hover_pos().is_some_and(|p| highlight.contains(p)));
    if hovered {
        ui.painter().set(
            bg_idx,
            egui::Shape::rect_filled(highlight, 0.0, theme.menu_hover),
        );
    }
}

pub fn show_menu_bar(
    ctx: &egui::Context,
    panes: &[Pane],
    settings: &mut AppSettings,
    theme: &UiTheme,
    fps_text: Option<&str>,
) -> MenuAction {
    let mut action = MenuAction::None;
    let is_dual = panes.len() >= 2;
    let has_images = panes.first().is_some_and(|p| !p.image_paths.is_empty());

    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                let (ml, mw) = setup_menu_hover(ui);
                hover_row(ui, "open_folder", theme, ml, mw, |ui| {
                    ui.menu_button("Open Folder  Ctrl+Shift+O", |ui| {
                        let (sl, sw) = setup_menu_hover(ui);
                        hover_row(ui, "pane1_folder", theme, sl, sw, |ui| {
                            if ui.button("Pane 1").clicked() {
                                action = MenuAction::OpenFolder(0);
                                ui.close_menu();
                            }
                        });
                        if is_dual {
                            hover_row(ui, "pane2_folder", theme, sl, sw, |ui| {
                                if ui.button("Pane 2").clicked() {
                                    action = MenuAction::OpenFolder(1);
                                    ui.close_menu();
                                }
                            });
                        }
                    });
                });
                hover_row(ui, "open_file", theme, ml, mw, |ui| {
                    ui.menu_button("Open File  Ctrl+O", |ui| {
                        let (sl, sw) = setup_menu_hover(ui);
                        hover_row(ui, "pane1_file", theme, sl, sw, |ui| {
                            if ui.button("Pane 1").clicked() {
                                action = MenuAction::OpenFile(0);
                                ui.close_menu();
                            }
                        });
                        if is_dual {
                            hover_row(ui, "pane2_file", theme, sl, sw, |ui| {
                                if ui.button("Pane 2").clicked() {
                                    action = MenuAction::OpenFile(1);
                                    ui.close_menu();
                                }
                            });
                        }
                    });
                });
                ui.separator();
                hover_row(ui, "close", theme, ml, mw, |ui| {
                    if ui
                        .add_enabled(has_images, egui::Button::new("Close  Ctrl+W"))
                        .clicked()
                    {
                        action = MenuAction::Close;
                        ui.close_menu();
                    }
                });
                hover_row(ui, "quit", theme, ml, mw, |ui| {
                    if ui.button("Quit  Ctrl+Q").clicked() {
                        action = MenuAction::Quit;
                        ui.close_menu();
                    }
                });
            });

            ui.menu_button("Edit", |ui| {
                let (ml, mw) = setup_menu_hover(ui);
                hover_row(ui, "preferences", theme, ml, mw, |ui| {
                    if ui.button("Preferences").clicked() {
                        action = MenuAction::ShowSettings;
                        ui.close_menu();
                    }
                });
            });

            ui.menu_button("View", |ui| {
                let (ml, mw) = setup_menu_hover(ui);
                let pane_count = panes.len();
                hover_row(ui, "single_pane", theme, ml, mw, |ui| {
                    if ui.radio(pane_count == 1, "Single Pane  Ctrl+1").clicked() {
                        if pane_count != 1 {
                            action = MenuAction::SetSinglePane;
                        }
                        ui.close_menu();
                    }
                });
                hover_row(ui, "dual_pane", theme, ml, mw, |ui| {
                    if ui.radio(pane_count >= 2, "Dual Pane  Ctrl+2").clicked() {
                        if pane_count < 2 {
                            action = MenuAction::SetDualPane;
                        }
                        ui.close_menu();
                    }
                });
                ui.separator();
                hover_row(ui, "footer", theme, ml, mw, |ui| {
                    ui.horizontal(|ui| {
                        toggle_switch(ui, &mut settings.show_footer, "Footer  Tab", theme);
                    });
                });
                hover_row(ui, "fps", theme, ml, mw, |ui| {
                    ui.horizontal(|ui| {
                        toggle_switch(ui, &mut settings.show_fps, "FPS Overlay", theme);
                    });
                });
                hover_row(ui, "cache", theme, ml, mw, |ui| {
                    ui.horizontal(|ui| {
                        toggle_switch(
                            ui,
                            &mut settings.show_cache_overlay,
                            "Cache Overlay",
                            theme,
                        );
                    });
                });
            });

            ui.menu_button("Help", |ui| {
                let (ml, mw) = setup_menu_hover(ui);
                hover_row(ui, "about", theme, ml, mw, |ui| {
                    if ui.button("About").clicked() {
                        action = MenuAction::ShowAbout;
                        ui.close_menu();
                    }
                });
            });

            // FPS display (right-aligned in menu bar)
            if let Some(text) = fps_text {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .monospace()
                            .color(egui::Color32::from_gray(220))
                            .size(12.0),
                    );
                });
            }
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

                    // Image index (right-aligned)
                    if !pane.image_paths.is_empty() {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} / {}",
                                        pane.current_index + 1,
                                        pane.image_paths.len()
                                    ))
                                    .monospace()
                                    .color(egui::Color32::from_gray(200))
                                    .size(13.0),
                                );
                            },
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
