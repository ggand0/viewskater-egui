//! Per-phase bookkeeping shared by the navigation benchmarks: wall time,
//! frame times, process CPU time, peak memory, background decode times
//! and the timed-out flag. Each benchmark adds its own counts on top.

use std::time::Instant;

use super::{process_cpu_secs, LatencyStats};

/// Frame-to-frame time within a phase.
#[derive(Default)]
pub(crate) struct FrameTimer {
    last: Option<Instant>,
    deltas_ms: Vec<f64>,
}

impl FrameTimer {
    pub fn tick(&mut self, now: Instant) {
        if let Some(last) = self.last {
            self.deltas_ms.push((now - last).as_secs_f64() * 1000.0);
        }
        self.last = Some(now);
    }

    pub fn stats(&self) -> LatencyStats {
        LatencyStats::from_ms(&self.deltas_ms)
    }
}

pub(crate) struct PhaseStats {
    started: Instant,
    ended: Option<Instant>,
    frames: FrameTimer,
    cpu_start: Option<f64>,
    pub cpu_secs: Option<f64>,
    peak_rss_bytes: u64,
    peak_gpu_bytes: u64,
    /// Background decode times in ms, handed over by the driver when the
    /// phase ends.
    pub decode_times_ms: Vec<f64>,
    pub timed_out: bool,
}

impl PhaseStats {
    pub fn start(now: Instant) -> Self {
        Self {
            started: now,
            ended: None,
            frames: FrameTimer::default(),
            cpu_start: process_cpu_secs(),
            cpu_secs: None,
            peak_rss_bytes: 0,
            peak_gpu_bytes: 0,
            decode_times_ms: Vec::new(),
            timed_out: false,
        }
    }

    pub fn tick_frame(&mut self, now: Instant) {
        self.frames.tick(now);
    }

    pub fn end(&mut self, now: Instant) {
        self.ended = Some(now);
        self.cpu_secs = match (self.cpu_start, process_cpu_secs()) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        };
    }

    pub fn wall_secs(&self) -> f64 {
        self.ended.map_or(0.0, |e| (e - self.started).as_secs_f64())
    }

    pub fn observe_memory(&mut self, rss_bytes: u64, gpu_bytes: u64) {
        self.peak_rss_bytes = self.peak_rss_bytes.max(rss_bytes);
        self.peak_gpu_bytes = self.peak_gpu_bytes.max(gpu_bytes);
    }

    pub fn frame_stats(&self) -> LatencyStats {
        self.frames.stats()
    }

    pub fn decode_stats(&self) -> LatencyStats {
        LatencyStats::from_ms(&self.decode_times_ms)
    }

    pub fn peak_rss_mb(&self) -> f64 {
        mb(self.peak_rss_bytes)
    }

    pub fn peak_gpu_mb(&self) -> f64 {
        mb(self.peak_gpu_bytes)
    }
}

pub(crate) fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
