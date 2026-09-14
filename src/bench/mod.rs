//! In-app benchmarking harnesses, enabled via CLI flags.
//!
//! Each benchmark target gets its own submodule that drives the real GUI
//! (window, decode workers, GPU uploads, vsync pacing) with synthetic
//! input, then logs a report and closes the app so runs are scriptable
//! and comparable across branches:
//!
//! - [`preview`]: slider preview thumbnails (`--bench-preview`)
//! - [`nav`]: keyboard navigation (`--bench-nav`)
//! - planned: main slider navigation (`--bench-slider`)
//!
//! Shared measurement helpers live in this module; [`report`] holds the
//! JSON and markdown output.

pub(crate) mod nav;
pub(crate) mod preview;
pub(crate) mod report;

/// Which benchmarks to run and how, from the CLI. Modes combine: nav runs
/// first, then preview, on the same folder.
#[derive(Clone, Debug, Default)]
pub(crate) struct BenchOptions {
    pub nav: bool,
    pub preview: bool,
    /// Cap on images per skate pass of `--bench-nav`; None is the whole folder.
    pub max_images: Option<usize>,
    /// Steps per second in the tap phase of `--bench-nav`.
    pub tap_rate: f64,
    /// Steps in the tap phase.
    pub tap_steps: usize,
    /// Repeat the whole sequence this many times in one process, reopening
    /// the folder between runs.
    pub runs: usize,
    /// Directory for the JSON and markdown reports. Log only when None.
    pub out_dir: Option<std::path::PathBuf>,
    /// Free text copied into the report header, e.g. "cold" or "warm".
    pub label: Option<String>,
}

impl BenchOptions {
    pub fn any(&self) -> bool {
        self.nav || self.preview
    }
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
