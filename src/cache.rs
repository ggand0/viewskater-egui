use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use eframe::egui;

const DEFAULT_CACHE_COUNT: usize = 5;

const COL_LOADED: egui::Color32 = egui::Color32::from_rgb(76, 175, 80);
const COL_LOADING: egui::Color32 = egui::Color32::from_rgb(255, 183, 77);
const COL_EMPTY: egui::Color32 = egui::Color32::from_rgb(60, 60, 60);

pub struct DecodeResult {
    pub file_index: usize,
    pub image: Option<egui::ColorImage>,
    #[allow(dead_code)]
    pub decode_ms: f64,
}

/// Sliding window cache that preloads neighboring images in background threads.
///
/// The window has `cache_count * 2 + 1` slots. `first_file_index` tracks which
/// file index slot 0 maps to. The current image sits at slot
/// `current_index - first_file_index`, ideally at the center (`cache_count`),
/// but off-center near directory boundaries.
pub struct SlidingWindowCache {
    slots: VecDeque<Option<egui::TextureHandle>>,
    first_file_index: usize,
    cache_count: usize,

    tx: mpsc::Sender<DecodeResult>,
    rx: mpsc::Receiver<DecodeResult>,
    in_flight: HashSet<usize>,

    ctx: egui::Context,
}

impl SlidingWindowCache {
    pub fn new(ctx: &egui::Context) -> Self {
        let cache_count = DEFAULT_CACHE_COUNT;
        let cache_size = cache_count * 2 + 1;
        let (tx, rx) = mpsc::channel();

        Self {
            slots: VecDeque::from(vec![None; cache_size]),
            first_file_index: 0,
            cache_count,
            tx,
            rx,
            in_flight: HashSet::new(),
            ctx: ctx.clone(),
        }
    }

    fn cache_size(&self) -> usize {
        self.cache_count * 2 + 1
    }

    /// Initialize the cache centered on `center_index`.
    /// Synchronously decodes the center image, spawns background loads for neighbors.
    pub fn initialize(&mut self, center_index: usize, image_paths: &[PathBuf]) {
        let num_files = image_paths.len();
        if num_files == 0 {
            return;
        }

        // Drain any pending results from previous window
        while self.rx.try_recv().is_ok() {}
        self.in_flight.clear();

        let cache_size = self.cache_size();

        // Position the window so center_index is at slot cache_count (center),
        // clamped to valid range
        let max_first = num_files.saturating_sub(cache_size);
        self.first_file_index = (center_index.saturating_sub(self.cache_count)).min(max_first);

        // Clear all slots
        self.slots.clear();
        self.slots.resize(cache_size, None);

        // Synchronously decode the center image
        let center_slot = center_index - self.first_file_index;
        if let Some(tex) = Self::decode_sync(&image_paths[center_index], &self.ctx) {
            self.slots[center_slot] = Some(tex);
        }

        // Spawn background loads for all other valid slots
        for i in 0..cache_size {
            if i == center_slot {
                continue;
            }
            let file_index = self.first_file_index + i;
            if file_index < num_files {
                self.spawn_load(file_index, &image_paths[file_index]);
            }
        }
    }

    /// Poll for completed background decodes and upload textures.
    /// Call this every frame from `update()`.
    pub fn poll(&mut self, image_paths: &[PathBuf]) {
        while let Ok(result) = self.rx.try_recv() {
            self.in_flight.remove(&result.file_index);

            let slot_idx = self.slot_index_for(result.file_index);
            if let Some(slot_idx) = slot_idx {
                if let Some(color_image) = result.image {
                    let name = image_paths
                        .get(result.file_index)
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let texture = self.ctx.load_texture(
                        name,
                        color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.slots[slot_idx] = Some(texture);
                }
            }
            // else: stale result for index outside current window, drop
        }
    }

    /// Shift the cache window for forward navigation.
    /// Returns the TextureHandle for the new current image, or None on cache miss.
    pub fn navigate_forward(
        &mut self,
        new_index: usize,
        image_paths: &[PathBuf],
    ) -> Option<egui::TextureHandle> {
        let num_files = image_paths.len();
        let current_slot = new_index - self.first_file_index;

        if current_slot > self.cache_count {
            // Shift window right
            self.slots.pop_front();
            self.slots.push_back(None);
            self.first_file_index += 1;

            // Spawn load for new rightmost slot
            let new_file_index = self.first_file_index + self.cache_size() - 1;
            if new_file_index < num_files {
                self.spawn_load(new_file_index, &image_paths[new_file_index]);
            }
        }

        self.current_texture_for(new_index)
    }

    /// Shift the cache window for backward navigation.
    /// Returns the TextureHandle for the new current image, or None on cache miss.
    pub fn navigate_backward(
        &mut self,
        new_index: usize,
        image_paths: &[PathBuf],
    ) -> Option<egui::TextureHandle> {
        let current_slot = new_index - self.first_file_index;

        if current_slot < self.cache_count && self.first_file_index > 0 {
            // Shift window left
            self.slots.pop_back();
            self.slots.push_front(None);
            self.first_file_index -= 1;

            // Spawn load for new leftmost slot
            self.spawn_load(self.first_file_index, &image_paths[self.first_file_index]);
        }

        self.current_texture_for(new_index)
    }

    /// Rebuild cache around a new position (slider release, Home/End).
    pub fn jump_to(&mut self, new_index: usize, image_paths: &[PathBuf]) {
        self.initialize(new_index, image_paths);
    }

    /// Get the TextureHandle for a given file index, if cached.
    pub fn current_texture_for(&self, file_index: usize) -> Option<egui::TextureHandle> {
        let slot_idx = file_index.checked_sub(self.first_file_index)?;
        self.slots.get(slot_idx).and_then(|opt| opt.clone())
    }

    /// Find which slot (if any) holds the given file index.
    fn slot_index_for(&self, file_index: usize) -> Option<usize> {
        if file_index < self.first_file_index {
            return None;
        }
        let idx = file_index - self.first_file_index;
        if idx < self.slots.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// Spawn a background thread to decode an image.
    fn spawn_load(&mut self, file_index: usize, path: &Path) {
        if self.in_flight.contains(&file_index) {
            return;
        }
        self.in_flight.insert(file_index);

        let path = path.to_path_buf();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();

        std::thread::spawn(move || {
            let start = Instant::now();
            let image = match image::open(&path) {
                Ok(img) => {
                    Some(crate::decode::image_to_color_image(img))
                }
                Err(e) => {
                    log::warn!("Background decode failed for {}: {}", path.display(), e);
                    None
                }
            };
            let decode_ms = start.elapsed().as_secs_f64() * 1000.0;
            let _ = tx.send(DecodeResult {
                file_index,
                image,
                decode_ms,
            });
            ctx.request_repaint();
        });
    }

    /// Draw debug overlay visualizing cache slot states.
    pub fn show_debug_overlay(&self, ctx: &egui::Context, current_index: usize, num_files: usize) {
        let cache_size = self.cache_size();

        egui::Window::new("cache_state")
            .title_bar(false)
            .resizable(false)
            .anchor(egui::Align2::LEFT_TOP, [10.0, 36.0])
            .interactable(false)
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_black_alpha(200))
                    .corner_radius(6.0)
                    .inner_margin(10.0),
            )
            .show(ctx, |ui| {
                let last_file = self.first_file_index + cache_size - 1;
                ui.label(
                    egui::RichText::new(format!(
                        "Cache [{}\u{2013}{}]",
                        self.first_file_index,
                        last_file.min(num_files.saturating_sub(1))
                    ))
                    .monospace()
                    .color(egui::Color32::from_gray(200))
                    .size(12.0),
                );

                ui.add_space(4.0);

                // Slot cells
                let cell_w: f32 = 28.0;
                let cell_h: f32 = 20.0;
                let gap: f32 = 2.0;
                let label_h: f32 = 12.0;
                let total_w = cache_size as f32 * (cell_w + gap) - gap;
                let total_h = cell_h + gap + label_h;

                let (area, _) = ui.allocate_exact_size(
                    egui::vec2(total_w, total_h),
                    egui::Sense::hover(),
                );

                let painter = ui.painter();

                for i in 0..cache_size {
                    let file_index = self.first_file_index + i;
                    let is_current = file_index == current_index;
                    let is_loaded = self.slots.get(i).is_some_and(|s| s.is_some());
                    let is_in_flight = self.in_flight.contains(&file_index);
                    let is_valid = file_index < num_files;

                    let x = area.min.x + i as f32 * (cell_w + gap);
                    let cell_rect = egui::Rect::from_min_size(
                        egui::pos2(x, area.min.y),
                        egui::vec2(cell_w, cell_h),
                    );

                    let fill = if !is_valid {
                        egui::Color32::from_gray(25)
                    } else if is_loaded {
                        COL_LOADED
                    } else if is_in_flight {
                        COL_LOADING
                    } else {
                        COL_EMPTY
                    };

                    painter.rect_filled(cell_rect, 3.0, fill);

                    if is_current {
                        painter.rect_stroke(
                            cell_rect,
                            3.0,
                            egui::Stroke::new(2.0, egui::Color32::WHITE),
                            egui::epaint::StrokeKind::Outside,
                        );
                    }

                    if is_valid {
                        painter.text(
                            egui::pos2(x + cell_w / 2.0, area.min.y + cell_h + gap),
                            egui::Align2::CENTER_TOP,
                            file_index.to_string(),
                            egui::FontId::monospace(9.0),
                            if is_current {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_gray(120)
                            },
                        );
                    }
                }

                ui.add_space(4.0);

                // Legend
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    legend_swatch(ui, COL_LOADED, "Loaded");
                    ui.add_space(4.0);
                    legend_swatch(ui, COL_LOADING, "Loading");
                    ui.add_space(4.0);
                    legend_swatch(ui, COL_EMPTY, "Empty");
                });
            });
    }

    /// Synchronously decode an image and upload as a texture.
    fn decode_sync(path: &Path, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        match image::open(path) {
            Ok(img) => {
                let color_image = crate::decode::image_to_color_image(img);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Some(ctx.load_texture(name, color_image, egui::TextureOptions::LINEAR))
            }
            Err(e) => {
                log::error!("Failed to decode {}: {}", path.display(), e);
                None
            }
        }
    }
}

/// Throttled synchronous slider loader.
///
/// Reproduces the iced viewskater's slider pattern adapted for egui:
/// In iced, async tasks just wrap raw bytes into Handles (~5ms), and iced's
/// engine lazily decodes only the latest Handle during its prepare phase —
/// so only one decode per render frame actually happens. Since egui has no
/// deferred decode pipeline, we achieve the equivalent by doing sync decode
/// of the latest slider position, throttled to limit how often we block.
pub struct SliderLoader {
    last_load: Instant,
}

const SLIDER_THROTTLE_MS: u128 = 10;

impl SliderLoader {
    pub fn new(_ctx: &egui::Context) -> Self {
        Self {
            last_load: Instant::now(),
        }
    }

    /// Returns true if enough time has passed since the last decode.
    pub fn should_load(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now
            .checked_duration_since(self.last_load)
            .map(|d| d.as_millis())
            .unwrap_or(SLIDER_THROTTLE_MS);

        if elapsed >= SLIDER_THROTTLE_MS {
            self.last_load = now;
            true
        } else {
            false
        }
    }

    /// Reset on slider release.
    pub fn cancel(&mut self) {
        // nothing to clean up
    }
}

/// LRU cache of decoded images, keyed by file index.
///
/// Stores decoded `ColorImage` in CPU memory so that revisiting an image
/// during slider scrubbing skips the ~90ms decode. This is the egui
/// equivalent of iced's raster cache where `Memory::Device(entry)` returns
/// instantly for previously-loaded images.
///
/// Memory budget: a 4K RGBA8 image is ~32MB. With capacity 50 that's ~1.6GB
/// worst case. For 1080p images (~8MB each), 50 images = ~400MB.
pub struct DecodeLruCache {
    /// Map from file_index → decoded ColorImage
    entries: HashMap<usize, egui::ColorImage>,
    /// Access order for LRU eviction — most recently used at the back
    order: VecDeque<usize>,
    capacity: usize,
}

const LRU_CAPACITY: usize = 50;

impl DecodeLruCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: LRU_CAPACITY,
        }
    }

    /// Get a decoded image if cached. Moves entry to most-recently-used.
    pub fn get(&mut self, file_index: usize) -> Option<&egui::ColorImage> {
        if self.entries.contains_key(&file_index) {
            // Move to back (most recently used)
            self.order.retain(|&i| i != file_index);
            self.order.push_back(file_index);
            self.entries.get(&file_index)
        } else {
            None
        }
    }

    /// Insert a decoded image. Evicts the least recently used if at capacity.
    pub fn insert(&mut self, file_index: usize, image: egui::ColorImage) {
        if self.entries.contains_key(&file_index) {
            self.order.retain(|&i| i != file_index);
        } else if self.entries.len() >= self.capacity {
            // Evict LRU (front of order)
            if let Some(evicted) = self.order.pop_front() {
                self.entries.remove(&evicted);
            }
        }
        self.entries.insert(file_index, image);
        self.order.push_back(file_index);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

fn legend_swatch(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.label(
        egui::RichText::new(label)
            .color(egui::Color32::from_gray(160))
            .size(10.0),
    );
}
