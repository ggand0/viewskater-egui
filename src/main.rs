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
    /// Path to an image file or directory of images
    path: Option<PathBuf>,
}

struct App {
    image_paths: Vec<PathBuf>,
    current_index: usize,
    current_texture: Option<egui::TextureHandle>,
    zoom: f32,
    pan: egui::Vec2,

    cache: Option<cache::SlidingWindowCache>,
    perf: perf::ImagePerfTracker,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self {
        let mut app = Self {
            image_paths: Vec::new(),
            current_index: 0,
            current_texture: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            cache: None,
            perf: perf::ImagePerfTracker::new(),
        };

        if let Some(path) = path {
            app.open_path(&path, &cc.egui_ctx);
        }

        app
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
                    .position(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()) == Some(name.clone()))
            })
            .unwrap_or(0);

        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;

        let mut c = cache::SlidingWindowCache::new(ctx);
        c.initialize(self.current_index, &self.image_paths);
        self.current_texture = c.current_texture_for(self.current_index);
        self.cache = Some(c);

        if self.current_texture.is_some() {
            self.perf.record_image_load(0.0);
        }
    }

    /// Synchronous decode fallback for cache misses.
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
                self.perf.record_image_load(decode_ms);
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
                self.perf.last_decode_ms = None;
            }
        }
    }

    fn navigate(&mut self, delta: isize, ctx: &egui::Context) {
        if self.image_paths.is_empty() {
            return;
        }
        let new_index = (self.current_index as isize + delta)
            .clamp(0, self.image_paths.len() as isize - 1) as usize;
        if new_index == self.current_index {
            return;
        }

        self.current_index = new_index;
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;

        if let Some(cache) = &mut self.cache {
            let tex = if delta > 0 {
                cache.navigate_forward(new_index, &self.image_paths)
            } else {
                cache.navigate_backward(new_index, &self.image_paths)
            };

            if let Some(t) = tex {
                self.current_texture = Some(t);
                self.perf.record_image_load(0.0); // Cache hit
            } else {
                log::debug!("Cache miss at index {}, falling back to sync decode", new_index);
                self.load_sync(ctx);
            }
        } else {
            self.load_sync(ctx);
        }
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
            if self.current_texture.is_some() {
                self.perf.record_image_load(0.0);
            } else {
                self.load_sync(ctx);
            }
        } else {
            self.load_sync(ctx);
        }
    }

    fn update_title(&self, ctx: &egui::Context) {
        if let Some(path) = self.image_paths.get(self.current_index) {
            let filename = path.file_name().unwrap_or_default().to_string_lossy();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                "{} ({}/{}) - viewskater-egui",
                filename,
                self.current_index + 1,
                self.image_paths.len()
            )));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                "viewskater-egui".to_string(),
            ));
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.first() {
            if let Some(path) = &file.path {
                self.open_path(path, ctx);
            }
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let (right, left, home, end) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
            )
        });

        if right {
            self.navigate(1, ctx);
        } else if left {
            self.navigate(-1, ctx);
        } else if home {
            self.jump_to(0, ctx);
        } else if end {
            self.jump_to(self.image_paths.len().saturating_sub(1), ctx);
        }
    }

    fn show_bottom_panel(&mut self, ctx: &egui::Context) {
        if self.image_paths.len() <= 1 {
            return;
        }

        let mut slider_target = None;
        let mut slider_released = false;

        egui::TopBottomPanel::bottom("nav").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let mut idx = self.current_index;
                let max = self.image_paths.len() - 1;
                let response = ui.add(
                    egui::Slider::new(&mut idx, 0..=max).show_value(false),
                );
                if response.changed() {
                    slider_target = Some(idx);
                }
                if response.drag_stopped() {
                    slider_released = true;
                }
                ui.label(format!(
                    "{} / {}",
                    self.current_index + 1,
                    self.image_paths.len()
                ));
            });
        });

        if let Some(idx) = slider_target {
            let idx = idx.min(self.image_paths.len().saturating_sub(1));
            if idx != self.current_index {
                self.current_index = idx;
                self.zoom = 1.0;
                self.pan = egui::Vec2::ZERO;
                // During drag: sync decode only, don't rebuild cache
                self.load_sync(ctx);
            }
        }

        if slider_released {
            // Rebuild cache around the final slider position
            if let Some(cache) = &mut self.cache {
                cache.jump_to(self.current_index, &self.image_paths);
                if let Some(t) = cache.current_texture_for(self.current_index) {
                    self.current_texture = Some(t);
                }
            }
        }
    }

    fn show_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_gray(20)))
            .show(ctx, |ui| {
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
            });
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

        // Allocate interaction area first so we can process input before painting
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

                // Keep the point under the cursor fixed
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

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll background decode completions
        if let Some(cache) = &mut self.cache {
            cache.poll(&self.image_paths);
        }

        self.handle_dropped_files(ctx);
        self.handle_keyboard(ctx);
        self.update_title(ctx);
        self.show_bottom_panel(ctx);
        self.show_central_panel(ctx);
        self.perf.show_overlay(ctx);
        if let Some(cache) = &self.cache {
            cache.show_debug_overlay(ctx, self.current_index, self.image_paths.len());
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
        Box::new(move |cc| Ok(Box::new(App::new(cc, args.path)))),
    )
}
