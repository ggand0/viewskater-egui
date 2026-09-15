//! In-app benchmarking harnesses, enabled via CLI flags.
//!
//! Each benchmark target gets its own submodule that drives the real GUI
//! (window, decode workers, GPU uploads, vsync pacing) with synthetic
//! input, then logs a report and closes the app so runs are scriptable
//! and comparable across branches:
//!
//! - [`preview`]: slider preview thumbnails (`--bench-preview`)
//! - [`nav`]: keyboard navigation (`--bench-nav`)
//! - [`slider`]: main slider navigation (`--bench-slider`)
//!
//! Shared measurement helpers live in this module; [`report`] holds the
//! JSON and markdown output.

use std::path::PathBuf;

pub(crate) mod nav;
pub(crate) mod phase;
pub(crate) mod preview;
pub(crate) mod report;
pub(crate) mod slider;

/// Phases that `--bench-skip` can leave out. Skate right stays because
/// skate left needs it to reach the far end; the tap phase is opt-in
/// through `--bench-tap-steps` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum SkipPhase {
    SkateLeft,
    Sweep,
    Scrub,
    Jump,
}

/// The `--bench-*` command line flags, flattened into the app's `Args`.
/// `BenchOptions` is the same information in the shape the app uses.
#[derive(clap::Args, Debug)]
pub(crate) struct BenchArgs {
    /// Run the slider preview benchmark on the given folder and exit.
    /// Simulates hovering the navigation slider and reports thumbnail
    /// latency stats to the log.
    #[arg(long)]
    pub bench_preview: bool,

    /// Run the keyboard navigation benchmark on the given folder and exit:
    /// skate to the end and back, then tap through at a human pace.
    /// Combines with --bench-preview (nav runs first).
    #[arg(long)]
    pub bench_nav: bool,

    /// Run the slider navigation benchmark on the given folder and exit:
    /// a sweep across the rail and back, scrubs around evenly spaced
    /// positions, and clicks spread over the folder. Combines with --bench-nav (nav runs first in each run).
    #[arg(long)]
    pub bench_slider: bool,

    /// Seconds the --bench-slider sweep takes to cross the rail.
    #[arg(long, default_value_t = 4.0, value_name = "SECS")]
    pub bench_sweep_secs: f64,

    /// Number of scrub gestures in --bench-slider (0 skips the phase).
    /// Anchors are spaced evenly from the first image to the last: 5
    /// means 0, 25, 50, 75 and 100 percent of the folder.
    #[arg(long, default_value_t = 5, value_name = "N")]
    pub bench_scrub_anchors: usize,

    /// Width of the region one scrub sweeps, as a share of the rail:
    /// 0.1 is 5 percent each side of the anchor.
    #[arg(long, default_value_t = 0.1, value_name = "SHARE")]
    pub bench_scrub_span: f32,

    /// Back-and-forth passes per scrub.
    #[arg(long, default_value_t = 2, value_name = "N")]
    pub bench_scrub_passes: usize,

    /// Seconds one scrub takes, press to release.
    #[arg(long, default_value_t = 2.0, value_name = "SECS")]
    pub bench_scrub_secs: f64,

    /// Number of click-to-jump gestures in --bench-slider (0 skips).
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub bench_jumps: usize,

    /// Phases to leave out, comma separated: skate-left, sweep, scrub,
    /// jump. All phases run by default.
    #[arg(long, value_enum, value_delimiter = ',', value_name = "PHASE,...")]
    pub bench_skip: Vec<SkipPhase>,

    /// Folder to benchmark. Repeat the flag to run several folders in one
    /// go; the positional path is then ignored. A summary averaged over
    /// all runs is written at the end.
    #[arg(long, value_name = "DIR")]
    pub bench_dir: Vec<PathBuf>,

    /// Only skate through the first N images of the folder (and back).
    /// Default is the whole folder.
    #[arg(long, value_name = "N")]
    pub bench_max_images: Option<usize>,

    /// Steps per second in the tap phase of --bench-nav.
    #[arg(long, default_value_t = 6.0, value_name = "PER_SEC")]
    pub bench_tap_rate: f64,

    /// Add a tap phase to --bench-nav: N single steps at --bench-tap-rate,
    /// measuring press-to-image latency. Off by default; useful for slow
    /// sources (RAW, JPEG 2000, network shares) where a decode can take
    /// longer than the gap between taps.
    #[arg(long, default_value_t = nav::DEFAULT_TAP_STEPS, value_name = "N")]
    pub bench_tap_steps: usize,

    /// Repeat the benchmarks this many times in one process, reopening the
    /// folder between runs.
    #[arg(long, default_value_t = 1, value_name = "N")]
    pub bench_runs: usize,

    /// Write a JSON and a markdown report per run into this directory.
    #[arg(long, value_name = "DIR")]
    pub bench_out: Option<PathBuf>,

    /// Free text copied into the report header to tell runs apart later,
    /// e.g. "before-exif" or "files-not-in-os-cache".
    #[arg(long, value_name = "TEXT")]
    pub bench_label: Option<String>,
}

impl From<BenchArgs> for BenchOptions {
    fn from(a: BenchArgs) -> Self {
        Self {
            nav: a.bench_nav,
            slider: a.bench_slider,
            preview: a.bench_preview,
            skip: a.bench_skip,
            dirs: a.bench_dir,
            max_images: a.bench_max_images,
            tap_rate: a.bench_tap_rate,
            tap_steps: a.bench_tap_steps,
            sweep_secs: a.bench_sweep_secs,
            scrub: slider::ScrubParams {
                anchors: a.bench_scrub_anchors,
                span: a.bench_scrub_span,
                passes: a.bench_scrub_passes,
                secs: a.bench_scrub_secs,
            },
            jumps: a.bench_jumps,
            runs: a.bench_runs.max(1),
            out_dir: a.bench_out,
            label: a.bench_label,
        }
    }
}

/// Which benchmarks to run and how. Modes combine: nav runs first, then
/// slider, then preview, on the same folder.
#[derive(Clone, Debug)]
pub(crate) struct BenchOptions {
    pub nav: bool,
    pub slider: bool,
    pub preview: bool,
    pub skip: Vec<SkipPhase>,
    /// Folders to benchmark in order (`--bench-dir`, repeatable). Empty
    /// means the folder given as the positional path.
    pub dirs: Vec<PathBuf>,
    /// Cap on images per skate pass of `--bench-nav`; None is the whole folder.
    pub max_images: Option<usize>,
    /// Steps per second in the tap phase of `--bench-nav`.
    pub tap_rate: f64,
    /// Steps in the tap phase.
    pub tap_steps: usize,
    /// `--bench-slider`: seconds for the sweep across the rail, the scrub
    /// gesture, number of jumps.
    pub sweep_secs: f64,
    pub scrub: slider::ScrubParams,
    pub jumps: usize,
    /// Repeat the whole sequence this many times in one process, reopening
    /// the folder between runs.
    pub runs: usize,
    /// Directory for the JSON and markdown reports. Log only when None.
    pub out_dir: Option<PathBuf>,
    /// Free text copied into the report header to tell runs apart later,
    /// e.g. "before-exif" or "files-not-in-os-cache".
    pub label: Option<String>,
}

impl BenchOptions {
    pub fn any(&self) -> bool {
        self.nav || self.slider || self.preview
    }

    pub fn skips(&self, phase: SkipPhase) -> bool {
        self.skip.contains(&phase)
    }
}

/// Visit order that jumps across the file list instead of walking to
/// neighbours: front, back, front + 1, back - 1, ... Used by the preview,
/// scrub and jump phases so consecutive targets are never cached.
pub(crate) fn scrambled_order(n: usize) -> Vec<usize> {
    (0..n)
        .map(|i| if i.is_multiple_of(2) { i / 2 } else { n - 1 - i / 2 })
        .collect()
}

/// Process CPU time (user + system) in seconds. Latency metrics can't see
/// wasted background work; this can. Unix only, None elsewhere.
#[cfg(unix)]
pub(crate) fn process_cpu_secs() -> Option<f64> {
    let mut ru = std::mem::MaybeUninit::<libc::rusage>::uninit();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, ru.as_mut_ptr()) } != 0 {
        return None;
    }
    let ru = unsafe { ru.assume_init() };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    Some(tv(ru.ru_utime) + tv(ru.ru_stime))
}

#[cfg(not(unix))]
pub(crate) fn process_cpu_secs() -> Option<f64> {
    None
}

/// Summary of a set of latency samples in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub(crate) struct LatencyStats {
    pub count: usize,
    pub avg_ms: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl LatencyStats {
    pub fn from_ms(samples: &[f64]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.total_cmp(b));
        if sorted.is_empty() {
            return Self::default();
        }
        Self {
            count: sorted.len(),
            avg_ms: sorted.iter().sum::<f64>() / sorted.len() as f64,
            median_ms: percentile(&sorted, 50.0),
            p95_ms: percentile(&sorted, 95.0),
            p99_ms: percentile(&sorted, 99.0),
            max_ms: *sorted.last().unwrap(),
        }
    }
}

/// Nearest-rank percentile of an ascending-sorted, non-empty slice.
/// `percentile(&s, 50.0)` is the median; `percentile(&s, 100.0)` the max.
fn percentile(sorted: &[f64], pct: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    let rank = (pct / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// `count / duration` guarding against zero/absent durations.
pub(crate) fn per_second(count: usize, duration: Option<std::time::Duration>) -> f64 {
    let secs = duration.map_or(0.0, |d| d.as_secs_f64()).max(f64::EPSILON);
    count as f64 / secs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_of_empty_are_zero() {
        let s = LatencyStats::from_ms(&[]);
        assert_eq!(s, LatencyStats::default());
    }

    #[test]
    fn stats_of_one_sample_are_that_sample() {
        let s = LatencyStats::from_ms(&[7.5]);
        assert_eq!(s.count, 1);
        assert_eq!((s.avg_ms, s.median_ms, s.p95_ms, s.p99_ms, s.max_ms), (7.5, 7.5, 7.5, 7.5, 7.5));
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        // 1..=100 in scrambled order: p50 = 50, p95 = 95, p99 = 99, max = 100.
        let mut v: Vec<f64> = (1..=100).map(|i| i as f64).collect();
        v.reverse();
        v.swap(3, 60);
        let s = LatencyStats::from_ms(&v);
        assert_eq!(s.count, 100);
        assert_eq!(s.median_ms, 50.0);
        assert_eq!(s.p95_ms, 95.0);
        assert_eq!(s.p99_ms, 99.0);
        assert_eq!(s.max_ms, 100.0);
        assert!((s.avg_ms - 50.5).abs() < 1e-9);
    }

    #[test]
    fn percentiles_of_small_sets_round_up_to_a_real_sample() {
        // Four samples: p50 is the 2nd, p95 and p99 the 4th. Never interpolated.
        let s = LatencyStats::from_ms(&[10.0, 20.0, 30.0, 40.0]);
        assert_eq!(s.median_ms, 20.0);
        assert_eq!(s.p95_ms, 40.0);
        assert_eq!(s.p99_ms, 40.0);
    }
}
