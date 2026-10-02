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

fn empty_record() -> Arc<MetadataRecord> {
    Arc::new(MetadataRecord::default())
}

/// Texture named after the original file index, so the mapping can be
/// checked by name after a removal.
fn tex(ctx: &egui::Context, original_index: usize) -> egui::TextureHandle {
    ctx.load_texture(format!("f{original_index}"), one_pixel(), egui::TextureOptions::LINEAR)
}

fn loaded(ctx: &egui::Context, original_index: usize) -> Decoded {
    Decoded::Image(Loaded { texture: tex(ctx, original_index), record: empty_record() })
}

/// Window over files [first, first + 2 * cache_count + 1), every slot
/// loaded with a texture named after its original file index.
fn window(ctx: &egui::Context, cache_count: usize, first: usize) -> SlidingWindowCache {
    let mut c = SlidingWindowCache::new(ctx, cache_count, 1);
    c.first_file_index = first;
    for (k, slot) in c.slots.iter_mut().enumerate() {
        *slot = Some(loaded(ctx, first + k));
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
                t.image().unwrap().texture.name(),
                format!("f{original}"),
                "slot {k} (file {new_index}) holds the wrong texture"
            );
        }
    }
}

/// Dual-pane case: the other pane, on the same folder, trashed a file
/// this pane does not have loaded. The loaded images are the same photos
/// with indices one lower, so only `first_file_index` moves and nothing
/// is decoded.
#[test]
fn remove_before_window_shifts_first_only() {
    let ctx = egui::Context::default();
    let paths = fake_paths(20);
    let mut c = window(&ctx, 2, 10); // files 10..14
    let before: Vec<_> = c.slots.iter().map(|s| s.as_ref().unwrap().image().unwrap().texture.name()).collect();

    let mut after = paths.clone();
    after.remove(3);
    c.remove_index(3, &after);

    assert_eq!(c.first_file_index, 9);
    let now: Vec<_> = c.slots.iter().map(|s| s.as_ref().unwrap().image().unwrap().texture.name()).collect();
    assert_eq!(now, before, "slot contents must not change");
    assert_slots_consistent(&c, 3);
    assert!(c.running_decodes.is_empty(), "nothing to load when the window is untouched");
}

/// Dual-pane case, other side: the other pane trashed a file past this
/// pane's loaded images. Nothing about this pane changes.
#[test]
fn remove_after_window_changes_nothing() {
    let ctx = egui::Context::default();
    let paths = fake_paths(20);
    let mut c = window(&ctx, 2, 3); // files 3..7
    let before: Vec<_> = c.slots.iter().map(|s| s.as_ref().unwrap().image().unwrap().texture.name()).collect();

    let mut after = paths.clone();
    after.remove(15);
    c.remove_index(15, &after);

    assert_eq!(c.first_file_index, 3);
    let now: Vec<_> = c.slots.iter().map(|s| s.as_ref().unwrap().image().unwrap().texture.name()).collect();
    assert_eq!(now, before);
    assert!(c.running_decodes.is_empty());
}

/// The typical culling case. Window over files 5..9 (all loaded), file 7
/// is trashed. Expected: slot for 7 dropped, the four other textures kept
/// (no re-decode), and one new load queued for the file that is now
/// index 9 (it used to be 10). `assert_slots_consistent` checks every
/// slot holds the texture of the file that now has that index.
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
    assert_eq!(c.running_decodes.get(&after[9]), Some(&9));
    // The first four slots kept their textures: f5 f6 f8 f9.
    let names: Vec<_> = c.slots.iter().take(4).map(|s| s.as_ref().unwrap().image().unwrap().texture.name()).collect();
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
    let names: Vec<_> = c.slots.iter().take(4).map(|s| s.as_ref().unwrap().image().unwrap().texture.name()).collect();
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
    assert_eq!(c.running_decodes.get(&after[4]), Some(&4));
    let names: Vec<_> = c.slots.iter().skip(1).map(|s| s.as_ref().unwrap().image().unwrap().texture.name()).collect();
    assert_eq!(names, ["f5", "f6", "f7", "f8"]);
    assert_slots_consistent(&c, 9);
}

#[test]
fn remove_when_list_is_shorter_than_window_leaves_empty_slot() {
    let ctx = egui::Context::default();
    let paths = fake_paths(3);
    let mut c = SlidingWindowCache::new(&ctx, 2, 1); // 5 slots, 3 files
    for k in 0..3 {
        c.slots[k] = Some(loaded(&ctx, k));
    }

    let mut after = paths.clone();
    after.remove(1);
    c.remove_index(1, &after);

    assert_eq!(c.first_file_index, 0);
    assert_eq!(c.slots.len(), 5);
    let names: Vec<_> = c.slots.iter().map(|s| s.as_ref().map(|t| t.image().unwrap().texture.name())).collect();
    assert_eq!(names, [Some("f0".into()), Some("f2".into()), None, None, None]);
    assert!(c.running_decodes.is_empty(), "nothing exists to load");
}

#[test]
fn remove_only_file_leaves_no_bookkeeping() {
    let ctx = egui::Context::default();
    let mut c = SlidingWindowCache::new(&ctx, 2, 1);
    c.slots[0] = Some(loaded(&ctx, 0));

    c.remove_index(0, &[]);

    assert_eq!(c.first_file_index, 0);
    assert!(c.slots.iter().all(|s| s.is_none()));
    assert!(c.running_decodes.is_empty());
    assert!(c.pending_decodes.is_empty());
    assert!(c.pending_uploads.is_empty());
}

#[test]
fn remove_reindexes_running_decodes_and_queues() {
    let ctx = egui::Context::default();
    let paths = fake_paths(20);
    let mut c = window(&ctx, 2, 5); // files 5..9
    c.max_decode_threads = 4;
    c.slots[3] = None; // file 8 loading
    c.slots[4] = None; // file 9 queued
    c.running_decodes.insert(paths[8].clone(), 8);
    c.pending_decodes.push_back((9, paths[9].clone()));
    c.pending_uploads.push_back(PendingUpload {
        file_index: 6, image: one_pixel(), name: "f6".into(), record: empty_record(),
    });
    c.pending_uploads.push_back(PendingUpload {
        file_index: 7, image: one_pixel(), name: "f7".into(), record: empty_record(),
    });

    let mut after = paths.clone();
    after.remove(7);
    c.remove_index(7, &after);

    // running_decodes: file 8 is now 7; plus the new rightmost (9, was f10).
    assert_eq!(c.running_decodes.get(&paths[8]), Some(&7));
    assert_eq!(c.running_decodes.get(&paths[10]), Some(&9));
    assert_eq!(c.running_decodes.len(), 2);
    // queued decode for file 9 is now 8
    assert_eq!(c.pending_decodes.len(), 1);
    assert_eq!(c.pending_decodes[0].0, 8);
    // upload for the removed file is dropped, the one for 6 stays
    let uploads: Vec<_> = c.pending_uploads.iter().map(|u| u.file_index).collect();
    assert_eq!(uploads, [6]);
}

/// The race. A background thread was told to decode file 8. While it
/// works, file 8 is trashed, so a different file is now number 8 (the one
/// that used to be 9). When the thread reports back, its result must not
/// land in slot 8.
///
/// This works because threads report the path they decoded, not the
/// number, and `poll` looks the number up in `running_decodes`. The removal
/// took the trashed path out of `running_decodes`, so the late result finds no
/// entry and is dropped. `reindexed_decode_result_lands_in_the_right_slot`
/// below is the other half: a thread decoding a file that survived the
/// removal lands in that file's new slot.
#[test]
fn stale_decode_result_is_dropped_by_poll() {
    let ctx = egui::Context::default();
    let paths = fake_paths(20);
    let mut c = window(&ctx, 2, 5);
    c.slots[3] = None;
    c.running_decodes.insert(paths[8].clone(), 8);

    let mut after = paths.clone();
    after.remove(8); // the file being decoded is the one removed
    c.remove_index(8, &after);
    assert!(!c.running_decodes.contains_key(&paths[8]));

    // The thread finishes and reports the old path.
    c.tx.send(DecodeResult { path: paths[8].clone(), image: Some(one_pixel()), decode_ms: 0.0, record: empty_record() })
        .unwrap();
    c.poll(&after);

    assert!(c.pending_uploads.is_empty(), "stale result must not be uploaded");
    // Slot 3 is now file 8 (originally f9), which had a texture.
    assert_eq!(c.slots[3].as_ref().unwrap().image().unwrap().texture.name(), "f9");
}

#[test]
fn reindexed_decode_result_lands_in_the_right_slot() {
    let ctx = egui::Context::default();
    let paths = fake_paths(20);
    let mut c = window(&ctx, 2, 5);
    c.slots[4] = None; // file 9 loading
    c.running_decodes.insert(paths[9].clone(), 9);

    let mut after = paths.clone();
    after.remove(6);
    c.remove_index(6, &after); // file 9 is now file 8, slot 3

    c.tx.send(DecodeResult { path: paths[9].clone(), image: Some(one_pixel()), decode_ms: 0.0, record: empty_record() })
        .unwrap();
    c.poll(&after);

    assert_eq!(c.pending_uploads.len(), 0, "uploaded within the frame");
    assert!(c.slots[3].is_some(), "result went to the reindexed slot");
    assert_eq!(c.loaded_for(8).unwrap().texture.name(), "f9.png");
}

#[test]
fn lru_remove_index_shifts_keys_and_keeps_order() {
    let ctx = egui::Context::default();
    let mut lru = DecodeLruCache::new(&ctx, 1024);
    for i in [2usize, 5, 7, 9] {
        let _ = lru.insert(i, format!("f{i}"), one_pixel(), empty_record());
    }
    let bytes_before = lru.total_bytes;

    lru.remove_index(5);

    assert_eq!(lru.len(), 3);
    assert_eq!(lru.total_bytes, bytes_before - 4);
    assert_eq!(lru.entries[&2].texture.name(), "f2");
    assert_eq!(lru.entries[&6].texture.name(), "f7");
    assert_eq!(lru.entries[&8].texture.name(), "f9");
    assert!(!lru.entries.contains_key(&5));
    assert_eq!(lru.order, [2, 6, 8]);

    // A removal outside the cached keys still reindexes those above.
    lru.remove_index(0);
    assert_eq!(lru.order, [1, 5, 7]);
    assert_eq!(lru.entries[&7].texture.name(), "f9");
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
