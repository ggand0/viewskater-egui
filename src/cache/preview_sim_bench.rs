use super::*;

// ---- Slider preview simulation benchmark ----
//
// Not a correctness test: simulates per-frame slider hovering against a
// real ThumbnailCache (worker thread, decode, upload) with fixed 60 fps
// frame pacing, and reports how many decodes the worker performed and
// how quickly thumbnails became ready.
//
// Run with:
//   cargo test --profile opt-dev bench_preview_simulation -- --ignored --nocapture
//
// "legacy" mode clears the pending marker before every frame, which makes
// current_thumbnail_for re-send the request each frame — reproducing the
// pre-dedup behavior exactly for comparison.
//
// Set THUMB_BENCH_DIR=/path/to/images to benchmark against real images
// instead of generated synthetic 1080p PNGs.

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;

const FRAME: Duration = Duration::from_millis(16);
const BENCH_IMAGE_COUNT: usize = 25;

struct SimStats {
    decodes: usize,
    elapsed: Duration,
    frames_to_ready: Vec<u32>,
}

fn bench_image_paths() -> Vec<PathBuf> {
    if let Ok(dir) = std::env::var("THUMB_BENCH_DIR") {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("THUMB_BENCH_DIR not readable")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| crate::file_io::is_supported_image(p))
            .collect();
        paths.sort();
        paths.truncate(BENCH_IMAGE_COUNT);
        assert!(!paths.is_empty(), "no supported images in THUMB_BENCH_DIR");
        return paths;
    }

    let dir = std::env::temp_dir().join("viewskater-thumb-bench");
    std::fs::create_dir_all(&dir).unwrap();
    (0..BENCH_IMAGE_COUNT)
        .map(|i| {
            let path = dir.join(format!("bench_{i:03}.png"));
            if !path.exists() {
                let seed = i as u32;
                let img = image::RgbImage::from_fn(1920, 1080, |x, y| {
                    image::Rgb([
                        ((x * 7 + seed * 13) % 256) as u8,
                        ((y * 5 + seed * 31) % 256) as u8,
                        (((x ^ y) + seed * 3) % 256) as u8,
                    ])
                });
                img.save(&path).unwrap();
            }
            path
        })
        .collect()
}

/// Intercept the worker→UI result channel so each completed decode can
/// be counted, without touching production code.
fn attach_decode_counter(tc: &mut ThumbnailCache) -> Arc<AtomicUsize> {
    let (tx, rx) = mpsc::channel();
    let real_rx = std::mem::replace(&mut tc.res_rx, rx);
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    std::thread::spawn(move || {
        while let Ok(msg) = real_rx.recv() {
            c.fetch_add(1, AtomicOrdering::Relaxed);
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
    counter
}

/// One simulated UI frame: poll results, query the hovered thumbnail,
/// wait one frame interval.
fn sim_frame(tc: &mut ThumbnailCache, idx: usize, path: &Path, legacy: bool) {
    if legacy {
        tc.pending_idx = None; // defeat dedup → pre-fix per-frame resend
    }
    tc.poll();
    let _ = tc.current_thumbnail_for(idx, path);
    std::thread::sleep(FRAME);
}

/// Keep polling until the worker has been idle ~0.5 s, so trailing
/// (possibly redundant) decodes are counted.
fn settle(tc: &mut ThumbnailCache, counter: &AtomicUsize) {
    let mut last = counter.load(AtomicOrdering::Relaxed);
    let mut idle_frames = 0;
    while idle_frames < 30 {
        std::thread::sleep(FRAME);
        tc.poll();
        let now = counter.load(AtomicOrdering::Relaxed);
        if now == last {
            idle_frames += 1;
        } else {
            last = now;
            idle_frames = 0;
        }
    }
}

/// Hover each target until its thumbnail is cached, then dwell 5 frames
/// (a user pausing on the slider before moving on).
fn simulate_hover(paths: &[PathBuf], targets: &[usize], legacy: bool) -> SimStats {
    let ctx = egui::Context::default();
    let mut tc = ThumbnailCache::new(&ctx, 200);
    let counter = attach_decode_counter(&mut tc);
    let start = Instant::now();

    let mut frames_to_ready = Vec::new();
    for &idx in targets {
        let mut frames = 0u32;
        while !tc.cache.contains_key(&idx) {
            sim_frame(&mut tc, idx, &paths[idx], legacy);
            frames += 1;
            assert!(frames < 600, "thumbnail [{idx}] never became ready");
        }
        frames_to_ready.push(frames);
        for _ in 0..5 {
            sim_frame(&mut tc, idx, &paths[idx], legacy);
        }
    }
    let elapsed = start.elapsed();
    settle(&mut tc, &counter);

    SimStats {
        decodes: counter.load(AtomicOrdering::Relaxed),
        elapsed,
        frames_to_ready,
    }
}

/// Sweep the cursor across all indices, one index per frame.
fn simulate_sweep(paths: &[PathBuf], passes: usize, legacy: bool) -> (usize, usize) {
    let ctx = egui::Context::default();
    let mut tc = ThumbnailCache::new(&ctx, 200);
    let counter = attach_decode_counter(&mut tc);

    for _ in 0..passes {
        for idx in 0..paths.len() {
            sim_frame(&mut tc, idx, &paths[idx], legacy);
        }
    }
    settle(&mut tc, &counter);
    (counter.load(AtomicOrdering::Relaxed), tc.cache.len())
}

fn report_hover(label: &str, stats: &SimStats, unique: usize) {
    let avg_frames = stats.frames_to_ready.iter().sum::<u32>() as f64
        / stats.frames_to_ready.len().max(1) as f64;
    let max_frames = stats.frames_to_ready.iter().max().copied().unwrap_or(0);
    println!(
        "  {label:<8} decodes={:<4} redundant={:<4} elapsed={:.2}s thumbs/s={:.1} ready avg={:.1} max={} frames",
        stats.decodes,
        stats.decodes.saturating_sub(unique),
        stats.elapsed.as_secs_f64(),
        unique as f64 / stats.elapsed.as_secs_f64(),
        avg_frames,
        max_frames,
    );
}

#[test]
#[ignore = "benchmark; run with --profile opt-dev --ignored --nocapture"]
fn bench_preview_simulation() {
    let paths = bench_image_paths();
    let n = paths.len();
    // Visit indices in a deterministic scrambled order (front, back,
    // front+1, back-1, ...) so each hover jumps across the file list.
    let targets: Vec<usize> = (0..n)
        .map(|i| if i.is_multiple_of(2) { i / 2 } else { n - 1 - i / 2 })
        .collect();

    println!("\n=== hover-and-pause: {n} thumbnails, 16ms frames, 5-frame dwell ===");
    let legacy = simulate_hover(&paths, &targets, true);
    report_hover("legacy", &legacy, n);
    let fixed = simulate_hover(&paths, &targets, false);
    report_hover("fixed", &fixed, n);

    assert_eq!(
        fixed.decodes, n,
        "fixed mode should decode each hovered thumbnail exactly once"
    );
    assert!(
        fixed.decodes <= legacy.decodes,
        "dedup must not increase decode count"
    );

    println!("\n=== fast sweep: {n} indices x 2 passes, 1 index/frame ===");
    let (legacy_decodes, legacy_cached) = simulate_sweep(&paths, 2, true);
    println!("  legacy   decodes={legacy_decodes} cached_at_end={legacy_cached}");
    let (fixed_decodes, fixed_cached) = simulate_sweep(&paths, 2, false);
    println!("  fixed    decodes={fixed_decodes} cached_at_end={fixed_cached}");
    assert!(fixed_cached > 0, "sweep should cache some thumbnails");
}
