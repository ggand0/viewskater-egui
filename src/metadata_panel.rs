//! The metadata side panel: file facts, camera and capture summary,
//! location, and every EXIF tag of the image on screen.
//!
//! Everything it shows is already a string in `Pane::current_record`,
//! formatted on the decode thread, so a frame is a few dozen labels and
//! no parsing. The All EXIF list lays out only the rows inside the
//! scroll viewport, because egui rebuilds the UI every frame and during
//! skate the text changes every frame.

use eframe::egui;

use crate::menu::{format_file_size, trash_button};
use crate::metadata::{ExifData, ExifSummary};
use crate::pane::Pane;
use crate::theme::UiTheme;

pub(crate) const DEFAULT_WIDTH: f32 = 260.0;
const MIN_WIDTH: f32 = 200.0;
const MAX_WIDTH: f32 = 480.0;
const PANEL_ID: &str = "metadata_panel";

const VALUE_COLOR: egui::Color32 = egui::Color32::from_gray(220);
const VALUE_SIZE: f32 = 13.0;
const TAG_FONT_SIZE: f32 = 12.0;

/// Panel state that lives between frames, on the app.
#[derive(Default)]
pub(crate) struct PanelState {
    /// Which pane the panel describes in dual pane mode.
    pub active_pane: usize,
    /// Text in the All EXIF filter box.
    pub filter: String,
}

/// What the panel reports to the app after a frame.
pub(crate) struct PanelOutput {
    /// Where the panel was drawn.
    pub rect: egui::Rect,
    /// The action row's trash button was clicked, for this pane index.
    pub trash_clicked: Option<usize>,
    /// The resize drag ended this frame; the panel is now this wide.
    pub resized_to: Option<f32>,
    /// The All EXIF header was clicked; the list is now open (true) or
    /// closed.
    pub all_exif_toggled: Option<bool>,
}

/// Per-frame facts the sections need.
struct Frame<'a> {
    theme: &'a UiTheme,
    /// Visible part of the scroll content, in content coordinates.
    viewport: egui::Rect,
    /// Screen y of the scroll content's top, to turn ui positions into
    /// content coordinates.
    content_top: f32,
    all_exif_open: bool,
}

struct PaneOutput {
    trash_clicked: bool,
    all_exif_toggled: Option<bool>,
}

pub(crate) fn show_metadata_panel(
    ctx: &egui::Context,
    panes: &[Pane],
    state: &mut PanelState,
    width: f32,
    all_exif_open: bool,
    theme: &UiTheme,
) -> PanelOutput {
    let panel_id = egui::Id::new(PANEL_ID);
    let mut trash_clicked = None;
    let mut all_exif_toggled = None;
    let response = egui::SidePanel::right(panel_id)
        .resizable(true)
        .default_width(width)
        .width_range(MIN_WIDTH..=MAX_WIDTH)
        .show(ctx, |ui| {
            if panes.len() >= 2 {
                state.active_pane = state.active_pane.min(panes.len() - 1);
                ui.add_space(4.0);
                tab_strip(ui, panes.len(), &mut state.active_pane, theme);
            } else {
                state.active_pane = 0;
            }
            let pane_index = state.active_pane;
            let Some(pane) = panes.get(pane_index) else {
                return;
            };
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, viewport| {
                    let frame = Frame {
                        theme,
                        viewport,
                        content_top: ui.cursor().top(),
                        all_exif_open,
                    };
                    let out = show_pane(ui, pane, &mut state.filter, &frame);
                    if out.trash_clicked {
                        trash_clicked = Some(pane_index);
                    }
                    all_exif_toggled = out.all_exif_toggled;
                });
        });

    // The resize handle is egui's own widget under the panel id; its
    // release is the moment to store the width, not every drag frame.
    let resized_to = ctx
        .read_response(panel_id.with("__resize"))
        .filter(|resize| resize.drag_stopped())
        .map(|_| response.response.rect.width());

    PanelOutput {
        rect: response.response.rect,
        trash_clicked,
        resized_to,
        all_exif_toggled,
    }
}

/// "Pane 1 | Pane 2", drawn like the Preferences tabs.
fn tab_strip(ui: &mut egui::Ui, count: usize, active: &mut usize, theme: &UiTheme) {
    let font = egui::FontId::proportional(13.0);
    let padding = egui::vec2(8.0, 4.0);
    ui.horizontal(|ui| {
        for i in 0..count {
            let label = format!("Pane {}", i + 1);
            let galley = ui.painter().layout_no_wrap(
                label.clone(),
                font.clone(),
                egui::Color32::PLACEHOLDER,
            );
            let desired = galley.size() + padding * 2.0 + egui::vec2(0.0, 3.0);
            let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
            let is_active = *active == i;
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
                &label,
                font.clone(),
                color,
            );
            if is_active {
                ui.painter().hline(
                    (rect.min.x + padding.x)..=(rect.max.x - padding.x),
                    rect.max.y - 1.0,
                    egui::Stroke::new(2.0_f32, theme.accent),
                );
            }
            if response.clicked() {
                *active = i;
            }
        }
    });
    ui.add_space(2.0);
}

fn show_pane(ui: &mut egui::Ui, pane: &Pane, filter: &mut String, f: &Frame) -> PaneOutput {
    let mut out = PaneOutput { trash_clicked: false, all_exif_toggled: None };
    let Some(path) = pane.image_paths.get(pane.current_index) else {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("No image").color(f.theme.muted));
        });
        return out;
    };
    let record = pane.current_record.as_deref();

    // Action row. Stars and a flag join the trash button later.
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if trash_button(ui, f.theme).clicked() {
            out.trash_clicked = true;
        }
    });

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let full_path = path.to_string_lossy().into_owned();
    section(
        ui,
        "File",
        f.theme,
        |ui| {
            // Right-to-left: the first button lands at the right edge.
            if ui.small_button("copy path").on_hover_text("Copy the full path").clicked() {
                ui.ctx().copy_text(full_path.clone());
            }
            if ui.small_button("copy name").on_hover_text("Copy the file name").clicked() {
                ui.ctx().copy_text(name.clone());
            }
        },
        |ui| {
            value(ui, &name);
            wrapped_value(ui, &full_path, f.theme.muted);
            if let Some(texture) = &pane.current_texture {
                let [w, h] = texture.size();
                value(ui, &format!("{w}x{h} · {}", megapixels(w, h)));
            }
            let mut facts: Vec<String> = Vec::new();
            if let Some(size) = record.and_then(|r| r.file_size) {
                facts.push(format_file_size(size));
            }
            if let Some(format) = record.and_then(|r| r.format.as_deref()) {
                facts.push(format.to_string());
            }
            if !facts.is_empty() {
                value(ui, &facts.join(" · "));
            }
            if let Some(modified) = record.and_then(|r| r.modified.as_deref()) {
                value(ui, modified);
            }
        },
    );

    match record.map(|r| &r.exif) {
        Some(ExifData::Present(exif)) => {
            camera_section(ui, exif, f.theme);
            capture_section(ui, exif, f.theme);
            location_section(ui, exif, f.theme);
            out.all_exif_toggled = all_exif_section(ui, exif, filter, f);
        }
        Some(ExifData::Unreadable) => note(ui, "EXIF unreadable", f.theme),
        Some(ExifData::None) | None => note(ui, "No EXIF data", f.theme),
    }
    out
}

/// One line of a section: a value, or two side by side.
enum Row<'a> {
    One(&'a str),
    Two(&'a str, &'a str),
}

fn one(text: Option<&str>) -> Option<Row<'_>> {
    text.map(Row::One)
}

fn pair<'a>(a: Option<&'a str>, b: Option<&'a str>) -> Option<Row<'a>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(Row::Two(a, b)),
        (Some(a), None) | (None, Some(a)) => Some(Row::One(a)),
        (None, None) => None,
    }
}

/// A section card of rows, or nothing when there are no rows.
fn rows_section(ui: &mut egui::Ui, title: &str, rows: &[Option<Row>], theme: &UiTheme) {
    if rows.iter().all(Option::is_none) {
        return;
    }
    section(ui, title, theme, |_| {}, |ui| {
        for row in rows.iter().flatten() {
            match row {
                Row::One(a) => value(ui, a),
                Row::Two(a, b) => two_values(ui, a, b),
            }
        }
    });
}

fn camera_section(ui: &mut egui::Ui, exif: &ExifSummary, theme: &UiTheme) {
    rows_section(
        ui,
        "Camera",
        &[
            one(exif.camera.as_deref()),
            one(exif.lens.as_deref()),
            one(exif.focal_length.as_deref()),
            pair(exif.aperture.as_deref(), exif.shutter.as_deref()),
            pair(exif.iso.as_deref(), exif.exposure_bias.as_deref()),
        ],
        theme,
    );
}

fn capture_section(ui: &mut egui::Ui, exif: &ExifSummary, theme: &UiTheme) {
    rows_section(
        ui,
        "Capture",
        &[
            one(exif.date_taken.as_deref()),
            one(exif.exposure_program.as_deref()),
            pair(exif.metering.as_deref(), exif.white_balance.as_deref()),
            one(exif.flash.as_deref()),
            // Only a turned picture is worth a line.
            one(exif.orientation.as_deref().filter(|o| *o != "Normal")),
        ],
        theme,
    );
}

fn location_section(ui: &mut egui::Ui, exif: &ExifSummary, theme: &UiTheme) {
    let Some(location) = exif.location else {
        return;
    };
    section(ui, "Location", theme, |_| {}, |ui| {
        value(ui, &location.text());
        if let Some(altitude) = location.altitude_text() {
            value(ui, &altitude);
        }
        ui.hyperlink_to("Open in map", location.map_url())
            .on_hover_text("OpenStreetMap");
    });
}

/// Every tag behind a collapsing header with a filter box. Returns the
/// new open state when the header was clicked.
fn all_exif_section(
    ui: &mut egui::Ui,
    exif: &ExifSummary,
    filter: &mut String,
    f: &Frame,
) -> Option<bool> {
    let needle = filter.trim().to_lowercase();
    let rows: Vec<&(String, String)> = if needle.is_empty() {
        exif.tags.iter().collect()
    } else {
        exif.tags
            .iter()
            .filter(|(name, value)| {
                name.to_lowercase().contains(&needle) || value.to_lowercase().contains(&needle)
            })
            .collect()
    };
    let title = if needle.is_empty() {
        format!("ALL EXIF ({})", exif.tags.len())
    } else {
        format!("ALL EXIF ({} / {})", rows.len(), exif.tags.len())
    };

    ui.add_space(8.0);
    let header = egui::CollapsingHeader::new(
        egui::RichText::new(title).size(11.0).color(f.theme.heading).strong(),
    )
    .id_salt("all_exif")
    .open(Some(f.all_exif_open))
    .show(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("Filter")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        tag_rows(ui, &rows, f);
    });
    header
        .header_response
        .clicked()
        .then_some(!f.all_exif_open)
}

/// Two columns, name and value, one line each, drawn only for the rows
/// inside the scroll viewport. The space above and below stands in for
/// the rest, so the list is always `n * row_h` tall and the scrollbar is
/// right whatever part is visible.
fn tag_rows(ui: &mut egui::Ui, rows: &[&(String, String)], f: &Frame) {
    let font = egui::FontId::monospace(TAG_FONT_SIZE);
    let row_h = ui.fonts(|fonts| fonts.row_height(&font)) + 6.0;
    let width = ui.available_width();
    let name_w = (width * 0.42).max(60.0);
    let n = rows.len();

    let list_top = ui.cursor().top() - f.content_top;
    let first = ((f.viewport.min.y - list_top) / row_h).floor().max(0.0) as usize;
    let last = (((f.viewport.max.y - list_top) / row_h).ceil().max(0.0) as usize + 1).min(n);
    let first = first.min(last);

    ui.scope(|ui| {
        // No spacing between rows: the drawn rows must be exactly as tall
        // as the space standing in for the undrawn ones, or the list's
        // height would change with the scroll offset and the panel would
        // twitch at the end.
        ui.spacing_mut().item_spacing.y = 0.0;
        if first > 0 {
            ui.add_space(first as f32 * row_h);
        }
        for (name, val) in &rows[first..last] {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, row_h), egui::Sense::hover());
            let name_galley = ui.fonts(|fonts| {
                fonts.layout_job(truncated(name, &font, f.theme.muted, name_w - 6.0))
            });
            let value_galley = ui.fonts(|fonts| {
                fonts.layout_job(truncated(val, &font, VALUE_COLOR, width - name_w))
            });
            let y = rect.center().y - name_galley.size().y / 2.0;
            let painter = ui.painter();
            painter.galley(egui::pos2(rect.left(), y), name_galley.clone(), f.theme.muted);
            painter.galley(egui::pos2(rect.left() + name_w, y), value_galley.clone(), VALUE_COLOR);
            if name_galley.elided || value_galley.elided {
                response.on_hover_text(format!("{name}\n{val}"));
            }
        }
        if last < n {
            ui.add_space((n - last) as f32 * row_h);
        }
    });
}

fn truncated(
    text: &str,
    font: &egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font.clone(), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(10.0));
    job
}

/// Heading row with optional controls at the right, then a card.
/// Returns the card's outer rect.
fn section(
    ui: &mut egui::Ui,
    title: &str,
    theme: &UiTheme,
    heading_right: impl FnOnce(&mut egui::Ui),
    body: impl FnOnce(&mut egui::Ui),
) -> egui::Rect {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .size(11.0)
                .color(theme.heading)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), heading_right);
    });
    ui.add_space(2.0);
    egui::Frame::default()
        .fill(theme.card_bg)
        .corner_radius(6.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            body(ui);
        })
        .response
        .rect
}

fn note(ui: &mut egui::Ui, text: &str, theme: &UiTheme) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).size(VALUE_SIZE).color(theme.muted));
}

/// One value on its own line, cut with an ellipsis when too long.
fn value(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .monospace()
                .size(VALUE_SIZE)
                .color(VALUE_COLOR),
        )
        .truncate(),
    );
}

/// A value that wraps onto several lines (the full path).
fn wrapped_value(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.add(
        egui::Label::new(egui::RichText::new(text).monospace().size(VALUE_SIZE).color(color))
            .wrap(),
    );
}

fn two_values(ui: &mut egui::Ui, a: &str, b: &str) {
    ui.horizontal(|ui| {
        value(ui, a);
        ui.add_space(10.0);
        value(ui, b);
    });
}

fn megapixels(w: usize, h: usize) -> String {
    let mp = (w * h) as f64 / 1_000_000.0;
    if mp >= 10.0 {
        format!("{mp:.0} MP")
    } else {
        format!("{mp:.1} MP")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay the File section out inside a scrollable, resizable panel
    /// headlessly and check the heading's right-aligned buttons end where
    /// the card ends. egui lays out without a display, so this runs in
    /// `cargo test`.
    #[test]
    fn heading_buttons_stay_inside_the_card() {
        let ctx = egui::Context::default();
        let theme = UiTheme::teal_dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let mut button_right = 0.0_f32;
        let mut card = egui::Rect::NOTHING;
        let mut content_right = 0.0_f32;
        for _ in 0..4 {
            let _ = ctx.run(input.clone(), |ctx| {
                egui::SidePanel::right("test_panel")
                    .resizable(true)
                    .default_width(DEFAULT_WIDTH)
                    .width_range(MIN_WIDTH..=MAX_WIDTH)
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show_viewport(ui, |ui, _viewport| {
                                content_right = ui.max_rect().right();
                                card = section(
                                    ui,
                                    "File",
                                    &theme,
                                    |ui| {
                                        button_right = ui.small_button("copy path").rect.right();
                                        let _ = ui.small_button("copy name");
                                    },
                                    |ui| value(ui, "46.jpg"),
                                );
                                // Enough rows to need a vertical scrollbar.
                                for i in 0..80 {
                                    value(ui, &format!("row {i}"));
                                }
                            });
                    });
            });
        }
        assert!(
            (button_right - card.right()).abs() <= 0.5,
            "button ends at {button_right}, card at {}, content at {content_right}",
            card.right()
        );
    }

    /// The tag list stands in for the rows outside the viewport with empty
    /// space, so its height must be the same whatever part is visible.
    /// If it is not, the scroll area's content shrinks as you scroll
    /// towards the end, the offset gets clamped, and the panel twitches.
    #[test]
    fn tag_list_height_does_not_depend_on_scroll_offset() {
        let ctx = egui::Context::default();
        let theme = UiTheme::teal_dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(260.0, 600.0),
            )),
            ..Default::default()
        };
        let rows: Vec<(String, String)> = (0..200)
            .map(|i| (format!("Tag{i}"), format!("value {i}")))
            .collect();
        let refs: Vec<&(String, String)> = rows.iter().collect();
        let mut heights = Vec::new();
        for offset in [0.0_f32, 300.0, 1500.0, 3500.0, 4200.0, 9000.0] {
            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let top = ui.cursor().top();
                    let frame = Frame {
                        theme: &theme,
                        viewport: egui::Rect::from_min_size(
                            egui::pos2(0.0, offset),
                            egui::vec2(260.0, 600.0),
                        ),
                        content_top: top,
                        all_exif_open: true,
                    };
                    tag_rows(ui, &refs, &frame);
                    heights.push(ui.cursor().top() - top);
                });
            });
        }
        let first = heights[0];
        assert!(
            heights.iter().all(|h| (h - first).abs() < 0.5),
            "list height changes with the scroll offset: {heights:?}"
        );
    }
}
