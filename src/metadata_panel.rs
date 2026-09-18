//! The metadata side panel: file facts, the camera summary, location,
//! and every EXIF tag of the image on screen.
//!
//! Everything it shows is already a string in `Pane::current_record`,
//! formatted on the decode thread, so a frame is a few dozen labels and
//! no parsing. Rows are a muted label and a value, 12 px text at an 18 px
//! pitch, the density photo tools use for metadata. The All EXIF list
//! lays out only the rows inside the scroll viewport, because egui
//! rebuilds the UI every frame and during skate the text changes every
//! frame.

use std::path::Path;

use eframe::egui;

use crate::menu::{format_file_size, trash_button};
use crate::metadata::{ExifData, ExifSummary, MetadataRecord};
use crate::pane::Pane;
use crate::theme::UiTheme;

pub(crate) const DEFAULT_WIDTH: f32 = 260.0;
const MIN_WIDTH: f32 = 200.0;
const MAX_WIDTH: f32 = 480.0;
const PANEL_ID: &str = "metadata_panel";

const VALUE_COLOR: egui::Color32 = egui::Color32::from_gray(220);
/// Text size of every row.
const TEXT_SIZE: f32 = 12.0;
/// Height of every row, in the sections and in the tag list alike.
const ROW_H: f32 = 18.0;
/// Size of the section headings and the buttons next to them.
const HEADING_SIZE: f32 = 10.5;
/// Width of the right-aligned label column in the File, Camera and
/// Location sections. "Orientation" is the longest label.
const LABEL_W: f32 = 68.0;
/// Gap between a label and its value.
const LABEL_GAP: f32 = 8.0;

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
            ui.label(egui::RichText::new("No image").size(TEXT_SIZE).color(f.theme.muted));
        });
        return out;
    };
    let record = pane.current_record.as_deref();
    let exif = match record.map(|r| &r.exif) {
        Some(ExifData::Present(exif)) => Some(exif.as_ref()),
        _ => None,
    };

    // Action row. Stars and a flag join the trash button later.
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if trash_button(ui, f.theme).clicked() {
            out.trash_clicked = true;
        }
    });

    ui.scope(|ui| {
        // Rows sit exactly ROW_H apart; headings bring their own space.
        ui.spacing_mut().item_spacing.y = 0.0;
        file_section(ui, path, pane, record, exif, f.theme);
        match record.map(|r| &r.exif) {
            Some(ExifData::Present(exif)) => {
                camera_section(ui, exif, f.theme);
                location_section(ui, exif, f.theme);
            }
            Some(ExifData::Unreadable) => note(ui, "EXIF unreadable", f.theme),
            Some(ExifData::None) | None => note(ui, "No EXIF data", f.theme),
        }
    });
    if let Some(exif) = exif {
        out.all_exif_toggled = all_exif_section(ui, exif, filter, f);
    }
    out
}

fn file_section(
    ui: &mut egui::Ui,
    path: &Path,
    pane: &Pane,
    record: Option<&MetadataRecord>,
    exif: Option<&ExifSummary>,
    theme: &UiTheme,
) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let full_path = path.to_string_lossy().into_owned();
    let folder = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    heading(ui, "FILE", theme, |ui| {
        // Right-to-left: the first button lands at the right edge.
        if heading_button(ui, "copy path").on_hover_text("Copy the full path").clicked() {
            ui.ctx().copy_text(full_path.clone());
        }
        if heading_button(ui, "copy name").on_hover_text("Copy the file name").clicked() {
            ui.ctx().copy_text(name.clone());
        }
    });
    row(ui, "Name", &name, theme);
    row_with(ui, "Folder", theme, |ui, width| folder_cell(ui, &folder, width));

    let mut size_and_format: Vec<String> = Vec::new();
    if let Some(size) = record.and_then(|r| r.file_size) {
        size_and_format.push(format_file_size(size));
    }
    if let Some(format) = record.and_then(|r| r.format.as_deref()) {
        size_and_format.push(format.to_string());
    }
    if !size_and_format.is_empty() {
        row(ui, "Size", &size_and_format.join(" · "), theme);
    }
    if let Some(texture) = &pane.current_texture {
        let [w, h] = texture.size();
        row(ui, "Dims", &format!("{w}x{h} · {}", megapixels(w, h)), theme);
    }
    // The app shows the pixels as stored, so a turned picture gets a line
    // saying which turn would show it upright.
    if let Some(turn) = exif.and_then(|e| e.orientation.as_deref()).filter(|o| *o != "Normal") {
        row(ui, "Orientation", turn, theme);
    }
    if let Some(modified) = record.and_then(|r| r.modified.as_deref()) {
        row(ui, "Modified", modified, theme);
    }
}

/// Shutter, aperture and ISO on one line, the way Photos and phone
/// galleries show an exposure: "1/60 s  f/1.78  ISO 160".
fn exposure_line(exif: &ExifSummary) -> Option<String> {
    let parts: Vec<&str> = [
        exif.shutter.as_deref(),
        exif.aperture.as_deref(),
        exif.iso.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("  "))
    }
}

fn camera_section(ui: &mut egui::Ui, exif: &ExifSummary, theme: &UiTheme) {
    let exposure = exposure_line(exif);
    let rows: [(&str, Option<&str>); 6] = [
        ("Exposure", exposure.as_deref()),
        ("Comp.", exif.exposure_bias.as_deref()),
        ("Focal", exif.focal_length.as_deref()),
        ("Camera", exif.camera.as_deref()),
        ("Lens", exif.lens.as_deref()),
        ("Taken", exif.date_taken.as_deref()),
    ];
    if rows.iter().all(|(_, value)| value.is_none()) {
        return;
    }
    heading(ui, "CAMERA", theme, |_| {});
    for (label, value) in rows {
        if let Some(value) = value {
            row(ui, label, value, theme);
        }
    }
}

fn location_section(ui: &mut egui::Ui, exif: &ExifSummary, theme: &UiTheme) {
    let Some(location) = exif.location else {
        return;
    };
    heading(ui, "LOCATION", theme, |_| {});
    row(ui, "Coords", &location.text(), theme);
    if let Some(altitude) = location.altitude_text() {
        row(ui, "Altitude", &altitude, theme);
    }
    row_with(ui, "", theme, |ui, _| {
        ui.add(egui::Hyperlink::from_label_and_url(
            egui::RichText::new("Open in map").size(TEXT_SIZE),
            location.map_url(),
        ))
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
        egui::RichText::new(title).size(HEADING_SIZE).color(f.theme.heading).strong(),
    )
    .id_salt("all_exif")
    .open(Some(f.all_exif_open))
    .show_unindented(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("Filter")
                .font(egui::FontId::proportional(TEXT_SIZE))
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

/// Tag name and value, one line each, drawn only for the rows inside the
/// scroll viewport. The space above and below stands in for the rest, so
/// the list is always `n * ROW_H` tall and the scrollbar is right
/// whatever part is visible. The cells are labels, so the text can be
/// selected by dragging and copied like the sections above.
fn tag_rows(ui: &mut egui::Ui, rows: &[&(String, String)], f: &Frame) {
    let width = ui.available_width();
    let name_w = (width * 0.45).max(60.0);
    let n = rows.len();

    let list_top = ui.cursor().top() - f.content_top;
    let first = ((f.viewport.min.y - list_top) / ROW_H).floor().max(0.0) as usize;
    let last = (((f.viewport.max.y - list_top) / ROW_H).ceil().max(0.0) as usize + 1).min(n);
    let first = first.min(last);

    ui.scope(|ui| {
        // No spacing between rows: the drawn rows must be exactly as tall
        // as the space standing in for the undrawn ones, or the list's
        // height would change with the scroll offset and the panel would
        // twitch at the end.
        ui.spacing_mut().item_spacing.y = 0.0;
        if first > 0 {
            ui.add_space(first as f32 * ROW_H);
        }
        for (name, val) in &rows[first..last] {
            ui.allocate_ui_with_layout(
                egui::vec2(width, ROW_H),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_size(egui::vec2(width, ROW_H));
                    ui.spacing_mut().item_spacing.x = 0.0;
                    text_cell(ui, name, name_w - LABEL_GAP, f.theme.muted);
                    ui.add_space(LABEL_GAP);
                    text_cell(ui, val, width - name_w, VALUE_COLOR);
                },
            );
        }
        if last < n {
            ui.add_space((n - last) as f32 * ROW_H);
        }
    });
}

// ---- rows -------------------------------------------------------------

/// A section heading with optional controls at the right.
fn heading(ui: &mut egui::Ui, title: &str, theme: &UiTheme, right: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(HEADING_SIZE)
                .color(theme.heading)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    ui.add_space(2.0);
}

fn heading_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(text).size(HEADING_SIZE)).small())
}

fn note(ui: &mut egui::Ui, text: &str, theme: &UiTheme) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).size(TEXT_SIZE).color(theme.muted));
}

/// One row: a muted label right-aligned in its column, then the value.
fn row(ui: &mut egui::Ui, label: &str, value: &str, theme: &UiTheme) {
    row_with(ui, label, theme, |ui, width| text_cell(ui, value, width, VALUE_COLOR));
}

/// A row whose value is drawn by `add_value`, which gets the width left
/// for it.
fn row_with(
    ui: &mut egui::Ui,
    label: &str,
    theme: &UiTheme,
    add_value: impl FnOnce(&mut egui::Ui, f32),
) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, ROW_H));
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.allocate_ui_with_layout(
                egui::vec2(LABEL_W, ROW_H),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.set_min_width(LABEL_W);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(label).size(TEXT_SIZE).color(theme.muted),
                        )
                        .truncate(),
                    );
                },
            );
            ui.add_space(LABEL_GAP);
            add_value(ui, (width - LABEL_W - LABEL_GAP).max(10.0));
        },
    );
}

/// A label cut with an ellipsis at `cell_w`. egui shows the full text on
/// hover when it cut a label, so nothing is added here. Takes exactly
/// `cell_w` so what follows lines up.
fn text_cell(ui: &mut egui::Ui, text: &str, cell_w: f32, color: egui::Color32) {
    let font = egui::FontId::proportional(TEXT_SIZE);
    ui.allocate_ui_with_layout(
        egui::vec2(cell_w, ROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(cell_w);
            ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).truncate());
        },
    );
}

/// The folder, cut from the left when it does not fit: the end of a path
/// is the part that tells folders apart.
fn folder_cell(ui: &mut egui::Ui, folder: &str, cell_w: f32) {
    let font = egui::FontId::proportional(TEXT_SIZE);
    let (shown, cut) = elide_start(ui, folder, cell_w, &font);
    ui.allocate_ui_with_layout(
        egui::vec2(cell_w, ROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(cell_w);
            // Already cut to fit, from the left; egui must not cut it again
            // from the right and add a tooltip of its own.
            let response = ui.add(
                egui::Label::new(egui::RichText::new(shown).font(font).color(VALUE_COLOR)).extend(),
            );
            if cut {
                response.on_hover_text(folder);
            }
        },
    );
}

/// `text`, or "…" plus the longest tail of it that fits in `max_w`,
/// starting at a path separator when there is one. The bool says whether
/// it was cut.
fn elide_start(ui: &egui::Ui, text: &str, max_w: f32, font: &egui::FontId) -> (String, bool) {
    let measure = |s: &str| {
        ui.fonts(|fonts| {
            fonts
                .layout_no_wrap(s.to_string(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    if measure(text) <= max_w {
        return (text.to_string(), false);
    }
    // Cutting more from the left only makes it narrower, so the shortest
    // cut that fits is found by bisection over the character starts.
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let (mut lo, mut hi) = (1, starts.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if measure(&format!("…{}", &text[starts[mid]..])) <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let tail = starts.get(lo).map_or("", |&i| &text[i..]);
    let tail = match tail.find(['/', '\\']) {
        Some(separator) if separator + 1 < tail.len() => &tail[separator..],
        _ => tail,
    };
    (format!("…{tail}"), true)
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

    fn input(width: f32, height: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, height),
            )),
            ..Default::default()
        }
    }

    /// Lay a heading with buttons and a few rows out inside a scrollable,
    /// resizable panel headlessly. The buttons must end at the content's
    /// right edge and every row must be exactly ROW_H tall, whatever its
    /// text. egui lays out without a display, so this runs in `cargo
    /// test`.
    #[test]
    fn heading_buttons_and_rows_keep_their_geometry() {
        let ctx = egui::Context::default();
        let theme = UiTheme::teal_dark();
        let mut button_right = 0.0_f32;
        let mut content_right = 0.0_f32;
        let mut rows_height = 0.0_f32;
        for _ in 0..4 {
            let _ = ctx.run(input(1280.0, 720.0), |ctx| {
                egui::SidePanel::right("test_panel")
                    .resizable(true)
                    .default_width(DEFAULT_WIDTH)
                    .width_range(MIN_WIDTH..=MAX_WIDTH)
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show_viewport(ui, |ui, _viewport| {
                                content_right = ui.max_rect().right();
                                ui.spacing_mut().item_spacing.y = 0.0;
                                heading(ui, "FILE", &theme, |ui| {
                                    button_right = heading_button(ui, "copy path").rect.right();
                                    let _ = heading_button(ui, "copy name");
                                });
                                let top = ui.cursor().top();
                                row(ui, "Name", "46.jpg", &theme);
                                row(ui, "Lens", &"a very long lens name ".repeat(8), &theme);
                                row_with(ui, "Folder", &theme, |ui, w| {
                                    folder_cell(ui, "/home/someone/pictures/2026/bali/day-3/raw", w)
                                });
                                row(ui, "", "", &theme);
                                rows_height = ui.cursor().top() - top;
                                // Enough rows to need a vertical scrollbar.
                                for i in 0..80 {
                                    row(ui, "Row", &format!("{i}"), &theme);
                                }
                            });
                    });
            });
        }
        assert!(
            (button_right - content_right).abs() <= 0.5,
            "button ends at {button_right}, content at {content_right}"
        );
        assert!(
            (rows_height - 4.0 * ROW_H).abs() < 0.5,
            "four rows are {rows_height} px tall, expected {}",
            4.0 * ROW_H
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
        let rows: Vec<(String, String)> = (0..200)
            .map(|i| (format!("Tag{i}"), format!("value {i}")))
            .collect();
        let refs: Vec<&(String, String)> = rows.iter().collect();
        let mut heights = Vec::new();
        for offset in [0.0_f32, 300.0, 1500.0, 3000.0, 3400.0, 9000.0] {
            let _ = ctx.run(input(260.0, 600.0), |ctx| {
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
        // 200 rows of ROW_H, plus the parent's item spacing after the list.
        assert!((first - 200.0 * ROW_H).abs() <= 4.0, "{first}");
    }

    #[test]
    fn folder_is_cut_from_the_left_at_a_separator() {
        let ctx = egui::Context::default();
        let font = egui::FontId::proportional(TEXT_SIZE);
        let path = "/home/someone/pictures/2026/bali/day-3/raw";
        let mut short = (String::new(), false);
        let mut long = (String::new(), false);
        let _ = ctx.run(input(400.0, 300.0), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                short = elide_start(ui, path, 1000.0, &font);
                long = elide_start(ui, path, 120.0, &font);
            });
        });
        assert_eq!(short, (path.to_string(), false));
        assert!(long.1);
        assert!(long.0.starts_with("…/"), "{}", long.0);
        assert!(long.0.ends_with("/day-3/raw") || long.0.ends_with("/raw"), "{}", long.0);
        assert!(path.ends_with(long.0.trim_start_matches('…')), "{}", long.0);

        // Windows separators are cut at too.
        let windows = r"C:\Users\someone\Pictures\2026\bali\day-3\raw";
        let mut cut = (String::new(), false);
        let _ = ctx.run(input(400.0, 300.0), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                cut = elide_start(ui, windows, 120.0, &font);
            });
        });
        assert!(cut.1);
        assert!(cut.0.starts_with("…\\"), "{}", cut.0);
        assert!(windows.ends_with(cut.0.trim_start_matches('…')), "{}", cut.0);
    }

    #[test]
    fn exposure_line_joins_what_exists() {
        let mut exif = ExifSummary::default();
        assert_eq!(exposure_line(&exif), None);
        exif.shutter = Some("1/60 s".into());
        exif.iso = Some("ISO 160".into());
        assert_eq!(exposure_line(&exif).as_deref(), Some("1/60 s  ISO 160"));
        exif.aperture = Some("f/1.78".into());
        assert_eq!(exposure_line(&exif).as_deref(), Some("1/60 s  f/1.78  ISO 160"));
    }
}
