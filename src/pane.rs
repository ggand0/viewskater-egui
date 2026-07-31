use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;

use crate::animation::{AnimationPlayer, AnimationPoll};
use crate::cache;
use crate::decode::image_to_color_image;
use crate::file_io::{self, open_image};
use crate::settings::{ImageDiscoveryOptions};

const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 100.0;

pub(crate) struct Pane {
    /// Top level directory from which the pane loaded files
    pub(crate) dir_path: Option<PathBuf>,
    pub(crate) image_paths: Vec<PathBuf>,
    pub(crate) current_index: usize,
    pub(crate) current_texture: Option<egui::TextureHandle>,
    animation: Option<AnimationPlayer>,
    pub(crate) zoom: f32,
    pub(crate) pan: egui::Vec2,
    pub(crate) cache: Option<cache::SlidingWindowCache>,
    pub(crate) thumbnail_cache: Option<cache::ThumbnailCache>,
    slider_loader: Option<cache::SliderLoader>,
    pub(crate) decode_cache: cache::DecodeLruCache,
    pub(crate) cache_count: usize,
    pub(crate) lru_budget_mb: usize,
    pub(crate) decode_threads: usize,
    pub(crate) selected: bool,
    pub(crate) mouse_wheel_zoom: bool,
    pub(crate) reset_zoom_pan_on_navigation: bool,
    pub(crate) preview_budget_mb: usize,
}

impl Pane {
    pub(crate) fn new(
        ctx: &egui::Context,
        cache_count: usize,
        lru_budget_mb: usize,
        decode_threads: usize,
        mouse_wheel_zoom: bool,
        reset_zoom_pan_on_navigation: bool,
        preview_budget_mb: usize,
    ) -> Self {
        Self {
            dir_path: None,
            image_paths: Vec::new(),
            current_index: 0,
            current_texture: None,
            animation: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            cache: None,
            thumbnail_cache: None,
            slider_loader: None,
            decode_cache: cache::DecodeLruCache::new(ctx, lru_budget_mb),
            cache_count,
            lru_budget_mb,
            decode_threads,
            selected: true,
            mouse_wheel_zoom,
            reset_zoom_pan_on_navigation,
            preview_budget_mb,
        }
    }

    pub(crate) fn close(&mut self) {
        self.image_paths.clear();
        self.current_index = 0;
        self.current_texture = None;
        self.animation = None;
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
        self.cache = None;
        self.thumbnail_cache = None;
        self.slider_loader = None;
        self.decode_cache.clear();
    }

    pub(crate) fn open_path(
        &mut self,
        path: &std::path::Path,
        ctx: &egui::Context,
        discovery_options: ImageDiscoveryOptions,
    ) {
        if !path.exists() {
            log::error!("Path does not exist: {}", path.display());
            return;
        }

        let (dir, target_filename) = file_io::resolve_path(path);
        self.image_paths = file_io::enumerate_images(&dir, discovery_options);

        if self.image_paths.is_empty() {
            log::warn!("No supported images found in {}", dir.display());
            return;
        }
        self.dir_path = Some(dir);

        self.current_index = target_filename
            .and_then(|name| {
                self.image_paths.iter().position(|p| {
                    p.file_name().map(|f| f.to_string_lossy().into_owned()) == Some(name.clone())
                })
            })
            .unwrap_or(0);

        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
        self.decode_cache.clear();
        self.animation = None;

        let mut c = cache::SlidingWindowCache::new(ctx, self.cache_count, self.decode_threads);
        c.initialize(self.current_index, &self.image_paths);
        self.set_current_texture(c.current_texture_for(self.current_index), ctx);
        self.cache = Some(c);
        self.thumbnail_cache = Some(cache::ThumbnailCache::new(ctx, self.preview_budget_mb));
        self.slider_loader = Some(cache::SliderLoader::new(ctx));
    }

    /// Synchronous decode fallback for slider drag and jump.
    /// Checks the GPU-backed LRU first to skip both decode and re-upload on
    /// revisits. On miss, decodes from disk and uploads a new texture via
    /// `DecodeLruCache::insert`, which also handles budget eviction.
    fn load_sync(&mut self, ctx: &egui::Context) {
        let Some(path) = self.image_paths.get(self.current_index).cloned() else {
            return;
        };
        let file_index = self.current_index;

        // LRU hit — texture is already on the GPU, no upload.
        if let Some(cached_handle) = self.decode_cache.get(file_index) {
            self.set_current_texture(Some(cached_handle), ctx);
            log::debug!("LRU hit [{}]", file_index);
            return;
        }

        let t0 = Instant::now();
        match open_image(&path) {
            Ok(img) => {
                let decode_ms = t0.elapsed().as_secs_f64() * 1000.0;

                let t1 = Instant::now();
                let color_image = image_to_color_image(img);
                let convert_ms = t1.elapsed().as_secs_f64() * 1000.0;

                let size = color_image.size;
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "slider_sync".into());

                let t2 = Instant::now();
                let handle = self.decode_cache.insert(file_index, name, color_image);
                let upload_ms = t2.elapsed().as_secs_f64() * 1000.0;
                self.set_current_texture(Some(handle), ctx);

                log::debug!(
                    "load_sync [{}] ({}x{}): decode={:.1}ms convert={:.1}ms upload={:.1}ms total={:.1}ms [LRU: {} / {:.0} MB]",
                    file_index, size[0], size[1],
                    decode_ms, convert_ms, upload_ms,
                    t0.elapsed().as_secs_f64() * 1000.0,
                    self.decode_cache.len(),
                    self.decode_cache.total_mb(),
                );
            }
            Err(e) => {
                log::error!("Failed to load {}: {}", path.display(), e);
                self.set_current_texture(None, ctx);
            }
        }
    }

    fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
    }

    /// Try to navigate by `delta` images. Returns true if the display advanced.
    pub(crate) fn navigate(&mut self, delta: isize, ctx: &egui::Context) -> bool {
        if self.image_paths.is_empty() {
            return false;
        }
        let new_index = (self.current_index as isize + delta)
            .clamp(0, self.image_paths.len() as isize - 1) as usize;
        if new_index == self.current_index {
            return false;
        }

        if let Some(t) = self
            .cache
            .as_ref()
            .and_then(|cache| cache.current_texture_for(new_index))
        {
            self.current_index = new_index;

            if let Some(cache) = &mut self.cache {
                if delta > 0 {
                    cache.navigate_forward(new_index, &self.image_paths);
                } else {
                    cache.navigate_backward(new_index, &self.image_paths);
                }

                let summary = cache.summary();

                if self.reset_zoom_pan_on_navigation {
                    self.reset_view();
                }

                let dir = if delta > 0 { "→" } else { "←" };
                log::debug!(
                    "nav {} {}/{} cache={} hit",
                    dir,
                    new_index,
                    self.image_paths.len(),
                    summary,
                );
            }
            self.set_current_texture(Some(t), ctx);
            return true;
        }
        false
    }

    pub(crate) fn jump_to(&mut self, index: usize, ctx: &egui::Context) {
        let index = index.min(self.image_paths.len().saturating_sub(1));
        if index == self.current_index {
            return;
        }

        self.current_index = index;

        if self.reset_zoom_pan_on_navigation {
            self.reset_view();
        }

        if let Some(cache) = &mut self.cache {
            cache.jump_to(index, &self.image_paths);
            let texture = cache.current_texture_for(index);
            let hit = texture.is_some();
            let summary = cache.summary();
            log::debug!(
                "jump {}/{} cache={} {}",
                index,
                self.image_paths.len(),
                summary,
                if hit { "hit" } else { "miss" },
            );
            if hit {
                self.set_current_texture(texture, ctx);
            } else {
                self.load_sync(ctx);
            }
        } else {
            self.load_sync(ctx);
        }
    }

    pub(crate) fn can_navigate_forward(&self) -> bool {
        !self.image_paths.is_empty() && self.current_index < self.image_paths.len() - 1
    }

    pub(crate) fn can_navigate_backward(&self) -> bool {
        !self.image_paths.is_empty() && self.current_index > 0
    }

    /// Check whether the next image in the given direction is cached and ready.
    pub(crate) fn is_next_cached(&self, delta: isize) -> bool {
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

    /// Returns (lru_mb, sliding_window_mb).
    pub(crate) fn cache_memory_mb(&self) -> (f64, f64) {
        let lru = self.decode_cache.total_mb();
        let sw = self.cache.as_ref().map_or(0.0, |c| c.total_mb());
        (lru, sw)
    }

    pub(crate) fn poll_cache(&mut self) {
        if let Some(cache) = &mut self.cache {
            cache.poll(&self.image_paths);
        }
        if let Some(tc) = &mut self.thumbnail_cache {
            tc.poll();
        }
    }

    /// Drag the slider to `idx`. Returns true if image was loaded.
    pub(crate) fn apply_slider_target(&mut self, idx: usize, ctx: &egui::Context) -> bool {
        let clamped = idx.min(self.image_paths.len().saturating_sub(1));
        if clamped == self.current_index {
            return false;
        }
        self.current_index = clamped;

        if self.reset_zoom_pan_on_navigation {
            self.reset_view();
        }

        let found_in_cache = self
            .cache
            .as_ref()
            .and_then(|c| c.current_texture_for(clamped));

        if let Some(tex) = found_in_cache {
            self.set_current_texture(Some(tex), ctx);
            true
        } else if let Some(loader) = &mut self.slider_loader {
            if loader.should_load() {
                self.load_sync(ctx);
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Finalize after slider drag released: re-center cache.
    pub(crate) fn apply_slider_release(&mut self, ctx: &egui::Context) {
        let texture = if let Some(cache) = &mut self.cache {
            cache.jump_to(self.current_index, &self.image_paths);
            let texture = cache.current_texture_for(self.current_index);
            log::debug!(
                "slider release {}/{} cache={}",
                self.current_index,
                self.image_paths.len(),
                cache.summary(),
            );
            texture
        } else {
            None
        };
        if let Some(texture) = texture {
            self.set_current_texture(Some(texture), ctx);
        }
    }

    /// Show the pane content. Returns true if zoom/pan was changed by user interaction.
    pub(crate) fn show_content(&mut self, ui: &mut egui::Ui) -> bool {
        let tex = self.current_texture.clone();
        if let Some(tex) = tex {
            return self.show_image(ui, &tex);
        }
        if self.image_paths.is_empty() {
            let available = ui.available_width();
            let font = egui::TextStyle::Body.resolve(ui.style());
            let measure = |text: &str| -> f32 {
                ui.fonts(|f| {
                    f.layout_no_wrap(text.into(), font.clone(), egui::Color32::WHITE)
                        .size()
                        .x
                })
            };
            let full = "Drop an image or folder here";
            let short = "Drop image";
            let label = if available >= measure(full) {
                Some(full)
            } else if available >= measure(short) {
                Some(short)
            } else {
                None
            };
            if let Some(text) = label {
                ui.centered_and_justified(|ui| {
                    ui.label(text);
                });
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Failed to load image");
            });
        }
        false
    }

    pub(crate) fn poll_animation(&mut self) {
        let Some(animation) = &mut self.animation else {
            return;
        };
        match animation.poll() {
            AnimationPoll::NewTexture(texture) => self.current_texture = Some(texture),
            AnimationPoll::Finished => self.animation = None,
            AnimationPoll::Unchanged => {}
        }
    }

    fn set_current_texture(&mut self, texture: Option<egui::TextureHandle>, ctx: &egui::Context) {
        self.current_texture = texture;
        self.start_animation(ctx);
    }

    fn start_animation(&mut self, ctx: &egui::Context) {
        self.animation = self
            .image_paths
            .get(self.current_index)
            .cloned()
            .filter(|path| self.current_texture.is_some() && file_io::may_have_animation(path))
            .map(|path| AnimationPlayer::new(path, ctx));
    }

    /// Returns true if the user changed zoom or pan this frame.
    fn show_image(&mut self, ui: &mut egui::Ui, tex: &egui::TextureHandle) -> bool {
        let tex_size = tex.size_vec2();
        let available = ui.available_rect_before_wrap();

        if available.width() <= 0.0 || available.height() <= 0.0 {
            return false;
        }
        if tex_size.x <= 0.0 || tex_size.y <= 0.0 {
            return false;
        }

        let old_zoom = self.zoom;
        let old_pan = self.pan;

        let response = ui.allocate_rect(available, egui::Sense::click_and_drag());
        let scale = (available.width() / tex_size.x).min(available.height() / tex_size.y);

        // Zoom: scroll wheel (when enabled) or Ctrl/Cmd+scroll, plus pinch-to-zoom
        if response.hovered() && (self.mouse_wheel_zoom || ui.input(|i| i.modifiers.command)) {
            self.zoom_image(ui, &response, &available);
        }

        // Pan: drag
        if response.dragged() {
            self.pan += response.drag_delta();
        }

        // Double-click: toggle between fit-to-screen and 1:1.
        if response.double_clicked() {
            let is_fit_to_screen = (self.zoom - 1.0).abs() < f32::EPSILON;
            let actual_size_zoom = (1.0 / scale).clamp(MIN_ZOOM, MAX_ZOOM);

            if is_fit_to_screen {
                self.zoom = actual_size_zoom;
                if self.zoom >= 1.0 {
                    if let Some(hover_pos) = response.hover_pos() {
                        self.pan = (hover_pos - available.center()) * (1.0 - self.zoom);
                    }
                } else {
                    self.pan = egui::Vec2::ZERO;
                }
            } else {
                self.reset_view();
            }
        }

        // Compute display rect with updated zoom/pan (zero-frame-delay)
        let base_size = tex_size * scale;
        let display_size = base_size * self.zoom;
        let center = available.center() + self.pan;
        let display_rect = egui::Rect::from_center_size(center, display_size);

        // Clip to the pane rect so zoomed images don't bleed into adjacent panes
        let painter = ui.painter_at(available);
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex.id(), display_rect, uv, egui::Color32::WHITE);

        self.zoom != old_zoom || self.pan != old_pan
    }

    // Zoom: scroll wheel + pinch-to-zoom
    fn zoom_image(&mut self, ui: &mut egui::Ui, response: &egui::Response, available: &egui::Rect) {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let scroll_zoom_speed = ui.ctx().options(|o| o.scroll_zoom_speed);
        let scroll_factor = (scroll * scroll_zoom_speed).exp();
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
}
