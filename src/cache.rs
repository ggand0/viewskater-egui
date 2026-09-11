use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use eframe::egui;

use crate::file_io::open_image;

#[cfg(test)]
mod preview_sim_bench;

const COL_LOADED: egui::Color32 = egui::Color32::from_rgb(76, 175, 80);
const COL_LOADING: egui::Color32 = egui::Color32::from_rgb(255, 183, 77);
const COL_EMPTY: egui::Color32 = egui::Color32::from_rgb(60, 60, 60);

pub struct ThumbnailCache {
    ctx: egui::Context,
    texture: Option<egui::TextureHandle>,
    texture_idx: Option<usize>,
    cache: HashMap<usize, egui::ColorImage>,
    cache_bytes: usize,
    req_tx: mpsc::Sender<(usize, PathBuf)>,
    res_rx: mpsc::Receiver<(usize, PathBuf, Option<egui::ColorImage>)>,
    /// Index of the most recently sent request whose result hasn't arrived
    /// yet. Prevents re-sending the same request every hovered frame, which
    /// made the worker decode the same image twice.
    pending_idx: Option<usize>,
    preview_budget_mb: usize
}

impl ThumbnailCache {
    pub fn new(ctx: &egui::Context, preview_budget_mb: usize) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<(usize, PathBuf)>();
        let (res_tx, res_rx) = mpsc::channel();

        let worker_ctx = ctx.clone();
        std::thread::spawn(move || {
            while let Ok((idx, path)) = req_rx.recv() {
                let mut latest_idx = idx;
                let mut latest_path = path;
                while let Ok((newer_idx, newer_path)) = req_rx.try_recv() {
                    latest_idx = newer_idx;
                    latest_path = newer_path;
                }
                // Send a result even on failure so the pending marker clears
                // and the index can be retried later.
                let thumbnail = match image::open(&latest_path) {
                    Ok(img) => Some(crate::decode::image_to_thumbnail(img)),
                    Err(e) => {
                        log::error!("Thumbnail decode failed for {}: {e}", latest_path.display());
                        None
                    }
                };
                let _ = res_tx.send((latest_idx, latest_path, thumbnail));
                worker_ctx.request_repaint();
            }
        });

        Self {
            ctx: ctx.clone(),
            texture: None,
            texture_idx: None,
            cache: HashMap::new(),
            cache_bytes: 0,
            req_tx,
            res_rx,
            pending_idx: None,
            preview_budget_mb,
        }
    }

    /// Drain finished thumbnails. `image_paths` is the pane's current list;
    /// a result whose path no longer sits at its index (the list changed
    /// under the worker, e.g. a file was moved to the trash) is dropped and
    /// re-requested on the next hover.
    pub fn poll(&mut self, image_paths: &[PathBuf]) {
        while let Ok((idx, path, img)) = self.res_rx.try_recv() {
            // Only clear when it matches: a newer request may already be
            // pending for a different index.
            if self.pending_idx == Some(idx) {
                self.pending_idx = None;
            }
            if image_paths.get(idx) != Some(&path) {
                log::debug!("thumb drop stale [{}] {}", idx, path.display());
                continue;
            }
            let Some(img) = img else { continue };
            let img_bytes = img.pixels.len() * 4;
            if let Some(old) = self.cache.insert(idx, img.clone()) {
                self.cache_bytes -= old.pixels.len() * 4;
            }
            self.cache_bytes += img_bytes;
            if self.preview_budget_mb > 0 {
                evict_thumb_cache(&mut self.cache, &mut self.cache_bytes, idx, self.preview_budget_mb);
            }
            self.upload(idx, img);
        }
    }

    pub fn current_thumbnail_for(&mut self, thumb_index: usize, path: &Path) -> (Option<egui::TextureHandle>, bool) {
        if self.texture_idx == Some(thumb_index) {
            return (self.texture.clone(), true);
        }
        if let Some(img) = self.cache.get(&thumb_index) {
            self.upload(thumb_index, img.clone());
            return (self.texture.clone(), true);
        }
        if let Some(&nearest_idx) = self.cache.keys()
            .min_by_key(|&&k| (k as isize - thumb_index as isize).unsigned_abs())
        {
            if self.texture_idx != Some(nearest_idx) {
                let img = self.cache[&nearest_idx].clone();
                self.upload(nearest_idx, img);
            }
        }
        if self.pending_idx != Some(thumb_index)
            && self.req_tx.send((thumb_index, path.to_path_buf())).is_ok()
        {
            self.pending_idx = Some(thumb_index);
        }
        (self.texture.clone(), false)
    }

    fn upload(&mut self, idx: usize, img: egui::ColorImage) {
        match &mut self.texture {
            Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
            None => {
                self.texture = Some(self.ctx.load_texture(
                    "thumb_preview", img, egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.texture_idx = Some(idx);
    }

    /// Change the memory budget (0 = unlimited), evicting entries furthest
    /// from the currently displayed thumbnail if over the new limit.
    pub fn set_budget_mb(&mut self, budget_mb: usize) {
        self.preview_budget_mb = budget_mb;
        if budget_mb > 0 {
            let center = self.texture_idx.unwrap_or(0);
            evict_thumb_cache(&mut self.cache, &mut self.cache_bytes, center, budget_mb);
        }
    }

    /// The file at `removed` left the list: drop its thumbnail and shift
    /// every higher index down by one so cached entries keep pointing at
    /// the same files. A request in flight for the old numbering is
    /// dropped by `poll` when its path no longer matches.
    pub fn remove_index(&mut self, removed: usize) {
        let mut shifted = HashMap::with_capacity(self.cache.len());
        for (idx, img) in self.cache.drain() {
            match shift_index(idx, removed) {
                Some(new_idx) => {
                    shifted.insert(new_idx, img);
                }
                None => self.cache_bytes -= img.pixels.len() * 4,
            }
        }
        self.cache = shifted;
        self.texture_idx = self.texture_idx.and_then(|i| shift_index(i, removed));
        self.pending_idx = None;
    }
}

/// Index of a file after the file at `removed` left the list. `None` for
/// the removed file itself.
fn shift_index(idx: usize, removed: usize) -> Option<usize> {
    use std::cmp::Ordering;
    match idx.cmp(&removed) {
        Ordering::Less => Some(idx),
        Ordering::Equal => None,
        Ordering::Greater => Some(idx - 1),
    }
}

pub struct DecodeResult {
    /// Path the thread decoded. The file index is looked up from it on
    /// arrival, because the list may have changed while decoding.
    pub path: PathBuf,
    pub image: Option<egui::ColorImage>,
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
    /// Decodes running on a thread, keyed by path with the file index the
    /// result belongs to. The index is updated by `remove_index`, so a
    /// result that arrives after the list changed still lands in the
    /// right slot, and a result for a path no longer tracked is dropped.
    in_flight: HashMap<PathBuf, usize>,

    /// Completed decodes waiting for GPU upload. `poll()` drains `rx` into
    /// this queue and uploads up to `UPLOADS_PER_FRAME` per frame.
    pending_uploads: VecDeque<(usize, egui::ColorImage, String)>,

    /// Decode requests waiting for a thread slot. `spawn_load` pushes here
    /// when the concurrent limit is reached; `poll` spawns the next one
    /// when a decode completes and frees a slot.
    pending_decodes: VecDeque<(usize, PathBuf)>,

    max_decode_threads: usize,

    ctx: egui::Context,
}

/// Maximum number of GPU uploads issued by `SlidingWindowCache::poll` per
/// frame.
const UPLOADS_PER_FRAME: usize = 2;


impl SlidingWindowCache {
    pub fn new(ctx: &egui::Context, cache_count: usize, decode_threads: usize) -> Self {
        let cache_size = cache_count * 2 + 1;
        let (tx, rx) = mpsc::channel();

        Self {
            slots: VecDeque::from(vec![None; cache_size]),
            first_file_index: 0,
            cache_count,
            tx,
            rx,
            in_flight: HashMap::new(),
            pending_uploads: VecDeque::new(),
            pending_decodes: VecDeque::new(),
            max_decode_threads: decode_threads,
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
        self.pending_uploads.clear();
        self.pending_decodes.clear();

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
    ///
    /// Drains completed background decodes into `pending_uploads`, then
    /// issues up to `UPLOADS_PER_FRAME` GPU uploads per call.
    pub fn poll(&mut self, image_paths: &[PathBuf]) {
        // Phase 1: drain decode results into the upload queue.
        while let Ok(result) = self.rx.try_recv() {
            let Some(file_index) = self.in_flight.remove(&result.path) else {
                log::debug!("bg decode drop stale {}", result.path.display());
                continue;
            };
            if let Some(color_image) = result.image {
                log::debug!(
                    "bg decode [{}]: {:.1}ms",
                    file_index,
                    result.decode_ms,
                );
                let name = image_paths
                    .get(file_index)
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.pending_uploads
                    .push_back((file_index, color_image, name));
            }

            // A decode slot freed up — spawn the next queued decode if any.
            while self.in_flight.len() < self.max_decode_threads {
                if let Some((idx, path)) = self.pending_decodes.pop_front() {
                    if self.slot_index_for(idx).is_some() {
                        self.spawn_thread(idx, &path);
                    }
                    // else: stale, skip and try the next
                } else {
                    break;
                }
            }
        }

        // Phase 2: upload at most UPLOADS_PER_FRAME.
        for _ in 0..UPLOADS_PER_FRAME {
            let Some((file_index, color_image, name)) = self.pending_uploads.pop_front() else {
                break;
            };
            if let Some(slot_idx) = self.slot_index_for(file_index) {
                if self.slots[slot_idx].is_none() {
                    let texture = self.ctx.load_texture(
                        &name,
                        color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.slots[slot_idx] = Some(texture);
                }
            }
        }

        if !self.pending_uploads.is_empty() || !self.pending_decodes.is_empty() {
            self.ctx.request_repaint();
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

    /// The file at `removed` left the list. `image_paths` is the list after
    /// removal. Every file above `removed` now has an index one lower, so
    /// the window and all bookkeeping shift with it; nothing is re-decoded
    /// except the one file that enters the window to fill the gap.
    ///
    /// Three cases for the window `[first, first + size)`:
    /// - `removed < first`: the window's files are unchanged, only their
    ///   numbering moved. Decrement `first`.
    /// - inside the window: drop that slot. Fill from the right if the
    ///   list still has a file there, else from the left if the window
    ///   can move back, else leave an empty slot (list shorter than the
    ///   window).
    /// - `removed` past the window: nothing changes.
    pub fn remove_index(&mut self, removed: usize, image_paths: &[PathBuf]) {
        let num_files = image_paths.len();
        let size = self.slots.len();
        let first = self.first_file_index;

        // Renumber everything that still refers to file indices before
        // any new load is queued, so the fill load below keeps its index.
        // Entries for the removed file are dropped; a thread still
        // decoding it reports a path no longer in `in_flight` and `poll`
        // ignores it.
        self.in_flight.retain(|_, idx| match shift_index(*idx, removed) {
            Some(new_idx) => {
                *idx = new_idx;
                true
            }
            None => false,
        });
        self.pending_decodes.retain_mut(|(idx, _)| match shift_index(*idx, removed) {
            Some(new_idx) => {
                *idx = new_idx;
                true
            }
            None => false,
        });
        self.pending_uploads.retain_mut(|(idx, _, _)| match shift_index(*idx, removed) {
            Some(new_idx) => {
                *idx = new_idx;
                true
            }
            None => false,
        });

        if removed < first {
            self.first_file_index = first - 1;
        } else if removed < first + size {
            self.slots.remove(removed - first);
            let right = first + size - 1;
            if right < num_files {
                self.slots.push_back(None);
                self.spawn_load(right, &image_paths[right]);
            } else if first > 0 {
                self.first_file_index = first - 1;
                self.slots.push_front(None);
                let left = self.first_file_index;
                self.spawn_load(left, &image_paths[left]);
            } else {
                self.slots.push_back(None);
            }
        }
    }

    /// Change the sliding window half-size and reinitialize around current position.
    pub fn set_cache_count(
        &mut self,
        cache_count: usize,
        current_index: usize,
        image_paths: &[PathBuf],
    ) {
        if self.cache_count == cache_count {
            return;
        }
        self.cache_count = cache_count;
        self.initialize(current_index, image_paths);
    }

    pub fn set_decode_threads(&mut self, n: usize) {
        self.max_decode_threads = n.max(1);
    }

    /// Returns a compact summary of the cache window for debug logging.
    /// Format: `[first..last] loaded/total inflight=N`
    pub fn summary(&self) -> String {
        let last = self.first_file_index + self.slots.len().saturating_sub(1);
        let loaded = self.slots.iter().filter(|s| s.is_some()).count();
        let total = self.slots.len();
        if self.in_flight.is_empty() {
            format!("[{}..{}] {}/{}", self.first_file_index, last, loaded, total)
        } else {
            format!(
                "[{}..{}] {}/{} inflight={}",
                self.first_file_index, last, loaded, total, self.in_flight.len()
            )
        }
    }

    /// Total bytes of loaded textures in the sliding window.
    pub fn total_bytes(&self) -> usize {
        self.slots.iter().filter_map(|s| s.as_ref()).map(|tex| {
            let size = tex.size();
            size[0] * size[1] * 4
        }).sum()
    }

    pub fn total_mb(&self) -> f64 {
        self.total_bytes() as f64 / (1024.0 * 1024.0)
    }

    #[cfg(test)]
    pub(crate) fn first_file_index_for_test(&self) -> usize {
        self.first_file_index
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

    /// Queue a background decode. If fewer than `self.max_decode_threads`
    /// threads are running, spawns immediately; otherwise queues until a
    /// slot opens in `poll`.
    fn spawn_load(&mut self, file_index: usize, path: &Path) {
        if self.in_flight.contains_key(path) {
            return;
        }
        if self.pending_decodes.iter().any(|(idx, _)| *idx == file_index) {
            return;
        }

        if self.in_flight.len() < self.max_decode_threads {
            self.spawn_thread(file_index, path);
        } else {
            self.pending_decodes.push_back((file_index, path.to_path_buf()));
        }
    }

    /// Actually spawn the decode thread.
    fn spawn_thread(&mut self, file_index: usize, path: &Path) {
        self.in_flight.insert(path.to_path_buf(), file_index);

        let path = path.to_path_buf();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();

        std::thread::spawn(move || {
            let start = Instant::now();
            let image = match open_image(&path) {
                Ok(img) => Some(crate::decode::image_to_color_image(img)),
                Err(e) => {
                    log::warn!("Background decode failed for {}: {}", path.display(), e);
                    None
                }
            };
            let decode_ms = start.elapsed().as_secs_f64() * 1000.0;
            let _ = tx.send(DecodeResult {
                path,
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
            .auto_sized()
            .anchor(egui::Align2::RIGHT_TOP, [-10.0, 28.0])
            .interactable(false)
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_black_alpha(200))
                    .corner_radius(6.0)
                    .inner_margin(10.0),
            )
            .show(ctx, |ui| {
                let last_file = self.first_file_index + cache_size - 1;
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "Cache [{}\u{2013}{}]",
                            self.first_file_index,
                            last_file.min(num_files.saturating_sub(1))
                        ))
                        .monospace()
                        .color(egui::Color32::from_gray(200))
                        .size(12.0),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
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
                    let is_in_flight = self.in_flight.values().any(|&i| i == file_index);
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
        match open_image(path) {
            Ok(img) => {
                let color_image = crate::decode::image_to_color_image(img);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Some(ctx.load_texture(&name, color_image, egui::TextureOptions::LINEAR))
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

}

/// LRU cache of uploaded GPU textures, keyed by file index.
///
/// On a hit, returns the existing `TextureHandle` directly — no CPU→GPU
/// upload needed. On a miss, uploads the decoded pixels once and stores
/// the resulting handle. LRU eviction drops the handle, which drops the
/// GPU allocation on the next `TextureDelta::Free` tick.
///
/// Uses a byte budget rather than a fixed entry count so the cache
/// self-adjusts for any resolution: 4K textures (~32 MB each) get fewer
/// entries than 1080p (~8 MB each) for the same budget. Bytes are
/// computed as `width × height × 4` (RGBA), matching how egui sizes
/// uploaded textures.
pub struct DecodeLruCache {
    /// Map from file_index → uploaded texture handle.
    entries: HashMap<usize, egui::TextureHandle>,
    /// Access order for LRU eviction — most recently used at the back.
    order: VecDeque<usize>,
    /// Maximum total bytes for cached textures.
    budget_bytes: usize,
    /// Current total bytes of cached textures.
    total_bytes: usize,
    /// egui context for uploading via `load_texture`.
    ctx: egui::Context,
}

impl DecodeLruCache {
    pub fn new(ctx: &egui::Context, budget_mb: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            budget_bytes: budget_mb * 1024 * 1024,
            total_bytes: 0,
            ctx: ctx.clone(),
        }
    }

    /// Byte size of a handle's underlying texture (width × height × 4).
    fn handle_bytes(handle: &egui::TextureHandle) -> usize {
        let size = handle.size();
        size[0] * size[1] * 4
    }

    /// Get the cached texture for `file_index`, if any. Marks MRU.
    pub fn get(&mut self, file_index: usize) -> Option<egui::TextureHandle> {
        if self.entries.contains_key(&file_index) {
            self.order.retain(|&i| i != file_index);
            self.order.push_back(file_index);
            self.entries.get(&file_index).cloned()
        } else {
            None
        }
    }

    /// Upload a decoded image as a new GPU texture, store as MRU, and
    /// evict LRU entries until within budget. Returns the new handle.
    pub fn insert(
        &mut self,
        file_index: usize,
        name: impl Into<String>,
        image: egui::ColorImage,
    ) -> egui::TextureHandle {
        let new_bytes = image.size[0] * image.size[1] * 4;

        // If replacing an existing entry, drop its bytes first.
        if let Some(old) = self.entries.remove(&file_index) {
            self.total_bytes -= Self::handle_bytes(&old);
            self.order.retain(|&i| i != file_index);
        }

        // Evict LRU until there's room. Do this before the upload so the
        // gpu_allocator sees the freed textures first (drop happens
        // synchronously here, but the GPU free is processed at the next
        // TextureDelta flush).
        while self.total_bytes + new_bytes > self.budget_bytes {
            if let Some(evicted) = self.order.pop_front() {
                if let Some(h) = self.entries.remove(&evicted) {
                    self.total_bytes -= Self::handle_bytes(&h);
                }
            } else {
                break;
            }
        }

        let handle = self.ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
        self.total_bytes += new_bytes;
        self.entries.insert(file_index, handle.clone());
        self.order.push_back(file_index);
        handle
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn total_mb(&self) -> f64 {
        self.total_bytes as f64 / (1024.0 * 1024.0)
    }

    /// The file at `removed` left the list: drop its texture and shift
    /// every higher key down by one. LRU order is preserved.
    pub fn remove_index(&mut self, removed: usize) {
        if let Some(handle) = self.entries.remove(&removed) {
            self.total_bytes -= Self::handle_bytes(&handle);
        }
        let mut shifted = HashMap::with_capacity(self.entries.len());
        for (idx, handle) in self.entries.drain() {
            if let Some(new_idx) = shift_index(idx, removed) {
                shifted.insert(new_idx, handle);
            }
        }
        self.entries = shifted;
        self.order = self
            .order
            .iter()
            .filter_map(|&idx| shift_index(idx, removed))
            .collect();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.total_bytes = 0;
    }

    /// Change the memory budget, evicting LRU entries if over the new limit.
    pub fn set_budget_mb(&mut self, budget_mb: usize) {
        self.budget_bytes = budget_mb * 1024 * 1024;
        while self.total_bytes > self.budget_bytes {
            if let Some(evicted) = self.order.pop_front() {
                if let Some(h) = self.entries.remove(&evicted) {
                    self.total_bytes -= Self::handle_bytes(&h);
                }
            } else {
                break;
            }
        }
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

fn evict_thumb_cache(
    cache: &mut HashMap<usize, egui::ColorImage>,
    cache_bytes: &mut usize,
    current_idx: usize,
    cache_budget: usize,
) {
    evict_thumb_cache_with_budget(cache, cache_bytes, current_idx, cache_budget * 1024 * 1024);
}

fn evict_thumb_cache_with_budget(
    cache: &mut HashMap<usize, egui::ColorImage>,
    cache_bytes: &mut usize,
    current_idx: usize,
    budget: usize,
) {
    while *cache_bytes > budget && cache.len() > 1 {
        let furthest = *cache.keys()
            .max_by_key(|&&k| (k as isize - current_idx as isize).unsigned_abs())
            .unwrap();
        if let Some(removed) = cache.remove(&furthest) {
            *cache_bytes -= removed.pixels.len() * 4;
            log::debug!(
                "thumb evict [{}]: cache={} entries, {:.1}MB",
                furthest, cache.len(),
                *cache_bytes as f64 / (1024.0 * 1024.0),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BUDGET: usize = 1024;

    fn make_thumb(size: usize) -> egui::ColorImage {
        let pixel_count = size / 4;
        egui::ColorImage {
            size: [pixel_count, 1],
            pixels: vec![egui::Color32::BLACK; pixel_count],
        }
    }

    fn insert(cache: &mut HashMap<usize, egui::ColorImage>, bytes: &mut usize, idx: usize, size: usize) {
        let img = make_thumb(size);
        *bytes += img.pixels.len() * 4;
        cache.insert(idx, img);
    }

    fn evict(cache: &mut HashMap<usize, egui::ColorImage>, bytes: &mut usize, current_idx: usize) {
        evict_thumb_cache_with_budget(cache, bytes, current_idx, TEST_BUDGET);
    }

    #[test]
    fn evicts_furthest_entry() {
        let mut cache = HashMap::new();
        let mut bytes = 0;
        let each = TEST_BUDGET / 2 + 1;

        insert(&mut cache, &mut bytes, 0, each);
        insert(&mut cache, &mut bytes, 50, each);
        insert(&mut cache, &mut bytes, 45, each);

        evict(&mut cache, &mut bytes, 45);

        assert!(!cache.contains_key(&0), "furthest entry (0) should be evicted");
        assert!(cache.contains_key(&45), "current position should remain");
    }

    #[test]
    fn evicts_multiple_until_under_budget() {
        let mut cache = HashMap::new();
        let mut bytes = 0;
        let chunk = TEST_BUDGET / 3 + 1;

        insert(&mut cache, &mut bytes, 0, chunk);
        insert(&mut cache, &mut bytes, 100, chunk);
        insert(&mut cache, &mut bytes, 50, chunk);
        insert(&mut cache, &mut bytes, 200, chunk);

        evict(&mut cache, &mut bytes, 50);

        assert!(bytes <= TEST_BUDGET);
        assert!(cache.contains_key(&50), "current position should remain");
        assert!(!cache.contains_key(&200), "furthest entry (200) should be evicted first");
    }

    #[test]
    fn no_eviction_under_budget() {
        let mut cache = HashMap::new();
        let mut bytes = 0;
        let small = TEST_BUDGET / 10;

        insert(&mut cache, &mut bytes, 5, small);
        insert(&mut cache, &mut bytes, 10, small);

        evict(&mut cache, &mut bytes, 5);

        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn keeps_at_least_one_entry() {
        let mut cache = HashMap::new();
        let mut bytes = 0;

        insert(&mut cache, &mut bytes, 42, TEST_BUDGET + 1000);

        evict(&mut cache, &mut bytes, 42);

        assert_eq!(cache.len(), 1, "should never evict the last entry");
    }

    #[test]
    fn set_budget_evicts_existing_entries() {
        let ctx = egui::Context::default();
        let mut tc = ThumbnailCache::new(&ctx, 0);
        let mb = 1024 * 1024;

        insert(&mut tc.cache, &mut tc.cache_bytes, 0, mb);
        insert(&mut tc.cache, &mut tc.cache_bytes, 10, mb);
        insert(&mut tc.cache, &mut tc.cache_bytes, 100, mb);
        tc.texture_idx = Some(10);

        tc.set_budget_mb(2);

        assert!(tc.cache_bytes <= 2 * mb);
        assert!(tc.cache.contains_key(&10), "displayed entry should remain");
        assert!(!tc.cache.contains_key(&100), "furthest entry should be evicted");
    }

    #[test]
    fn bytes_tracking_stays_consistent() {
        let mut cache = HashMap::new();
        let mut bytes = 0;
        let chunk = TEST_BUDGET / 2 + 1;

        insert(&mut cache, &mut bytes, 0, chunk);
        insert(&mut cache, &mut bytes, 50, chunk);
        insert(&mut cache, &mut bytes, 100, chunk);

        evict(&mut cache, &mut bytes, 50);

        let actual: usize = cache.values().map(|img| img.pixels.len() * 4).sum();
        assert_eq!(bytes, actual);
    }

    // ---- index removal -------------------------------------------------
    //
    // A file leaves the list (moved to the trash). Every structure keyed by
    // file index must keep pointing at the same files afterwards.

    #[test]
    fn shift_index_maps_around_the_removed_file() {
        assert_eq!(shift_index(0, 3), Some(0));
        assert_eq!(shift_index(2, 3), Some(2));
        assert_eq!(shift_index(3, 3), None);
        assert_eq!(shift_index(4, 3), Some(3));
        assert_eq!(shift_index(100, 3), Some(99));
        assert_eq!(shift_index(0, 0), None);
        assert_eq!(shift_index(1, 0), Some(0));
    }

    fn one_pixel() -> egui::ColorImage {
        egui::ColorImage::new([1, 1], egui::Color32::WHITE)
    }

    fn fake_paths(n: usize) -> Vec<PathBuf> {
        (0..n).map(|i| PathBuf::from(format!("/nonexistent/f{i}.png"))).collect()
    }

    /// Texture named after the original file index, so the mapping can be
    /// checked by name after a removal.
    fn tex(ctx: &egui::Context, original_index: usize) -> egui::TextureHandle {
        ctx.load_texture(format!("f{original_index}"), one_pixel(), egui::TextureOptions::LINEAR)
    }

    /// Window over files [first, first + 2 * cache_count + 1), every slot
    /// loaded with a texture named after its original file index.
    fn window(ctx: &egui::Context, cache_count: usize, first: usize) -> SlidingWindowCache {
        let mut c = SlidingWindowCache::new(ctx, cache_count, 1);
        c.first_file_index = first;
        for (k, slot) in c.slots.iter_mut().enumerate() {
            *slot = Some(tex(ctx, first + k));
        }
        c
    }

    /// Every loaded slot must hold the texture of the file that now has
    /// that index: original index `i` for `i < removed`, `i + 1` above.
    fn assert_slots_consistent(c: &SlidingWindowCache, removed: usize) {
        for (k, slot) in c.slots.iter().enumerate() {
            let new_index = c.first_file_index + k;
            let original = if new_index >= removed { new_index + 1 } else { new_index };
            if let Some(t) = slot {
                assert_eq!(
                    t.name(),
                    format!("f{original}"),
                    "slot {k} (file {new_index}) holds the wrong texture"
                );
            }
        }
    }

    #[test]
    fn remove_before_window_shifts_first_only() {
        let ctx = egui::Context::default();
        let paths = fake_paths(20);
        let mut c = window(&ctx, 2, 10); // files 10..14
        let before: Vec<_> = c.slots.iter().map(|s| s.as_ref().unwrap().name()).collect();

        let mut after = paths.clone();
        after.remove(3);
        c.remove_index(3, &after);

        assert_eq!(c.first_file_index, 9);
        let now: Vec<_> = c.slots.iter().map(|s| s.as_ref().unwrap().name()).collect();
        assert_eq!(now, before, "slot contents must not change");
        assert_slots_consistent(&c, 3);
        assert!(c.in_flight.is_empty(), "nothing to load when the window is untouched");
    }

    #[test]
    fn remove_after_window_changes_nothing() {
        let ctx = egui::Context::default();
        let paths = fake_paths(20);
        let mut c = window(&ctx, 2, 3); // files 3..7
        let before: Vec<_> = c.slots.iter().map(|s| s.as_ref().unwrap().name()).collect();

        let mut after = paths.clone();
        after.remove(15);
        c.remove_index(15, &after);

        assert_eq!(c.first_file_index, 3);
        let now: Vec<_> = c.slots.iter().map(|s| s.as_ref().unwrap().name()).collect();
        assert_eq!(now, before);
        assert!(c.in_flight.is_empty());
    }

    #[test]
    fn remove_center_fills_from_the_right() {
        let ctx = egui::Context::default();
        let paths = fake_paths(20);
        let mut c = window(&ctx, 2, 5); // files 5..9, center 7

        let mut after = paths.clone();
        after.remove(7);
        c.remove_index(7, &after);

        assert_eq!(c.first_file_index, 5);
        assert_eq!(c.slots.len(), 5);
        assert_slots_consistent(&c, 7);
        // Slot 4 is now file 9 (originally f10) and is being loaded.
        assert!(c.slots[4].is_none());
        assert_eq!(c.in_flight.get(&after[9]), Some(&9));
        // The first four slots kept their textures: f5 f6 f8 f9.
        let names: Vec<_> = c.slots.iter().take(4).map(|s| s.as_ref().unwrap().name()).collect();
        assert_eq!(names, ["f5", "f6", "f8", "f9"]);
    }

    #[test]
    fn remove_first_slot_fills_from_the_right() {
        let ctx = egui::Context::default();
        let paths = fake_paths(20);
        let mut c = window(&ctx, 2, 5);

        let mut after = paths.clone();
        after.remove(5);
        c.remove_index(5, &after);

        assert_eq!(c.first_file_index, 5);
        assert_slots_consistent(&c, 5);
        let names: Vec<_> = c.slots.iter().take(4).map(|s| s.as_ref().unwrap().name()).collect();
        assert_eq!(names, ["f6", "f7", "f8", "f9"]);
        assert!(c.slots[4].is_none());
    }

    #[test]
    fn remove_at_end_of_list_fills_from_the_left() {
        let ctx = egui::Context::default();
        let paths = fake_paths(10);
        let mut c = window(&ctx, 2, 5); // files 5..9, the last five files

        let mut after = paths.clone();
        after.remove(9); // last file
        c.remove_index(9, &after);

        // No file 9 exists any more, so the window slides back to 4..8.
        assert_eq!(c.first_file_index, 4);
        assert_eq!(c.slots.len(), 5);
        assert!(c.slots[0].is_none(), "new leftmost slot is loading");
        assert_eq!(c.in_flight.get(&after[4]), Some(&4));
        let names: Vec<_> = c.slots.iter().skip(1).map(|s| s.as_ref().unwrap().name()).collect();
        assert_eq!(names, ["f5", "f6", "f7", "f8"]);
        assert_slots_consistent(&c, 9);
    }

    #[test]
    fn remove_when_list_is_shorter_than_window_leaves_empty_slot() {
        let ctx = egui::Context::default();
        let paths = fake_paths(3);
        let mut c = SlidingWindowCache::new(&ctx, 2, 1); // 5 slots, 3 files
        for k in 0..3 {
            c.slots[k] = Some(tex(&ctx, k));
        }

        let mut after = paths.clone();
        after.remove(1);
        c.remove_index(1, &after);

        assert_eq!(c.first_file_index, 0);
        assert_eq!(c.slots.len(), 5);
        let names: Vec<_> = c.slots.iter().map(|s| s.as_ref().map(|t| t.name())).collect();
        assert_eq!(names, [Some("f0".into()), Some("f2".into()), None, None, None]);
        assert!(c.in_flight.is_empty(), "nothing exists to load");
    }

    #[test]
    fn remove_only_file_leaves_no_bookkeeping() {
        let ctx = egui::Context::default();
        let mut c = SlidingWindowCache::new(&ctx, 2, 1);
        c.slots[0] = Some(tex(&ctx, 0));

        c.remove_index(0, &[]);

        assert_eq!(c.first_file_index, 0);
        assert!(c.slots.iter().all(|s| s.is_none()));
        assert!(c.in_flight.is_empty());
        assert!(c.pending_decodes.is_empty());
        assert!(c.pending_uploads.is_empty());
    }

    #[test]
    fn remove_renumbers_in_flight_and_queues() {
        let ctx = egui::Context::default();
        let paths = fake_paths(20);
        let mut c = window(&ctx, 2, 5); // files 5..9
        c.max_decode_threads = 4;
        c.slots[3] = None; // file 8 loading
        c.slots[4] = None; // file 9 queued
        c.in_flight.insert(paths[8].clone(), 8);
        c.pending_decodes.push_back((9, paths[9].clone()));
        c.pending_uploads.push_back((6, one_pixel(), "f6".into()));
        c.pending_uploads.push_back((7, one_pixel(), "f7".into()));

        let mut after = paths.clone();
        after.remove(7);
        c.remove_index(7, &after);

        // in_flight: file 8 is now 7; plus the new rightmost (9, was f10).
        assert_eq!(c.in_flight.get(&paths[8]), Some(&7));
        assert_eq!(c.in_flight.get(&paths[10]), Some(&9));
        assert_eq!(c.in_flight.len(), 2);
        // queued decode for file 9 is now 8
        assert_eq!(c.pending_decodes.len(), 1);
        assert_eq!(c.pending_decodes[0].0, 8);
        // upload for the removed file is dropped, the one for 6 stays
        let uploads: Vec<_> = c.pending_uploads.iter().map(|(i, _, _)| *i).collect();
        assert_eq!(uploads, [6]);
    }

    #[test]
    fn stale_decode_result_is_dropped_by_poll() {
        let ctx = egui::Context::default();
        let paths = fake_paths(20);
        let mut c = window(&ctx, 2, 5);
        c.slots[3] = None;
        c.in_flight.insert(paths[8].clone(), 8);

        let mut after = paths.clone();
        after.remove(8); // the file being decoded is the one removed
        c.remove_index(8, &after);
        assert!(c.in_flight.get(&paths[8]).is_none());

        // The thread finishes and reports the old path.
        c.tx.send(DecodeResult { path: paths[8].clone(), image: Some(one_pixel()), decode_ms: 0.0 })
            .unwrap();
        c.poll(&after);

        assert!(c.pending_uploads.is_empty(), "stale result must not be uploaded");
        // Slot 3 is now file 8 (originally f9), which had a texture.
        assert_eq!(c.slots[3].as_ref().unwrap().name(), "f9");
    }

    #[test]
    fn renumbered_decode_result_lands_in_the_right_slot() {
        let ctx = egui::Context::default();
        let paths = fake_paths(20);
        let mut c = window(&ctx, 2, 5);
        c.slots[4] = None; // file 9 loading
        c.in_flight.insert(paths[9].clone(), 9);

        let mut after = paths.clone();
        after.remove(6);
        c.remove_index(6, &after); // file 9 is now file 8, slot 3

        c.tx.send(DecodeResult { path: paths[9].clone(), image: Some(one_pixel()), decode_ms: 0.0 })
            .unwrap();
        c.poll(&after);

        assert_eq!(c.pending_uploads.len(), 0, "uploaded within the frame");
        assert!(c.slots[3].is_some(), "result went to the renumbered slot");
        assert_eq!(c.current_texture_for(8).unwrap().name(), "f9.png");
    }

    #[test]
    fn lru_remove_index_shifts_keys_and_keeps_order() {
        let ctx = egui::Context::default();
        let mut lru = DecodeLruCache::new(&ctx, 1024);
        for i in [2usize, 5, 7, 9] {
            let _ = lru.insert(i, format!("f{i}"), one_pixel());
        }
        let bytes_before = lru.total_bytes;

        lru.remove_index(5);

        assert_eq!(lru.len(), 3);
        assert_eq!(lru.total_bytes, bytes_before - 4);
        assert_eq!(lru.entries[&2].name(), "f2");
        assert_eq!(lru.entries[&6].name(), "f7");
        assert_eq!(lru.entries[&8].name(), "f9");
        assert!(!lru.entries.contains_key(&5));
        assert_eq!(lru.order, [2, 6, 8]);

        // A removal outside the cached keys still renumbers those above.
        lru.remove_index(0);
        assert_eq!(lru.order, [1, 5, 7]);
        assert_eq!(lru.entries[&7].name(), "f9");
        assert_eq!(lru.total_bytes, bytes_before - 4);
    }

    #[test]
    fn thumbnail_remove_index_shifts_keys_and_displayed_index() {
        let ctx = egui::Context::default();
        let mut tc = ThumbnailCache::new(&ctx, 0);
        insert(&mut tc.cache, &mut tc.cache_bytes, 3, 64);
        insert(&mut tc.cache, &mut tc.cache_bytes, 4, 64);
        insert(&mut tc.cache, &mut tc.cache_bytes, 9, 64);
        tc.texture_idx = Some(9);
        tc.pending_idx = Some(6);

        tc.remove_index(4);

        let mut keys: Vec<_> = tc.cache.keys().copied().collect();
        keys.sort();
        assert_eq!(keys, [3, 8]);
        assert_eq!(tc.cache_bytes, 128);
        assert_eq!(tc.texture_idx, Some(8));
        assert_eq!(tc.pending_idx, None);

        // Removing the displayed thumbnail clears the displayed index.
        tc.remove_index(8);
        assert_eq!(tc.texture_idx, None);
        let keys: Vec<_> = tc.cache.keys().copied().collect();
        assert_eq!(keys, [3]);
    }

    #[test]
    fn thumbnail_poll_drops_result_whose_path_moved() {
        let ctx = egui::Context::default();
        let mut tc = ThumbnailCache::new(&ctx, 0);
        let paths = fake_paths(5);
        let (res_tx, res_rx) = mpsc::channel();
        tc.res_rx = res_rx;
        tc.pending_idx = Some(3);

        // The worker finished index 3 for the old list; file 1 was removed
        // meanwhile so that path now sits at index 2.
        let mut after = paths.clone();
        after.remove(1);
        res_tx.send((3, paths[3].clone(), Some(one_pixel()))).unwrap();
        tc.poll(&after);

        assert!(tc.cache.is_empty(), "stale thumbnail must not be cached");
        assert_eq!(tc.pending_idx, None, "the pending marker still clears");

        // A result that still matches its index is accepted.
        res_tx.send((2, after[2].clone(), Some(one_pixel()))).unwrap();
        tc.poll(&after);
        assert!(tc.cache.contains_key(&2));
    }
}
