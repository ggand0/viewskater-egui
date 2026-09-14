//! Keyboard navigation benchmark (`--bench-nav`).
//!
//! Drives the real navigation step (`App::step_navigation`) the way a held
//! key does, one attempt per frame, and measures what a person would feel:
//! images per second, frames where the next image was not ready (stalls),
//! frame time percentiles, and the background decode times underneath.
//!
//! Phases, per folder:
//!
//! 1. Settle: wait for the first image and for the sliding window to fill.
//! 2. Skate right: hold the key to the end of the folder. The first
//!    `cache_count` steps were prefetched during settle and are excluded
//!    from the rate.
//! 3. Skate left: back to the start. Revisits hit the decode LRU, so this
//!    is the cached path and is reported separately.
//! 4. Tap right: one step every `1 / tap_rate` seconds over the first
//!    `TAP_STEPS` images, a person culling. Measures the latency from the
//!    press to the image appearing.
//!
//! The state machine takes the frame's `Instant` from the caller so it can
//! be unit tested with a fake clock.

use std::time::{Duration, Instant};

use crate::app::handlers::NavOutcome;

use super::report::{NavReport, SettleReport, SkateReport, TapReport};
use super::{process_cpu_secs, LatencyStats};

/// Default steps in the tap phase (`--bench-tap-steps`).
pub(crate) const DEFAULT_TAP_STEPS: usize = 200;
/// A skate or tap phase with no progress for this long is marked timed out
/// and the run moves on, like the preview bench's target timeout.
const NO_PROGRESS_TIMEOUT: Duration = Duration::from_secs(10);
/// Settle gives the decode threads longer: a cold 4K folder over a share
/// can take a while to fill the first window.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Settle,
    SkateRight,
    SkateLeft,
    Tap,
    Done,
}

/// What the driver should do this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drive {
    /// Call `tick_settle` with the pane state.
    Settle,
    /// Call `step_navigation(dir)` and pass the outcome to `tick_nav`.
    Step(isize),
    /// Tap phase, not due yet: call `tick_idle`.
    Idle,
    Done,
}

/// Returned by the tick functions when a phase just ended so the driver
/// can hand over that phase's background decode samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PhaseEnd(pub Phase);

/// Frame-to-frame time within a phase.
#[derive(Default)]
struct FrameTimer {
    last: Option<Instant>,
    deltas_ms: Vec<f64>,
}

impl FrameTimer {
    fn tick(&mut self, now: Instant) {
        if let Some(last) = self.last {
            self.deltas_ms.push((now - last).as_secs_f64() * 1000.0);
        }
        self.last = Some(now);
    }
}

/// Bookkeeping shared by every measured phase.
struct PhaseStats {
    started: Instant,
    ended: Option<Instant>,
    frames: FrameTimer,
    cpu_start: Option<f64>,
    cpu_secs: Option<f64>,
    peak_rss_bytes: u64,
    peak_gpu_bytes: u64,
    decode_ms: Vec<f64>,
    timed_out: bool,
}

impl PhaseStats {
    fn start(now: Instant) -> Self {
        Self {
            started: now,
            ended: None,
            frames: FrameTimer::default(),
            cpu_start: process_cpu_secs(),
            cpu_secs: None,
            peak_rss_bytes: 0,
            peak_gpu_bytes: 0,
            decode_ms: Vec::new(),
            timed_out: false,
        }
    }

    fn end(&mut self, now: Instant) {
        self.ended = Some(now);
        self.cpu_secs = match (self.cpu_start, process_cpu_secs()) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        };
    }

    fn wall_secs(&self) -> f64 {
        self.ended.map_or(0.0, |e| (e - self.started).as_secs_f64())
    }
}

struct SkateRun {
    stats: PhaseStats,
    dir: isize,
    /// Advances excluded from the rate: prefetched before the phase began.
    skip: usize,
    /// Stop after this many advances even if the folder goes on
    /// (`--bench-max-images`). None walks to the end.
    max_advances: Option<usize>,
    advances: usize,
    /// When the `skip`-th advance happened; the rate is measured from here.
    rate_start: Option<Instant>,
    last_advance: Option<Instant>,
    /// Frames and stalls counted only after `rate_start`.
    frames_counted: usize,
    stall_frames: usize,
}

impl SkateRun {
    fn new(now: Instant, dir: isize, skip: usize, max_advances: Option<usize>) -> Self {
        Self {
            stats: PhaseStats::start(now),
            dir,
            skip,
            max_advances,
            advances: 0,
            rate_start: if skip == 0 { Some(now) } else { None },
            last_advance: Some(now),
            frames_counted: 0,
            stall_frames: 0,
        }
    }

    /// Returns true when the phase is over.
    fn tick(&mut self, now: Instant, outcome: NavOutcome) -> bool {
        self.stats.frames.tick(now);
        if self.rate_start.is_some() {
            self.frames_counted += 1;
            if outcome.blocked {
                self.stall_frames += 1;
            }
        }
        if outcome.advanced {
            self.advances += 1;
            self.last_advance = Some(now);
            if self.advances == self.skip {
                self.rate_start = Some(now);
            }
        }
        let capped = self.max_advances.is_some_and(|max| self.advances >= max);
        if outcome.at_end || capped {
            self.stats.end(now);
            return true;
        }
        if self
            .last_advance
            .is_some_and(|t| now.duration_since(t) > NO_PROGRESS_TIMEOUT)
        {
            log::warn!("nav bench: skate {} made no progress for {:?}, giving up", self.label(), NO_PROGRESS_TIMEOUT);
            self.stats.timed_out = true;
            self.stats.end(now);
            return true;
        }
        false
    }

    fn label(&self) -> &'static str {
        if self.dir > 0 { "right" } else { "left" }
    }

    fn report(&self) -> SkateReport {
        let counted = self.advances.saturating_sub(self.skip);
        let secs = match (self.rate_start, self.last_advance) {
            (Some(s), Some(e)) if counted > 0 => (e - s).as_secs_f64(),
            _ => 0.0,
        };
        SkateReport {
            direction: self.label().to_string(),
            images: counted,
            skipped_images: self.advances.min(self.skip),
            wall_secs: self.stats.wall_secs(),
            images_per_sec: if secs > 0.0 { counted as f64 / secs } else { 0.0 },
            frames: self.frames_counted,
            stall_frames: self.stall_frames,
            stall_share: if self.frames_counted > 0 {
                self.stall_frames as f64 / self.frames_counted as f64
            } else {
                0.0
            },
            frame_ms: LatencyStats::from_ms(&self.stats.frames.deltas_ms),
            decode_ms: LatencyStats::from_ms(&self.stats.decode_ms),
            cpu_secs: self.stats.cpu_secs,
            peak_rss_mb: mb(self.stats.peak_rss_bytes),
            peak_gpu_mb: mb(self.stats.peak_gpu_bytes),
            timed_out: self.stats.timed_out,
        }
    }
}

struct TapRun {
    stats: PhaseStats,
    interval: Duration,
    steps_wanted: usize,
    steps_done: usize,
    next_due: Instant,
    /// Set when a press was issued and the image has not appeared yet.
    pending_since: Option<Instant>,
    latencies_ms: Vec<f64>,
    frames: usize,
    stall_frames: usize,
}

impl TapRun {
    fn new(now: Instant, rate_per_sec: f64, steps_wanted: usize) -> Self {
        Self {
            stats: PhaseStats::start(now),
            interval: Duration::from_secs_f64(1.0 / rate_per_sec.max(0.1)),
            steps_wanted,
            steps_done: 0,
            next_due: now,
            pending_since: None,
            latencies_ms: Vec::new(),
            frames: 0,
            stall_frames: 0,
        }
    }

    fn wants_step(&self, now: Instant) -> bool {
        self.pending_since.is_some() || now >= self.next_due
    }

    /// Returns true when the phase is over.
    fn tick(&mut self, now: Instant, outcome: NavOutcome) -> bool {
        self.stats.frames.tick(now);
        self.frames += 1;
        let pressed = *self.pending_since.get_or_insert_with(|| {
            // A new press. Keep the cadence anchored to the schedule, not
            // to when the previous image happened to appear.
            self.next_due += self.interval;
            now
        });
        if outcome.at_end {
            self.stats.end(now);
            return true;
        }
        if outcome.advanced {
            self.latencies_ms.push((now - pressed).as_secs_f64() * 1000.0);
            self.steps_done += 1;
            self.pending_since = None;
            if self.steps_done >= self.steps_wanted {
                self.stats.end(now);
                return true;
            }
        } else if outcome.blocked {
            self.stall_frames += 1;
            if now.duration_since(pressed) > NO_PROGRESS_TIMEOUT {
                log::warn!("nav bench: tap step {} never completed, giving up", self.steps_done + 1);
                self.stats.timed_out = true;
                self.stats.end(now);
                return true;
            }
        }
        false
    }

    fn tick_idle(&mut self, now: Instant) {
        self.stats.frames.tick(now);
        self.frames += 1;
    }

    fn report(&self) -> TapReport {
        TapReport {
            rate_per_sec: 1.0 / self.interval.as_secs_f64(),
            steps: self.steps_done,
            wall_secs: self.stats.wall_secs(),
            step_latency_ms: LatencyStats::from_ms(&self.latencies_ms),
            frames: self.frames,
            stall_frames: self.stall_frames,
            frame_ms: LatencyStats::from_ms(&self.stats.frames.deltas_ms),
            decode_ms: LatencyStats::from_ms(&self.stats.decode_ms),
            cpu_secs: self.stats.cpu_secs,
            peak_rss_mb: mb(self.stats.peak_rss_bytes),
            peak_gpu_mb: mb(self.stats.peak_gpu_bytes),
            timed_out: self.stats.timed_out,
        }
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

pub(crate) struct NavBench {
    num_images: usize,
    cache_count: usize,
    /// Images to walk per skate pass; None means the whole folder.
    max_images: Option<usize>,
    tap_rate: f64,
    tap_steps: usize,
    run_start: Instant,
    phase: Phase,
    settle_first_image: Option<Duration>,
    settle_done: Option<Duration>,
    settle_timed_out: bool,
    skate_right: Option<SkateRun>,
    skate_left: Option<SkateRun>,
    tap: Option<TapRun>,
}

impl NavBench {
    /// `run_start` is when this run began: process start for the first run
    /// (so settle includes window creation and the first sync decode), the
    /// folder reopen for later runs. `max_images` caps each skate pass;
    /// `tap_steps` the tap phase. Both are clamped to the folder.
    pub fn new(
        num_images: usize,
        cache_count: usize,
        max_images: Option<usize>,
        tap_rate: f64,
        tap_steps: usize,
        run_start: Instant,
    ) -> Self {
        Self {
            num_images,
            cache_count,
            max_images: max_images.map(|m| m.min(num_images.saturating_sub(1))),
            tap_rate,
            tap_steps,
            run_start,
            phase: Phase::Settle,
            settle_first_image: None,
            settle_done: None,
            settle_timed_out: false,
            skate_right: None,
            skate_left: None,
            tap: None,
        }
    }

    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// What the driver should do this frame.
    pub fn drive(&self, now: Instant) -> Drive {
        match self.phase {
            Phase::Settle => Drive::Settle,
            Phase::SkateRight => Drive::Step(1),
            Phase::SkateLeft => Drive::Step(-1),
            Phase::Tap => {
                if self.tap.as_ref().is_some_and(|t| t.wants_step(now)) {
                    Drive::Step(1)
                } else {
                    Drive::Idle
                }
            }
            Phase::Done => Drive::Done,
        }
    }

    /// Settle phase: `has_texture` is whether the pane shows an image,
    /// `settled` whether its sliding window has nothing in flight.
    pub fn tick_settle(&mut self, now: Instant, has_texture: bool, settled: bool) -> Option<PhaseEnd> {
        debug_assert_eq!(self.phase, Phase::Settle);
        let since_start = now.duration_since(self.run_start);
        if has_texture && self.settle_first_image.is_none() {
            self.settle_first_image = Some(since_start);
        }
        let timed_out = since_start > SETTLE_TIMEOUT;
        if (has_texture && settled) || timed_out {
            if timed_out {
                log::warn!("nav bench: window did not settle within {:?}", SETTLE_TIMEOUT);
                self.settle_timed_out = true;
            } else {
                self.settle_done = Some(since_start);
            }
            self.phase = Phase::SkateRight;
            self.skate_right = Some(SkateRun::new(
                now,
                1,
                self.cache_count.min(self.num_images),
                self.max_images,
            ));
            return Some(PhaseEnd(Phase::Settle));
        }
        None
    }

    /// Skate and tap phases: the outcome of this frame's `step_navigation`.
    pub fn tick_nav(&mut self, now: Instant, outcome: NavOutcome) -> Option<PhaseEnd> {
        match self.phase {
            Phase::SkateRight => {
                let run = self.skate_right.as_mut().expect("skate right run");
                if run.tick(now, outcome) {
                    // Coming back, the first images are the ones just shown
                    // and still in the window; skip the same count. Walk
                    // back exactly as far as the right pass went.
                    let walked = run.advances;
                    self.phase = Phase::SkateLeft;
                    self.skate_left = Some(SkateRun::new(
                        now,
                        -1,
                        self.cache_count.min(self.num_images),
                        Some(walked),
                    ));
                    return Some(PhaseEnd(Phase::SkateRight));
                }
                None
            }
            Phase::SkateLeft => {
                let run = self.skate_left.as_mut().expect("skate left run");
                if run.tick(now, outcome) {
                    self.phase = Phase::Tap;
                    let steps = self.tap_steps.min(self.num_images.saturating_sub(1));
                    self.tap = Some(TapRun::new(now, self.tap_rate, steps));
                    return Some(PhaseEnd(Phase::SkateLeft));
                }
                None
            }
            Phase::Tap => {
                let run = self.tap.as_mut().expect("tap run");
                if run.tick(now, outcome) {
                    self.phase = Phase::Done;
                    return Some(PhaseEnd(Phase::Tap));
                }
                None
            }
            Phase::Settle | Phase::Done => None,
        }
    }

    /// Tap phase, frame with no press due.
    pub fn tick_idle(&mut self, now: Instant) {
        if let Some(run) = &mut self.tap {
            run.tick_idle(now);
        }
    }

    /// Memory readings for this frame, kept as per-phase peaks.
    pub fn observe_memory(&mut self, rss_bytes: u64, gpu_bytes: u64) {
        let stats = match self.phase {
            Phase::SkateRight => self.skate_right.as_mut().map(|r| &mut r.stats),
            Phase::SkateLeft => self.skate_left.as_mut().map(|r| &mut r.stats),
            Phase::Tap => self.tap.as_mut().map(|r| &mut r.stats),
            Phase::Settle | Phase::Done => None,
        };
        if let Some(s) = stats {
            s.peak_rss_bytes = s.peak_rss_bytes.max(rss_bytes);
            s.peak_gpu_bytes = s.peak_gpu_bytes.max(gpu_bytes);
        }
    }

    /// Background decode times collected during `phase`, handed over by
    /// the driver when that phase ended.
    pub fn set_decode_samples(&mut self, phase: Phase, samples: Vec<f64>) {
        let stats = match phase {
            Phase::SkateRight => self.skate_right.as_mut().map(|r| &mut r.stats),
            Phase::SkateLeft => self.skate_left.as_mut().map(|r| &mut r.stats),
            Phase::Tap => self.tap.as_mut().map(|r| &mut r.stats),
            Phase::Settle | Phase::Done => None,
        };
        if let Some(s) = stats {
            s.decode_ms = samples;
        }
    }

    pub fn report(&self) -> NavReport {
        NavReport {
            images: self.num_images,
            settle: SettleReport {
                first_image_ms: self.settle_first_image.map(|d| d.as_secs_f64() * 1000.0),
                settled_ms: self.settle_done.map(|d| d.as_secs_f64() * 1000.0),
                timed_out: self.settle_timed_out,
            },
            skate_right: self.skate_right.as_ref().map(SkateRun::report),
            skate_left: self.skate_left.as_ref().map(SkateRun::report),
            tap: self.tap.as_ref().map(TapRun::report),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADV: NavOutcome = NavOutcome { advanced: true, blocked: false, at_end: false };
    const STALL: NavOutcome = NavOutcome { advanced: false, blocked: true, at_end: false };
    const END: NavOutcome = NavOutcome { advanced: false, blocked: false, at_end: true };

    fn ms(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn settle_waits_for_texture_and_quiet_window() {
        let t0 = Instant::now();
        let mut b = NavBench::new(10, 2, None, 6.0, DEFAULT_TAP_STEPS, t0);
        assert_eq!(b.drive(t0), Drive::Settle);
        assert_eq!(b.tick_settle(ms(t0, 100), false, true), None);
        assert_eq!(b.tick_settle(ms(t0, 200), true, false), None);
        assert_eq!(b.tick_settle(ms(t0, 300), true, true), Some(PhaseEnd(Phase::Settle)));
        assert_eq!(b.phase(), Phase::SkateRight);
        let r = b.report();
        assert_eq!(r.settle.first_image_ms, Some(200.0));
        assert_eq!(r.settle.settled_ms, Some(300.0));
    }

    #[test]
    fn skate_rate_excludes_prefetched_images_and_counts_stalls() {
        let t0 = Instant::now();
        let mut b = NavBench::new(10, 2, None, 6.0, DEFAULT_TAP_STEPS, t0);
        b.tick_settle(t0, true, true);
        // Two prefetched advances at 10 ms spacing, then 4 counted advances
        // with one stall in between, then the end.
        let mut t = t0;
        for _ in 0..2 {
            t += Duration::from_millis(10);
            assert_eq!(b.tick_nav(t, ADV), None);
        }
        let rate_start = t;
        for outcome in [ADV, STALL, ADV, ADV, ADV] {
            t += Duration::from_millis(10);
            assert_eq!(b.tick_nav(t, outcome), None);
        }
        let last = t;
        t += Duration::from_millis(10);
        assert_eq!(b.tick_nav(t, END), Some(PhaseEnd(Phase::SkateRight)));
        assert_eq!(b.phase(), Phase::SkateLeft);

        let r = b.report().skate_right.unwrap();
        assert_eq!(r.skipped_images, 2);
        assert_eq!(r.images, 4);
        // 4 images over the 50 ms between the skip point and the last advance.
        let secs = (last - rate_start).as_secs_f64();
        assert!((r.images_per_sec - 4.0 / secs).abs() < 1e-6);
        // Frames counted after the skip point: 5 stepping frames + the end frame.
        assert_eq!(r.frames, 6);
        assert_eq!(r.stall_frames, 1);
        assert!(!r.timed_out);
    }

    #[test]
    fn skate_gives_up_after_no_progress() {
        let t0 = Instant::now();
        let mut b = NavBench::new(10, 0, None, 6.0, DEFAULT_TAP_STEPS, t0);
        b.tick_settle(t0, true, true);
        assert_eq!(b.tick_nav(ms(t0, 10), ADV), None);
        assert_eq!(b.tick_nav(ms(t0, 5_000), STALL), None);
        assert_eq!(b.tick_nav(ms(t0, 10_100), STALL), Some(PhaseEnd(Phase::SkateRight)));
        assert!(b.report().skate_right.unwrap().timed_out);
    }

    #[test]
    fn tap_presses_on_schedule_and_measures_latency() {
        let t0 = Instant::now();
        let mut b = NavBench::new(5, 0, None, 10.0, DEFAULT_TAP_STEPS, t0); // 100 ms interval
        b.tick_settle(t0, true, true);
        assert_eq!(b.tick_nav(t0, END), Some(PhaseEnd(Phase::SkateRight)));
        assert_eq!(b.tick_nav(t0, END), Some(PhaseEnd(Phase::SkateLeft)));
        assert_eq!(b.phase(), Phase::Tap);

        // Due immediately: press, image appears two frames later (16 ms each).
        assert_eq!(b.drive(t0), Drive::Step(1));
        assert_eq!(b.tick_nav(t0, STALL), None);
        assert_eq!(b.drive(ms(t0, 16)), Drive::Step(1)); // still pending
        assert_eq!(b.tick_nav(ms(t0, 16), STALL), None);
        assert_eq!(b.tick_nav(ms(t0, 32), ADV), None);
        // Not due again until t0 + 100 ms.
        assert_eq!(b.drive(ms(t0, 48)), Drive::Idle);
        b.tick_idle(ms(t0, 48));
        assert_eq!(b.drive(ms(t0, 100)), Drive::Step(1));
        assert_eq!(b.tick_nav(ms(t0, 100), ADV), None);

        let r = b.report().tap.unwrap();
        assert_eq!(r.steps, 2);
        assert_eq!(r.stall_frames, 2);
        assert_eq!(r.step_latency_ms.max_ms, 32.0);
        assert_eq!(r.step_latency_ms.median_ms, 0.0);
    }

    #[test]
    fn max_images_caps_both_skate_passes() {
        let t0 = Instant::now();
        let mut b = NavBench::new(1000, 0, Some(3), 6.0, 1, t0);
        b.tick_settle(t0, true, true);
        // Right pass stops after 3 advances without seeing the end.
        assert_eq!(b.tick_nav(ms(t0, 1), ADV), None);
        assert_eq!(b.tick_nav(ms(t0, 2), ADV), None);
        assert_eq!(b.tick_nav(ms(t0, 3), ADV), Some(PhaseEnd(Phase::SkateRight)));
        // Left pass walks back the same 3, also without an at_end.
        assert_eq!(b.tick_nav(ms(t0, 4), ADV), None);
        assert_eq!(b.tick_nav(ms(t0, 5), ADV), None);
        assert_eq!(b.tick_nav(ms(t0, 6), ADV), Some(PhaseEnd(Phase::SkateLeft)));
        let r = b.report();
        assert_eq!(r.skate_right.unwrap().images, 3);
        assert_eq!(r.skate_left.unwrap().images, 3);
    }

    #[test]
    fn tap_step_count_is_configurable() {
        let t0 = Instant::now();
        let mut b = NavBench::new(100, 0, None, 10.0, 2, t0);
        b.tick_settle(t0, true, true);
        b.tick_nav(t0, END);
        b.tick_nav(t0, END);
        assert_eq!(b.tick_nav(t0, ADV), None);
        assert_eq!(b.tick_nav(ms(t0, 100), ADV), Some(PhaseEnd(Phase::Tap)));
        assert!(b.is_done());
    }

    #[test]
    fn tap_ends_at_step_count_or_folder_end() {
        let t0 = Instant::now();
        let mut b = NavBench::new(3, 0, None, 10.0, DEFAULT_TAP_STEPS, t0); // 2 tap steps possible
        b.tick_settle(t0, true, true);
        b.tick_nav(t0, END);
        b.tick_nav(t0, END);
        assert_eq!(b.tick_nav(t0, ADV), None);
        assert_eq!(b.tick_nav(ms(t0, 100), ADV), Some(PhaseEnd(Phase::Tap)));
        assert!(b.is_done());
        assert_eq!(b.drive(ms(t0, 200)), Drive::Done);
    }
}
