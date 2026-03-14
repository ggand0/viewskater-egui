use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;

use crate::cache;
use crate::decode::image_to_color_image;
use crate::file_io;

const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 100.0;

pub struct Pane {
    pub image_paths: Vec<PathBuf>,
    pub current_index: usize,
    pub current_texture: Option<egui::TextureHandle>,
    pub zoom: f32,
    pub pan: egui::Vec2,
    pub cache: Option<cache::SlidingWindowCache>,
    pub slider_loader: Option<cache::SliderLoader>,
    pub decode_cache: cache::DecodeLruCache,
    pub cache_count: usize,
    pub lru_capacity: usize,
    pub selected: bool,
}

impl Pane {
    pub fn new(cache_count: usize, lru_capacity: usize) -> Self {
        Self {
            image_paths: Vec::new(),
            current_index: 0,
            current_texture: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            cache: None,
            slider_loader: None,
            decode_cache: cache::DecodeLruCache::new(lru_capacity),
            cache_count,
            lru_capacity,
            selected: true,
        }
    }

    pub fn close(&mut self) {
        self.image_paths.clear();
        self.current_index = 0;
        self.current_texture = None;
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
        self.cache = None;
        self.slider_loader = None;
        self.decode_cache.clear();
    }

    pub fn open_path(&mut self, path: &std::path::Path, ctx: &egui::Context) {
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
        self.decode_cache.clear();

        let mut c = cache::SlidingWindowCache::new(ctx, self.cache_count);
        c.initialize(self.current_index, &self.image_paths);
        self.current_texture = c.current_texture_for(self.current_index);
        self.cache = Some(c);
        self.slider_loader = Some(cache::SliderLoader::new(ctx));
    }

    /// Synchronous decode fallback for slider drag and jump.
    /// Checks the LRU decode cache first to skip the ~90ms decode on revisits.
    /// Reuses the existing TextureHandle via `set()` when possible to
    /// avoid GPU texture allocation overhead on every call.
    pub fn load_sync(&mut self, ctx: &egui::Context) {
        let Some(path) = self.image_paths.get(self.current_index) else {
            return;
        };
        let file_index = self.current_index;

        // Check LRU decode cache first — skip decode on revisits
        if let Some(cached_image) = self.decode_cache.get(file_index) {
            let t0 = Instant::now();
            let cached_image = cached_image.clone();
            let clone_ms = t0.elapsed().as_secs_f64() * 1000.0;

            let t1 = Instant::now();
            if let Some(tex) = &mut self.current_texture {
                tex.set(cached_image, egui::TextureOptions::LINEAR);
            } else {
                self.current_texture = Some(ctx.load_texture(
                    "slider_sync",
                    cached_image,
                    egui::TextureOptions::LINEAR,
                ));
            }
            let upload_ms = t1.elapsed().as_secs_f64() * 1000.0;

            log::debug!(
                "LRU hit [{}]: clone={:.1}ms upload={:.1}ms",
                file_index, clone_ms, upload_ms,
            );
            return;
        }

        let t0 = Instant::now();
        match image::open(path) {
            Ok(img) => {
                let decode_ms = t0.elapsed().as_secs_f64() * 1000.0;

                let t1 = Instant::now();
                let color_image = image_to_color_image(img);
                let convert_ms = t1.elapsed().as_secs_f64() * 1000.0;

                let t2 = Instant::now();
                self.decode_cache.insert(file_index, color_image.clone());
                let cache_ms = t2.elapsed().as_secs_f64() * 1000.0;

                let size = color_image.size;
                let t3 = Instant::now();
                if let Some(tex) = &mut self.current_texture {
                    tex.set(color_image, egui::TextureOptions::LINEAR);
                } else {
                    self.current_texture = Some(ctx.load_texture(
                        "slider_sync",
                        color_image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                let upload_ms = t3.elapsed().as_secs_f64() * 1000.0;

                log::debug!(
                    "load_sync [{}] ({}x{}): decode={:.1}ms convert={:.1}ms cache={:.1}ms upload={:.1}ms total={:.1}ms [LRU: {}]",
                    file_index, size[0], size[1],
                    decode_ms, convert_ms, cache_ms, upload_ms,
                    t0.elapsed().as_secs_f64() * 1000.0,
                    self.decode_cache.len(),
                );
            }
            Err(e) => {
                log::error!("Failed to load {}: {}", path.display(), e);
                self.current_texture = None;
            }
        }
    }

    /// Try to navigate by `delta` images. Returns true if the display advanced.
    pub fn navigate(&mut self, delta: isize) -> bool {
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

    pub fn jump_to(&mut self, index: usize, ctx: &egui::Context) {
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

    pub fn can_navigate_forward(&self) -> bool {
        !self.image_paths.is_empty()
            && self.current_index < self.image_paths.len() - 1
    }

    pub fn can_navigate_backward(&self) -> bool {
        !self.image_paths.is_empty() && self.current_index > 0
    }

    /// Check whether the next image in the given direction is cached and ready.
    pub fn is_next_cached(&self, delta: isize) -> bool {
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
            .is_some_and(|c| c.current_texture_for(new_index).is_some())
    }

    pub fn poll_cache(&mut self) {
        if let Some(cache) = &mut self.cache {
            cache.poll(&self.image_paths);
        }
    }

    pub fn show_content(&mut self, ui: &mut egui::Ui) {
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

        // Clip to the pane rect so zoomed images don't bleed into adjacent panes
        let painter = ui.painter_at(available);
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex.id(), display_rect, uv, egui::Color32::WHITE);
    }
}
