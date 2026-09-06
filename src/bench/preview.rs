use std::time::{Duration, Instant};

use super::{per_second, process_cpu_secs, LatencyStats};

/// Drives a synthetic hover across the navigation slider to benchmark the
/// slider preview pipeline in the real GUI (real window, decode worker,
/// GPU uploads, vsync frame pacing). Enabled with `--bench-preview`.
///
/// Phase 1 (hover-and-pause): visits half the image indices in a scrambled
/// order, holding the hover until the exact thumbnail is displayed, then
/// dwelling a few frames like a user pausing. Records time-to-exact.
/// Phase 2 (hover-hop): same over the other half, but moves on one frame
/// after the thumbnail appears. This exposes decode-worker contention:
/// if the worker is still busy with redundant work from the previous
/// index, the next thumbnail is delayed by up to a full decode.
/// Phase 3 (sweep): moves the hover across the whole slider at one index
/// per frame for a few passes, recording how often the displayed
/// thumbnail was the exact one.
///
/// When finished the app logs a report (including process CPU time, where
/// wasted background decodes show up) and closes, so runs are scriptable
/// and comparable across branches.
pub(crate) struct PreviewBench {
    num_images: usize,
    hover: HoverRun,
    hop: HoverRun,
    sweep_frame: usize,
    sweep_exact_frames: usize,
    sweep_started: Option<Instant>,
    sweep_duration: Option<Duration>,
    sweep_overlay_fps: Option<f64>,
    phase: Phase,
    started: Instant,
    cpu_start_secs: Option<f64>,
    cpu_used_secs: Option<f64>,
}

enum Phase {
    Hover,
    Hop,
    Sweep,
    Done,
}

/// Returned by `tick` when a phase just finished, so the caller can sample
/// the overlay's Preview FPS counter at that moment.
pub(crate) enum BenchPhaseEnd {
    Hover,
    Hop,
    Sweep,
}

const SWEEP_PASSES: usize = 2;
const TARGET_TIMEOUT_SECS: f64 = 10.0;

/// One hover-style phase: visit targets, wait for the exact thumbnail,
/// dwell a configurable number of frames, move on.
struct HoverRun {
    label: &'static str,
    targets: Vec<usize>,
    pos: usize,
    dwell_frames: u32,
    dwell_left: u32,
    wait_started: Option<Instant>,
    latencies_ms: Vec<f64>,
    timeouts: usize,
    frames: usize,
    duration: Option<Duration>,
    overlay_fps: Option<f64>,
    started: Option<Instant>,
}

impl HoverRun {
    fn new(label: &'static str, targets: Vec<usize>, dwell_frames: u32) -> Self {
        Self {
            label,
            targets,
            pos: 0,
            dwell_frames,
            dwell_left: 0,
            wait_started: None,
            latencies_ms: Vec::new(),
            timeouts: 0,
            frames: 0,
            duration: None,
            overlay_fps: None,
            started: None,
        }
    }

    fn current_target(&self) -> usize {
        self.targets[self.pos]
    }

    /// Advance one frame. Returns true when the run just finished.
    fn tick(&mut self, cursor_index: Option<usize>, exact: bool) -> bool {
        let started = *self.started.get_or_insert_with(Instant::now);
        self.frames += 1;

        if self.dwell_left > 0 {
            self.dwell_left -= 1;
            if self.dwell_left == 0 {
                self.pos += 1;
                if self.pos >= self.targets.len() {
                    self.duration = Some(started.elapsed());
                    return true;
                }
            }
            return false;
        }

        let target = self.current_target();
        let waiting = *self.wait_started.get_or_insert_with(Instant::now);
        let hit = exact && cursor_index == Some(target);
        let timed_out = waiting.elapsed().as_secs_f64() > TARGET_TIMEOUT_SECS;
        if hit || timed_out {
            if hit {
                self.latencies_ms.push(waiting.elapsed().as_secs_f64() * 1000.0);
            } else {
                log::warn!("preview bench: {} target {target} timed out", self.label);
                self.timeouts += 1;
            }
            self.wait_started = None;
            self.dwell_left = self.dwell_frames;
        }
        false
    }

    fn report_lines(&self) -> String {
        let stats = LatencyStats::from_ms(&self.latencies_ms);
        let secs = self.duration.map_or(0.0, |d| d.as_secs_f64());
        format!(
            "{} (dwell {}): {} targets, time-to-exact avg={:.1}ms median={:.1}ms max={:.1}ms, timeouts={}\n\
             {} (dwell {}): {} frames in {:.2}s ({:.0} fps), {:.1} thumbs/s, overlay Preview FPS={:.1}",
            self.label,
            self.dwell_frames,
            self.latencies_ms.len(),
            stats.avg_ms,
            stats.median_ms,
            stats.max_ms,
            self.timeouts,
            self.label,
            self.dwell_frames,
            self.frames,
            secs,
            per_second(self.frames, self.duration),
            per_second(self.latencies_ms.len(), self.duration),
            self.overlay_fps.unwrap_or(0.0),
        )
    }
}

impl PreviewBench {
    /// `num_images` must be >= 2.
    pub fn new(num_images: usize) -> Self {
        // Visit front, back, front+1, back-1, ... so consecutive targets
        // jump across the file list instead of walking to neighbors. Split
        // between the two hover phases so hop targets are still uncached.
        let scrambled: Vec<usize> = (0..num_images)
            .map(|i| if i.is_multiple_of(2) { i / 2 } else { num_images - 1 - i / 2 })
            .collect();
        let (hover_targets, hop_targets) = scrambled.split_at(num_images / 2);
        Self {
            num_images,
            hover: HoverRun::new("hover-and-pause", hover_targets.to_vec(), 5),
            hop: HoverRun::new("hover-hop", hop_targets.to_vec(), 1),
            sweep_frame: 0,
            sweep_exact_frames: 0,
            sweep_started: None,
            sweep_duration: None,
            sweep_overlay_fps: None,
            phase: Phase::Hover,
            started: Instant::now(),
            cpu_start_secs: process_cpu_secs(),
            cpu_used_secs: None,
        }
    }

    fn t_for(&self, idx: usize) -> f32 {
        idx as f32 / (self.num_images - 1) as f32
    }

    /// Synthetic hover fraction (0..=1) along the slider for this frame,
    /// or None once the benchmark has finished.
    pub fn hover_t(&self) -> Option<f32> {
        match self.phase {
            Phase::Hover => Some(self.t_for(self.hover.current_target())),
            Phase::Hop => Some(self.t_for(self.hop.current_target())),
            Phase::Sweep => {
                let fpp = self.num_images;
                let pass = self.sweep_frame / fpp;
                let within = (self.sweep_frame % fpp) as f32 / (fpp - 1) as f32;
                Some(if pass.is_multiple_of(2) { within } else { 1.0 - within })
            }
            Phase::Done => None,
        }
    }

    /// Advance the state machine with this frame's preview result.
    /// Returns which phase (if any) just finished on this frame.
    pub fn tick(&mut self, cursor_index: Option<usize>, exact: bool) -> Option<BenchPhaseEnd> {
        match self.phase {
            Phase::Hover => {
                if self.hover.tick(cursor_index, exact) {
                    self.phase = Phase::Hop;
                    return Some(BenchPhaseEnd::Hover);
                }
                None
            }
            Phase::Hop => {
                if self.hop.tick(cursor_index, exact) {
                    self.sweep_started = Some(Instant::now());
                    self.phase = Phase::Sweep;
                    return Some(BenchPhaseEnd::Hop);
                }
                None
            }
            Phase::Sweep => {
                if exact {
                    self.sweep_exact_frames += 1;
                }
                self.sweep_frame += 1;
                if self.sweep_frame >= SWEEP_PASSES * self.num_images {
                    self.sweep_duration = self.sweep_started.map(|s| s.elapsed());
                    self.cpu_used_secs = match (self.cpu_start_secs, process_cpu_secs()) {
                        (Some(start), Some(end)) => Some(end - start),
                        _ => None,
                    };
                    self.phase = Phase::Done;
                    return Some(BenchPhaseEnd::Sweep);
                }
                None
            }
            Phase::Done => None,
        }
    }

    pub fn set_overlay_fps(&mut self, phase: &BenchPhaseEnd, fps: f64) {
        match phase {
            BenchPhaseEnd::Hover => self.hover.overlay_fps = Some(fps),
            BenchPhaseEnd::Hop => self.hop.overlay_fps = Some(fps),
            BenchPhaseEnd::Sweep => self.sweep_overlay_fps = Some(fps),
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    pub fn report(&self) -> String {
        let sweep_total = SWEEP_PASSES * self.num_images;
        let sweep_secs = self.sweep_duration.map_or(0.0, |d| d.as_secs_f64());
        let cpu = self
            .cpu_used_secs
            .map_or("n/a".to_string(), |s| format!("{:.2}s", s));
        format!(
            "preview bench report ({} images)\n\
             {}\n\
             {}\n\
             sweep: exact thumbnail on {}/{} frames ({:.0}%), {} frames in {:.2}s ({:.0} fps), overlay Preview FPS={:.1}\n\
             process CPU time: {} | total wall: {:.1}s",
            self.num_images,
            self.hover.report_lines(),
            self.hop.report_lines(),
            self.sweep_exact_frames,
            sweep_total,
            100.0 * self.sweep_exact_frames as f64 / sweep_total as f64,
            self.sweep_frame,
            sweep_secs,
            per_second(self.sweep_frame, self.sweep_duration),
            self.sweep_overlay_fps.unwrap_or(0.0),
            cpu,
            self.started.elapsed().as_secs_f64(),
        )
    }
}
