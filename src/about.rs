use eframe::egui;

use crate::build_info::BuildInfo;
use crate::theme::UiTheme;

/// Show the about modal. Dismiss via Escape or click-outside.
pub fn show_about_modal(ctx: &egui::Context, show: &mut bool, theme: &UiTheme) {
    if !*show {
        return;
    }

    // Semi-transparent backdrop
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("about_backdrop"))
        .fixed_pos(screen.min)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let response = ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter().rect_filled(screen, 0.0, theme.backdrop);
            if response.clicked() {
                *show = false;
            }
        });

    // Modal content (Tooltip order so it renders above the Foreground backdrop)
    egui::Area::new(egui::Id::new("about_modal"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            let max_width = (screen.width() * 0.8).min(450.0);
            egui::Frame::default()
                .fill(theme.card_bg)
                .stroke(egui::Stroke::new(1.0, theme.card_stroke))
                .corner_radius(8.0)
                .inner_margin(20.0)
                .show(ui, |ui| {
                    ui.set_max_width(max_width);
                    ui.vertical_centered(|ui| {
                        // Title
                        ui.label(
                            egui::RichText::new("ViewSkater")
                                .size(25.0)
                                .strong(),
                        );
                        ui.add_space(15.0);

                        // Version
                        ui.label(
                            egui::RichText::new(format!(
                                "Version {}",
                                BuildInfo::display_version()
                            ))
                            .size(15.0),
                        );

                        // Build
                        ui.label(
                            egui::RichText::new(format!(
                                "Build: {} ({})",
                                BuildInfo::build_string(),
                                BuildInfo::build_profile()
                            ))
                            .size(12.0)
                            .color(theme.muted),
                        );

                        // Commit
                        ui.label(
                            egui::RichText::new(format!(
                                "Commit: {}",
                                BuildInfo::git_hash_short()
                            ))
                            .size(12.0)
                            .color(theme.muted),
                        );

                        // Platform
                        ui.label(
                            egui::RichText::new(format!(
                                "Platform: {}",
                                BuildInfo::target_platform()
                            ))
                            .size(12.0)
                            .color(theme.muted),
                        );

                        ui.add_space(8.0);

                        // Contributors
                        ui.label(egui::RichText::new("Contributors:").size(15.0));
                        let contributors = [
                            ("@ggand0", "https://github.com/ggand0"),
                            ("@hml-pip", "https://github.com/hml-pip"),
                            ("@BafDyce", "https://github.com/BafDyce"),
                            ("@YelovSK", "https://github.com/YelovSK"),
                        ];
                        let font = egui::FontId::proportional(15.0);
                        let spacing = ui.spacing().item_spacing.x;
                        let dot = " · ";
                        let content_width: f32 = contributors.iter().enumerate().map(|(i, (name, _))| {
                            let name_w = ui.fonts(|f| f.layout_no_wrap(name.to_string(), font.clone(), theme.accent).size().x);
                            let dot_w = if i > 0 { ui.fonts(|f| f.layout_no_wrap(dot.to_string(), font.clone(), theme.muted).size().x) + spacing } else { 0.0 };
                            name_w + dot_w + if i > 0 { spacing } else { 0.0 }
                        }).sum();
                        let avail = ui.available_width();
                        let fits = content_width <= avail;
                        let render_contributors = |ui: &mut egui::Ui| {
                            for (i, (name, url)) in contributors.iter().enumerate() {
                                if i > 0 {
                                    ui.label(egui::RichText::new(dot).size(15.0).color(theme.muted));
                                }
                                if ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new(*name)
                                                .size(15.0)
                                                .color(theme.accent),
                                        )
                                        .sense(egui::Sense::click()),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    let _ = webbrowser::open(url);
                                }
                            }
                        };
                        if fits {
                            let pad = ((avail - content_width) / 2.0).max(0.0);
                            ui.horizontal(|ui| {
                                ui.add_space(pad);
                                render_contributors(ui);
                            });
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                render_contributors(ui);
                            });
                        }

                        ui.add_space(4.0);

                        // Link
                        ui.label(egui::RichText::new("Learn more at:").size(15.0));
                        let link_text = "https://github.com/ggand0/viewskater-egui";
                        if ui
                            .add(
                                egui::Label::new(
                                    egui::RichText::new(link_text)
                                        .size(16.0)
                                        .color(theme.accent),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            let _ = webbrowser::open(link_text);
                        }
                    });
                });
        });

    // Escape to close
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *show = false;
    }
}
