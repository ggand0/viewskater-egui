use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use eframe::egui;

mod cache;
mod file_io;
mod perf;

const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 100.0;

#[derive(Parser)]
#[command(name = "viewskater-egui", about = "Fast image viewer")]
struct Args {
    /// Paths to image files or directories
    paths: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// Per-pane state
// ---------------------------------------------------------------------------

struct PaneState {
    image_paths: Vec<PathBuf>,
    current_index: usize,
    current_texture: Option<egui::TextureHandle>,
    zoom: f32,
    pan: egui::Vec2,
    cache: Option<cache::SlidingWindowCache>,
}

impl PaneState {
    fn new() -> Self {
        Self {
            image_paths: Vec::new(),
            current_index: 0,
            current_texture: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            cache: None,
        }
    }

    fn open_path(&mut self, path: &std::path::Path, ctx: &egui::Context) {
        if !path.exists() {
            log::error!("Path does not exist: {}", path.display());
            return;
        }

        let (dir, target_filename) = file_io::resolve_path(path);
        self.image_paths = file_io::enumerate_images(&dir);

        if self.image_paths.is_empty() {
            log::warn!("No supported images found in {}", dir.display());
            return;
        }

        self.current_index = target_filename
            .and_then(|name| {
                self.image_paths
                    .iter()
                    .position(|p| {
                        p.file_name().map(|f| f.to_string_lossy().into_owned())
                            == Some(name.clone())
                    })
            })
            .unwrap_or(0);

        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;

        let mut c = cache::SlidingWindowCache::new(ctx);
        c.initialize(self.current_index, &self.image_paths);
        self.current_texture = c.current_texture_for(self.current_index);
        self.cache = Some(c);
    }

    /// Synchronous decode fallback for slider drag and jump.
    fn load_sync(&mut self, ctx: &egui::Context) {
        let Some(path) = self.image_paths.get(self.current_index) else {
            return;
        };

        let start = Instant::now();
        match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let pixels = rgba.into_raw();
                let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                self.current_texture = Some(ctx.load_texture(
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    color_image,
                    egui::TextureOptions::LINEAR,
                ));
                let decode_ms = start.elapsed().as_secs_f64() * 1000.0;
                log::debug!(
                    "Sync fallback: {} ({}x{}) in {:.1}ms",
                    path.display(),
                    size[0],
                    size[1],
                    decode_ms
                );
            }
            Err(e) => {
                log::error!("Failed to load {}: {}", path.display(), e);
                self.current_texture = None;
            }
        }
    }

    /// Try to navigate by `delta` images. Returns true if the display advanced.
    fn navigate(&mut self, delta: isize) -> bool {
        if self.image_paths.is_empty() {
            return false;
        }
        let new_index = (self.current_index as isize + delta)
            .clamp(0, self.image_paths.len() as isize - 1) as usize;
        if new_index == self.current_index {
            return false;
        }

        if let Some(cache) = &mut self.cache {
            if let Some(t) = cache.current_texture_for(new_index) {
                self.current_index = new_index;
                self.zoom = 1.0;
                self.pan = egui::Vec2::ZERO;
                self.current_texture = Some(t);

                if delta > 0 {
                    cache.navigate_forward(new_index, &self.image_paths);
                } else {
                    cache.navigate_backward(new_index, &self.image_paths);
                }
                return true;
            }
        }
        false
    }

    fn jump_to(&mut self, index: usize, ctx: &egui::Context) {
        let index = index.min(self.image_paths.len().saturating_sub(1));
        if index == self.current_index {
            return;
        }

        self.current_index = index;
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;

        if let Some(cache) = &mut self.cache {
            cache.jump_to(index, &self.image_paths);
            self.current_texture = cache.current_texture_for(index);
            if self.current_texture.is_none() {
                self.load_sync(ctx);
            }
        } else {
            self.load_sync(ctx);
        }
    }

    fn can_navigate_forward(&self) -> bool {
        !self.image_paths.is_empty()
            && self.current_index < self.image_paths.len() - 1
    }

    fn can_navigate_backward(&self) -> bool {
        !self.image_paths.is_empty() && self.current_index > 0
    }

    /// Check whether the next image in the given direction is cached and ready.
    fn is_next_cached(&self, delta: isize) -> bool {
        if self.image_paths.is_empty() {
            return false;
        }
        let new_index = (self.current_index as isize + delta)
            .clamp(0, self.image_paths.len() as isize - 1) as usize;
        if new_index == self.current_index {
            return true; // at boundary — nothing to advance to
        }
        self.cache
            .as_ref()
            .map_or(false, |c| c.current_texture_for(new_index).is_some())
    }

    fn poll_cache(&mut self) {
        if let Some(cache) = &mut self.cache {
            cache.poll(&self.image_paths);
        }
    }

    fn show_content(&mut self, ui: &mut egui::Ui) {
        let tex = self.current_texture.clone();
        if let Some(tex) = tex {
            self.show_image(ui, &tex);
        } else if self.image_paths.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("Drop an image or folder here, or pass a path as argument");
            });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Failed to load image");
            });
        }
    }

    fn show_image(&mut self, ui: &mut egui::Ui, tex: &egui::TextureHandle) {
        let tex_size = tex.size_vec2();
        let available = ui.available_rect_before_wrap();

        if available.width() <= 0.0 || available.height() <= 0.0 {
            return;
        }
        if tex_size.x <= 0.0 || tex_size.y <= 0.0 {
            return;
        }

        let response = ui.allocate_rect(available, egui::Sense::click_and_drag());

        // Zoom: scroll wheel + pinch-to-zoom
        if response.hovered() {
            let (scroll, pinch) = ui.input(|i| (i.raw_scroll_delta.y, i.zoom_delta()));
            let scroll_factor = if scroll != 0.0 {
                (scroll * 0.003).exp()
            } else {
                1.0
            };
            let zoom_factor = pinch * scroll_factor;

            if zoom_factor != 1.0 {
                let old_zoom = self.zoom;
                self.zoom = (self.zoom * zoom_factor).clamp(MIN_ZOOM, MAX_ZOOM);

                if let Some(hover_pos) = response.hover_pos() {
                    let old_center = available.center() + self.pan;
                    let cursor_rel = hover_pos - old_center;
                    self.pan += cursor_rel * (1.0 - self.zoom / old_zoom);
                }
            }
        }

        // Pan: drag
        if response.dragged() {
            self.pan += response.drag_delta();
        }

        // Double-click: reset zoom and pan
        if response.double_clicked() {
            self.zoom = 1.0;
            self.pan = egui::Vec2::ZERO;
        }

        // Compute display rect with updated zoom/pan (zero-frame-delay)
        let scale = (available.width() / tex_size.x).min(available.height() / tex_size.y);
        let base_size = tex_size * scale;
        let display_size = base_size * self.zoom;
        let center = available.center() + self.pan;
        let display_rect = egui::Rect::from_center_size(center, display_size);

        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter()
            .image(tex.id(), display_rect, uv, egui::Color32::WHITE);
    }
}

// ---------------------------------------------------------------------------
// App — coordinates panes, input, and overlays
// ---------------------------------------------------------------------------

struct App {
    panes: Vec<PaneState>,
    perf: perf::ImagePerfTracker,
    divider_fraction: f32,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        let mut app = Self {
            panes: vec![PaneState::new()],
            perf: perf::ImagePerfTracker::new(),
            divider_fraction: 0.5,
        };

        if !paths.is_empty() {
            app.panes[0].open_path(&paths[0], &cc.egui_ctx);
        }
        if paths.len() >= 2 {
            let mut pane1 = PaneState::new();
            pane1.open_path(&paths[1], &cc.egui_ctx);
            app.panes.push(pane1);
        }

        if app.panes[0].current_texture.is_some() {
            app.perf.record_image_load(0.0);
        }

        app
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let (home, end, nav_right_held, nav_left_held, toggle_dual) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
                i.key_down(egui::Key::ArrowRight) || i.key_down(egui::Key::D),
                i.key_down(egui::Key::ArrowLeft) || i.key_down(egui::Key::A),
                i.key_pressed(egui::Key::Tab),
            )
        });

        if toggle_dual {
            if self.panes.len() >= 2 {
                // Dual → single: remove second pane
                self.panes.truncate(1);
            } else if !self.panes.is_empty() {
                // Single → dual: clone pane 0's directory into a new pane
                let mut pane = PaneState::new();
                if !self.panes[0].image_paths.is_empty() {
                    if let Some(dir) = self.panes[0].image_paths[0].parent() {
                        pane.open_path(dir, ctx);
                        pane.jump_to(self.panes[0].current_index, ctx);
                    }
                }
                self.panes.push(pane);
            }
            return;
        }

        if home {
            for pane in &mut self.panes {
                pane.jump_to(0, ctx);
            }
            self.perf.record_image_load(0.0);
        } else if end {
            for pane in &mut self.panes {
                let last = pane.image_paths.len().saturating_sub(1);
                pane.jump_to(last, ctx);
            }
            self.perf.record_image_load(0.0);
        } else if nav_right_held {
            // Only advance if ALL panes have the next image cached (synced nav)
            let all_ready = self.panes.iter().all(|p| p.is_next_cached(1));
            if all_ready {
                // fold instead of any() to avoid short-circuit — call navigate on every pane
                let any_advanced = self.panes.iter_mut().fold(false, |acc, p| p.navigate(1) || acc);
                if any_advanced {
                    self.perf.record_image_load(0.0);
                }
            }
            let any_can = self.panes.iter().any(|p| p.can_navigate_forward());
            if any_can {
                ctx.request_repaint();
            }
        } else if nav_left_held {
            let all_ready = self.panes.iter().all(|p| p.is_next_cached(-1));
            if all_ready {
                let any_advanced = self.panes.iter_mut().fold(false, |acc, p| p.navigate(-1) || acc);
                if any_advanced {
                    self.perf.record_image_load(0.0);
                }
            }
            let any_can = self.panes.iter().any(|p| p.can_navigate_backward());
            if any_can {
                ctx.request_repaint();
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.first() {
            if let Some(path) = &file.path {
                if self.panes.len() == 1 && self.panes[0].image_paths.is_empty() {
                    self.panes[0].open_path(path, ctx);
                } else if self.panes.len() == 1 {
                    // Second drop creates a new pane
                    let mut pane = PaneState::new();
                    pane.open_path(path, ctx);
                    self.panes.push(pane);
                } else {
                    // Already dual — replace pane 0
                    self.panes[0].open_path(path, ctx);
                }
            }
        }
    }

    fn update_title(&self, ctx: &egui::Context) {
        let parts: Vec<String> = self
            .panes
            .iter()
            .filter_map(|pane| {
                pane.image_paths.get(pane.current_index).map(|path| {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    format!("{} ({}/{})", name, pane.current_index + 1, pane.image_paths.len())
                })
            })
            .collect();

        let title = if parts.is_empty() {
            "viewskater-egui".to_string()
        } else {
            format!("{} - viewskater-egui", parts.join(" | "))
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    fn show_bottom_panel(&mut self, ctx: &egui::Context) {
        let max_images = self
            .panes
            .iter()
            .map(|p| p.image_paths.len())
            .max()
            .unwrap_or(0);
        if max_images <= 1 {
            return;
        }

        let current_idx = self.panes.first().map_or(0, |p| p.current_index);
        let mut slider_target = None;
        let mut slider_released = false;

        egui::TopBottomPanel::bottom("nav").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let mut idx = current_idx;
                let max = max_images - 1;
                let response =
                    ui.add(egui::Slider::new(&mut idx, 0..=max).show_value(false));
                if response.changed() {
                    slider_target = Some(idx);
                }
                if response.drag_stopped() {
                    slider_released = true;
                }
                ui.label(format!("{} / {}", current_idx + 1, max_images));
            });
        });

        if let Some(idx) = slider_target {
            let mut any_loaded = false;
            for pane in &mut self.panes {
                let clamped = idx.min(pane.image_paths.len().saturating_sub(1));
                if clamped != pane.current_index {
                    pane.current_index = clamped;
                    pane.zoom = 1.0;
                    pane.pan = egui::Vec2::ZERO;
                    pane.load_sync(ctx);
                    any_loaded = true;
                }
            }
            if any_loaded {
                self.perf.record_image_load(0.0);
            }
        }

        if slider_released {
            for pane in &mut self.panes {
                if let Some(cache) = &mut pane.cache {
                    cache.jump_to(pane.current_index, &pane.image_paths);
                    if let Some(t) = cache.current_texture_for(pane.current_index) {
                        pane.current_texture = Some(t);
                    }
                }
            }
        }
    }

    fn show_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_gray(20)))
            .show(ctx, |ui| {
                if self.panes.len() <= 1 {
                    if let Some(pane) = self.panes.first_mut() {
                        pane.show_content(ui);
                    }
                } else {
                    let available = ui.available_rect_before_wrap();
                    let divider_w = 4.0;
                    let grab_w = 12.0; // wider hit area for easy grabbing
                    let left_w = (available.width() - divider_w) * self.divider_fraction;

                    let left_rect = egui::Rect::from_min_size(
                        available.min,
                        egui::vec2(left_w, available.height()),
                    );
                    let right_rect = egui::Rect::from_min_size(
                        egui::pos2(available.min.x + left_w + divider_w, available.min.y),
                        egui::vec2(available.width() - left_w - divider_w, available.height()),
                    );

                    // Divider interaction — wide grab area centered on the visual line
                    let divider_center_x = available.min.x + left_w + divider_w / 2.0;
                    let grab_rect = egui::Rect::from_center_size(
                        egui::pos2(divider_center_x, available.center().y),
                        egui::vec2(grab_w, available.height()),
                    );
                    let divider_response =
                        ui.allocate_rect(grab_rect, egui::Sense::click_and_drag());

                    if divider_response.dragged() {
                        let usable = available.width() - divider_w;
                        if usable > 0.0 {
                            let delta = divider_response.drag_delta().x;
                            self.divider_fraction =
                                (self.divider_fraction + delta / usable).clamp(0.1, 0.9);
                        }
                    }

                    // Double-click resets to 50/50
                    if divider_response.double_clicked() {
                        self.divider_fraction = 0.5;
                    }

                    // Resize cursor when hovering or dragging
                    if divider_response.hovered() || divider_response.dragged() {
                        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }

                    // Visual divider line — highlighted when interacting
                    let divider_color = if divider_response.dragged() {
                        egui::Color32::from_gray(140)
                    } else if divider_response.hovered() {
                        egui::Color32::from_gray(100)
                    } else {
                        egui::Color32::from_gray(60)
                    };
                    ui.painter().vline(
                        divider_center_x,
                        available.y_range(),
                        egui::Stroke::new(divider_w, divider_color),
                    );

                    let (first, rest) = self.panes.split_at_mut(1);
                    ui.allocate_new_ui(
                        egui::UiBuilder::new().max_rect(left_rect),
                        |ui| first[0].show_content(ui),
                    );
                    ui.allocate_new_ui(
                        egui::UiBuilder::new().max_rect(right_rect),
                        |ui| rest[0].show_content(ui),
                    );
                }
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for pane in &mut self.panes {
            pane.poll_cache();
        }

        self.handle_dropped_files(ctx);
        self.handle_keyboard(ctx);
        self.update_title(ctx);
        self.show_bottom_panel(ctx);
        self.show_central_panel(ctx);
        self.perf.show_overlay(ctx);

        if let Some(pane) = self.panes.first() {
            if let Some(cache) = &pane.cache {
                cache.show_debug_overlay(ctx, pane.current_index, pane.image_paths.len());
            }
        }
    }
}

fn main() -> eframe::Result {
    env_logger::init();
    let args = Args::parse();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "viewskater-egui",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, args.paths)))),
    )
}
