use std::collections::VecDeque;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

const WINDOW_DURATION: Duration = Duration::from_secs(2);
const MEMORY_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Tracks image rendering performance — how many unique images are
/// displayed per second, not UI event loop frame rate.
/// Also tracks process memory usage (RSS).
pub(crate) struct ImagePerfTracker {
    image_timestamps: VecDeque<Instant>,
    sys: System,
    pid: Pid,
    last_memory_check: Instant,
    memory_bytes: u64,
}

impl ImagePerfTracker {
    pub(crate) fn new() -> Self {
        let pid = sysinfo::get_current_pid().expect("failed to get current PID");
        let mut sys = System::new();
        let refresh_kind = ProcessRefreshKind::nothing().with_memory();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            false,
            refresh_kind,
        );
        let memory_bytes = sys.process(pid).map_or(0, |p| p.memory());
        Self {
            image_timestamps: VecDeque::new(),
            sys,
            pid,
            last_memory_check: Instant::now(),
            memory_bytes,
        }
    }

    /// Record that a new image was displayed.
    pub(crate) fn record_image_load(&mut self) {
        self.image_timestamps.push_back(Instant::now());
    }

    /// Calculate image rendering FPS from upload timestamps.
    /// Uses a 2-second rolling window, matching viewskater's ImageDisplayTracker.
    fn image_fps(&mut self) -> f64 {
        let now = Instant::now();
        let cutoff = now - WINDOW_DURATION;
        while let Some(front) = self.image_timestamps.front() {
            if *front < cutoff {
                self.image_timestamps.pop_front();
            } else {
                break;
            }
        }

        if self.image_timestamps.len() < 2 {
            return 0.0;
        }
        // Use (now - oldest) rather than (newest - oldest) so the denominator
        // keeps growing after navigation stops, producing smooth decay instead
        // of spiking when only a tight cluster of final timestamps remains.
        let oldest = *self.image_timestamps.front().unwrap();
        let span = now.duration_since(oldest).as_secs_f64();
        if span > 0.0 {
            (self.image_timestamps.len() - 1) as f64 / span
        } else {
            0.0
        }
    }

    /// Query process RSS, throttled to once per second.
    fn poll_memory(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_memory_check) >= MEMORY_POLL_INTERVAL {
            let refresh_kind = ProcessRefreshKind::nothing().with_memory();
            self.sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[self.pid]),
                false,
                refresh_kind,
            );
            if let Some(proc) = self.sys.process(self.pid) {
                self.memory_bytes = proc.memory();
            }
            self.last_memory_check = now;
        }
    }

    /// Format memory bytes as a human-readable string.
    fn memory_text(&self) -> String {
        let mb = self.memory_bytes as f64 / (1024.0 * 1024.0);
        if mb >= 1024.0 {
            format!("{:.1} GB", mb / 1024.0)
        } else {
            format!("{:.0} MB", mb)
        }
    }

    /// Build the FPS + memory display string.
    /// `cache_mb` is an optional (lru_mb, sliding_window_mb) breakdown.
    pub(crate) fn fps_text(&mut self, cache_mb: Option<(f64, f64)>) -> String {
        self.poll_memory();
        let mem = self.memory_text();
        if let Some((lru, sw)) = cache_mb {
            format!("Img: {:.1} FPS | {} (L:{:.0} C:{:.0})", self.image_fps(), mem, lru, sw)
        } else {
            format!("Img: {:.1} FPS | {}", self.image_fps(), mem)
        }
    }
}
