//! In-app benchmarking harnesses, enabled via CLI flags.
//!
//! Each benchmark target gets its own submodule that drives the real GUI
//! (window, decode workers, GPU uploads, vsync pacing) with synthetic
//! input, then logs a report and closes the app so runs are scriptable
//! and comparable across branches:
//!
//! - [`preview`]: slider preview thumbnails (`--bench-preview`)
//! - planned: keyboard navigation, main slider navigation
//!
//! Shared measurement helpers live in this module.

pub(crate) mod preview;

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
pub(crate) struct LatencyStats {
    pub avg_ms: f64,
    pub median_ms: f64,
    pub max_ms: f64,
}

impl LatencyStats {
    pub fn from_ms(samples: &[f64]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.total_cmp(b));
        if sorted.is_empty() {
            return Self { avg_ms: 0.0, median_ms: 0.0, max_ms: 0.0 };
        }
        Self {
            avg_ms: sorted.iter().sum::<f64>() / sorted.len() as f64,
            median_ms: sorted[sorted.len() / 2],
            max_ms: *sorted.last().unwrap(),
        }
    }
}

/// `count / duration` guarding against zero/absent durations.
pub(crate) fn per_second(count: usize, duration: Option<std::time::Duration>) -> f64 {
    let secs = duration.map_or(0.0, |d| d.as_secs_f64()).max(f64::EPSILON);
    count as f64 / secs
}
