//! Slider navigation benchmark (`--bench-slider`).
//!
//! Injects a synthetic drag into `paint_nav_slider` (`BenchDrag`, next to
//! the preview bench's `bench_hover_t`) so the throttle, the main-thread
//! sync decode, the decode LRU and the window refill after release all run
//! exactly as for a real drag. Three phases model what people do with the
//! slider:
//!
//! 1. Sweep: drag from the first image to the last over `sweep_secs`, one
//!    position per frame, release, then back from the last to the first
//!    and release again.
//! 2. Scrub: at each of `scrub.anchors` positions spaced evenly from the
//!    first image to the last (five anchors: 0, 25, 50, 75 and 100
//!    percent), press and drag back and forth across `scrub.span` of the
//!    rail (default a tenth of it, 5 percent each side), `scrub.passes`
//!    times (default 2) over `scrub.secs`, then release. A person hunting
//!    for a frame; the return passes revisit images just loaded, so the
//!    LRU shows here.
//! 3. Jump: `jumps` positions spread evenly over the whole rail, visited
//!    first, last, second, second to last so consecutive clicks are far
//!    apart. Each is a press and release in one frame. The click gesture.
//!
//! The phases are independent: the driver empties the decode LRU when a
//! phase ends, so what one phase loaded cannot turn the next phase's
//! loads into cache hits. Every release is followed by a wait for the
//! window to refill, timed.
//! Every jump is followed by a wait for the target image to be the one on
//! screen, timed. The state machine takes the frame's `Instant` from the
//! caller so it can be unit tested with a fake clock.

use std::time::{Duration, Instant};

use super::phase::PhaseStats;
use super::report::{SliderPhaseReport, SliderReport, SyncStats};
use super::LatencyStats;

/// Shape of one scrub gesture, from the `--bench-scrub-*` flags.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrubParams {
    /// How many anchors to scrub around; 0 skips the phase.
    pub anchors: usize,
    /// Width of the region swept, as a share of the rail. The handle goes
    /// half of it each side of the anchor.
    pub span: f32,
    /// Back-and-forth passes per anchor.
    pub passes: usize,
    /// Duration of one gesture, press to release.
    pub secs: f64,
}
/// A wait (for refill, or for a jump target to appear) longer than this is
/// marked timed out and the run moves on.
const WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Injected into `paint_nav_slider` in place of the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BenchDrag {
    /// Position along the rail, 0 to 1.
    pub t: f32,
    /// This frame is the release.
    pub released: bool,
}

/// What the app saw this frame, after applying the slider result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SliderFrame {
    /// `paint_nav_slider` produced a target index different from the
    /// current one (a position was visited).
    pub target_changed: bool,
    /// `apply_slider_target` put a texture on screen this frame.
    pub shown: bool,
    pub current_index: usize,
    pub has_texture: bool,
    /// Sliding window full, nothing decoding anywhere.
    pub settled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Settle,
    Sweep,
    Scrub,
    Jump,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PhaseEnd(pub Phase);

/// How long one synchronous load in `Pane::load_sync` took, per step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SyncLoadTiming {
    pub decode_ms: f64,
    pub convert_ms: f64,
    pub upload_ms: f64,
}

/// Bookkeeping for one slider phase: the shared frame, CPU, memory and
/// background decode figures plus the slider's own counts.
struct GestureStats {
    core: PhaseStats,
    positions: usize,
    shown: usize,
    releases: usize,
    refill_ms: Vec<f64>,
    jump_ms: Vec<f64>,
    sync_loads: Vec<SyncLoadTiming>,
    lru_hits: usize,
}

impl GestureStats {
    fn start(now: Instant) -> Self {
        Self {
            core: PhaseStats::start(now),
            positions: 0,
            shown: 0,
            releases: 0,
            refill_ms: Vec::new(),
            jump_ms: Vec::new(),
            sync_loads: Vec::new(),
            lru_hits: 0,
        }
    }

    fn observe(&mut self, now: Instant, frame: &SliderFrame) {
        self.core.tick_frame(now);
        if frame.target_changed {
            self.positions += 1;
        }
        if frame.shown {
            self.shown += 1;
        }
    }

    fn report(&self, phase: &str) -> SliderPhaseReport {
        let ms = |pick: fn(&SyncLoadTiming) -> f64| {
            LatencyStats::from_ms(&self.sync_loads.iter().map(pick).collect::<Vec<_>>())
        };
        SliderPhaseReport {
            phase: phase.to_string(),
            wall_secs: self.core.wall_secs(),
            positions: self.positions,
            images_shown: self.shown,
            display_ratio: if self.positions > 0 {
                self.shown as f64 / self.positions as f64
            } else {
                0.0
            },
            sync: SyncStats {
                count: self.sync_loads.len(),
                decode_ms: ms(|s| s.decode_ms),
                convert_ms: ms(|s| s.convert_ms),
                upload_ms: ms(|s| s.upload_ms),
                total_ms: ms(|s| s.decode_ms + s.convert_ms + s.upload_ms),
            },
            lru_hits: self.lru_hits,
            releases: self.releases,
            refill_ms: LatencyStats::from_ms(&self.refill_ms),
            jump_ms: (!self.jump_ms.is_empty()).then(|| LatencyStats::from_ms(&self.jump_ms)),
            frame_ms: self.core.frame_stats(),
            bg_decode_ms: self.core.decode_stats(),
            cpu_secs: self.core.cpu_secs,
            peak_rss_mb: self.core.peak_rss_mb(),
            peak_gpu_mb: self.core.peak_gpu_mb(),
            timed_out: self.core.timed_out,
        }
    }
}

/// Where a gesture is within its phase.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    /// Finger down, moving. `since` is when it began.
    Dragging { since: Instant },
    /// Released; waiting for the window to refill.
    Refilling { released_at: Instant },
    /// Clicked; waiting for `target` to be the image on screen.
    Landing { clicked_at: Instant, target: usize },
}

pub(crate) struct SliderBench {
    num_images: usize,
    sweep_secs: f64,
    /// `--bench-skip sweep`.
    skip_sweep: bool,
    scrub_params: ScrubParams,
    /// Rail positions for the scrub and jump phases, disjoint.
    scrub_anchors: Vec<f32>,
    jump_targets: Vec<f32>,
    run_start: Instant,
    phase: Phase,
    settle_first_image: Option<Duration>,
    settle_done: Option<Duration>,
    settle_timed_out: bool,
    /// Which anchor or jump target the phase is on.
    cursor: usize,
    gesture: Option<Gesture>,
    /// Position issued last frame, so a gesture can be released at the
    /// place it stopped.
    last_t: f32,
    sweep: Option<GestureStats>,
    scrub: Option<GestureStats>,
    jump: Option<GestureStats>,
}

impl SliderBench {
    /// `num_images` >= 2. Scrub anchors are spaced evenly from the first
    /// image to the last: `k` anchors at `0, 1/(k-1), ..., 1`; a single
    /// anchor sits in the middle. Jump targets are `jumps` evenly spaced
    /// positions, visited in the preview bench's scrambled order (first,
    /// last, second, second to last, ...) so consecutive clicks are far
    /// apart.
    pub fn new(
        num_images: usize,
        sweep_secs: f64,
        skip_sweep: bool,
        scrub: ScrubParams,
        jumps: usize,
        run_start: Instant,
    ) -> Self {
        let anchors: Vec<f32> = match scrub.anchors {
            0 => Vec::new(),
            1 => vec![0.5],
            k => (0..k).map(|i| i as f32 / (k - 1) as f32).collect(),
        };
        let jump: Vec<f32> = super::scrambled_order(jumps)
            .into_iter()
            .map(|i| (i as f32 + 0.5) / jumps as f32)
            .collect();
        Self {
            num_images,
            sweep_secs: sweep_secs.max(0.1),
            skip_sweep,
            scrub_params: ScrubParams {
                span: scrub.span.clamp(0.0, 1.0),
                passes: scrub.passes.max(1),
                secs: scrub.secs.max(0.1),
                ..scrub
            },
            scrub_anchors: anchors,
            jump_targets: jump,
            run_start,
            phase: Phase::Settle,
            settle_first_image: None,
            settle_done: None,
            settle_timed_out: false,
            cursor: 0,
            gesture: None,
            last_t: 0.0,
            sweep: None,
            scrub: None,
            jump: None,
        }
    }

    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    pub fn phase_is_settle(&self) -> bool {
        self.phase == Phase::Settle
    }

    /// Index the slider maps a rail position to, same formula as
    /// `paint_nav_slider`.
    fn index_of(&self, t: f32) -> usize {
        ((self.num_images - 1) as f32 * t.clamp(0.0, 1.0)).round() as usize
    }

    /// Settle phase: wait for an image and a quiet window.
    pub fn tick_settle(&mut self, now: Instant, has_texture: bool, settled: bool) -> Option<PhaseEnd> {
        debug_assert_eq!(self.phase, Phase::Settle);
        let since = now.duration_since(self.run_start);
        if has_texture && self.settle_first_image.is_none() {
            self.settle_first_image = Some(since);
        }
        let timed_out = since > SETTLE_TIMEOUT;
        if (has_texture && settled) || timed_out {
            if timed_out {
                log::warn!("slider bench: window did not settle within {:?}", SETTLE_TIMEOUT);
                self.settle_timed_out = true;
            } else {
                self.settle_done = Some(since);
            }
            self.start_phase_after(Phase::Settle, now);
            return Some(PhaseEnd(Phase::Settle));
        }
        None
    }

    /// The drag to inject this frame, or None while waiting between
    /// gestures (finger up).
    pub fn drag(&self, now: Instant) -> Option<BenchDrag> {
        let Some(Gesture::Dragging { since }) = self.gesture else {
            return None;
        };
        match self.phase {
            Phase::Sweep => {
                let u = ((now - since).as_secs_f64() / self.sweep_secs).min(1.0) as f32;
                // First pass left to right, second pass back.
                let t = if self.cursor == 0 { u } else { 1.0 - u };
                Some(BenchDrag { t, released: u >= 1.0 })
            }
            Phase::Scrub => {
                let anchor = self.scrub_anchors[self.cursor];
                let u = (now - since).as_secs_f64() / self.scrub_params.secs;
                // Each pass: anchor -> +half -> -half -> anchor, three
                // legs. `passes` of them fill the gesture.
                let p = ((u * self.scrub_params.passes as f64).fract()) as f32;
                let a = self.scrub_params.span / 2.0;
                let t = if u >= 1.0 {
                    anchor
                } else if p < 1.0 / 3.0 {
                    anchor + a * (p * 3.0)
                } else if p < 2.0 / 3.0 {
                    anchor + a - 2.0 * a * ((p - 1.0 / 3.0) * 3.0)
                } else {
                    anchor - a + a * ((p - 2.0 / 3.0) * 3.0)
                };
                Some(BenchDrag { t: t.clamp(0.0, 1.0), released: u >= 1.0 })
            }
            Phase::Jump => Some(BenchDrag { t: self.jump_targets[self.cursor], released: true }),
            Phase::Settle | Phase::Done => None,
        }
    }

    fn stats_for(&mut self, phase: Phase) -> Option<&mut GestureStats> {
        match phase {
            Phase::Sweep => self.sweep.as_mut(),
            Phase::Scrub => self.scrub.as_mut(),
            Phase::Jump => self.jump.as_mut(),
            Phase::Settle | Phase::Done => None,
        }
    }

    /// Stats of the phase in progress.
    fn stats(&mut self) -> Option<&mut GestureStats> {
        let phase = self.phase;
        self.stats_for(phase)
    }

    /// Advance with what the app saw after applying this frame's drag.
    /// `drag` is what `drag(now)` returned before the frame was drawn.
    pub fn tick(&mut self, now: Instant, drag: Option<BenchDrag>, frame: SliderFrame) -> Option<PhaseEnd> {
        let phase = self.phase;
        if matches!(phase, Phase::Settle | Phase::Done) {
            return None;
        }
        if let Some(d) = drag {
            self.last_t = d.t;
        }
        if let Some(s) = self.stats() {
            s.observe(now, &frame);
        }

        // A release this frame ends the drag. The frame that shows the
        // image is observed on the next tick, since the sync decode runs
        // inside this frame.
        if let Some(Gesture::Dragging { .. }) = self.gesture {
            if drag.is_some_and(|d| d.released) {
                if let Some(s) = self.stats() {
                    s.releases += 1;
                }
                self.gesture = Some(match phase {
                    Phase::Jump => Gesture::Landing {
                        clicked_at: now,
                        target: self.index_of(self.last_t),
                    },
                    _ => Gesture::Refilling { released_at: now },
                });
                return None;
            }
        }
        // Landing and refilling can resolve on the same frame when the
        // clicked image came from the cache and nothing needed decoding,
        // so both are checked in order.
        if let Some(Gesture::Landing { clicked_at, target }) = self.gesture {
            let target_on_screen = frame.has_texture && frame.current_index == target;
            let timed_out = now.duration_since(clicked_at) > WAIT_TIMEOUT;
            if target_on_screen || timed_out {
                if let Some(s) = self.stats() {
                    if target_on_screen {
                        s.jump_ms.push((now - clicked_at).as_secs_f64() * 1000.0);
                    } else {
                        s.core.timed_out = true;
                    }
                }
                self.gesture = Some(Gesture::Refilling { released_at: clicked_at });
            }
        }
        let mut gesture_over = false;
        if let Some(Gesture::Refilling { released_at }) = self.gesture {
            let timed_out = now.duration_since(released_at) > WAIT_TIMEOUT;
            if frame.settled || timed_out {
                if let Some(s) = self.stats() {
                    if frame.settled {
                        s.refill_ms.push((now - released_at).as_secs_f64() * 1000.0);
                    } else {
                        log::warn!("slider bench: window did not refill within {:?}", WAIT_TIMEOUT);
                        s.core.timed_out = true;
                    }
                }
                gesture_over = true;
            }
        }

        if !gesture_over {
            return None;
        }

        // Next gesture in this phase, or the next phase.
        let more = match phase {
            Phase::Sweep => self.cursor == 0,
            Phase::Scrub => self.cursor + 1 < self.scrub_anchors.len(),
            Phase::Jump => self.cursor + 1 < self.jump_targets.len(),
            Phase::Settle | Phase::Done => false,
        };
        if more {
            self.cursor += 1;
            self.gesture = Some(Gesture::Dragging { since: now });
            return None;
        }
        if let Some(s) = self.stats() {
            s.core.end(now);
        }
        self.cursor = 0;
        self.start_phase_after(phase, now);
        Some(PhaseEnd(phase))
    }

    /// Enter the next phase that has something to do: sweep unless
    /// skipped, scrub if it has anchors, jump if it has targets, else done.
    fn start_phase_after(&mut self, phase: Phase, now: Instant) {
        let next = match phase {
            Phase::Settle if !self.skip_sweep => Phase::Sweep,
            Phase::Settle | Phase::Sweep if !self.scrub_anchors.is_empty() => Phase::Scrub,
            Phase::Settle | Phase::Sweep | Phase::Scrub if !self.jump_targets.is_empty() => Phase::Jump,
            _ => Phase::Done,
        };
        self.phase = next;
        let stats = match next {
            Phase::Sweep => Some(&mut self.sweep),
            Phase::Scrub => Some(&mut self.scrub),
            Phase::Jump => Some(&mut self.jump),
            Phase::Settle | Phase::Done => None,
        };
        match stats {
            Some(slot) => {
                *slot = Some(GestureStats::start(now));
                self.gesture = Some(Gesture::Dragging { since: now });
            }
            None => self.gesture = None,
        }
    }

    pub fn observe_memory(&mut self, rss_bytes: u64, gpu_bytes: u64) {
        if let Some(s) = self.stats() {
            s.core.observe_memory(rss_bytes, gpu_bytes);
        }
    }

    /// Timings recorded during `phase`, handed over by the driver when
    /// that phase ended: the main-thread loads with their LRU hit count,
    /// and the background decodes.
    pub fn set_timings(
        &mut self,
        phase: Phase,
        sync_loads: Vec<SyncLoadTiming>,
        lru_hits: usize,
        bg_decode_ms: Vec<f64>,
    ) {
        if let Some(s) = self.stats_for(phase) {
            s.sync_loads = sync_loads;
            s.lru_hits = lru_hits;
            s.core.decode_times_ms = bg_decode_ms;
        }
    }

    pub fn report(&self) -> SliderReport {
        SliderReport {
            images: self.num_images,
            sweep_secs: self.sweep_secs,
            scrub_anchors: self.scrub_anchors.len(),
            scrub_span: self.scrub_params.span,
            scrub_passes: self.scrub_params.passes,
            scrub_secs: self.scrub_params.secs,
            jumps: self.jump_targets.len(),
            settle_first_image_ms: self.settle_first_image.map(|d| d.as_secs_f64() * 1000.0),
            settle_settled_ms: self.settle_done.map(|d| d.as_secs_f64() * 1000.0),
            settle_timed_out: self.settle_timed_out,
            sweep: self.sweep.as_ref().map(|s| s.report("sweep")),
            scrub: self.scrub.as_ref().map(|s| s.report("scrub")),
            jump: self.jump.as_ref().map(|s| s.report("jump")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    fn frame(target_changed: bool, shown: bool, current_index: usize, settled: bool) -> SliderFrame {
        SliderFrame { target_changed, shown, current_index, has_texture: true, settled }
    }

    fn scrub(anchors: usize) -> ScrubParams {
        ScrubParams { anchors, span: 0.1, passes: 1, secs: 1.0 }
    }

    fn settled_bench(n: usize, anchors: usize, jumps: usize, t0: Instant) -> SliderBench {
        let mut b = SliderBench::new(n, 1.0, false, scrub(anchors), jumps, t0);
        assert_eq!(b.tick_settle(t0, true, true), Some(PhaseEnd(Phase::Settle)));
        assert_eq!(b.phase(), Phase::Sweep);
        b
    }

    /// Run both sweep passes with instant refills. Returns the time the
    /// sweep phase ended.
    fn finish_sweep(b: &mut SliderBench, t0: Instant) -> Instant {
        let d = b.drag(ms(t0, 1000)).unwrap();
        b.tick(ms(t0, 1000), Some(d), frame(true, true, 100, false));
        assert_eq!(b.tick(ms(t0, 1001), None, frame(false, false, 100, true)), None);
        let d = b.drag(ms(t0, 2001)).unwrap();
        assert_eq!(d, BenchDrag { t: 0.0, released: true });
        b.tick(ms(t0, 2001), Some(d), frame(true, true, 0, false));
        assert_eq!(b.tick(ms(t0, 2002), None, frame(false, false, 0, true)), Some(PhaseEnd(Phase::Sweep)));
        ms(t0, 2002)
    }

    #[test]
    fn anchors_span_the_folder_and_jumps_are_spread_far_apart() {
        let b = SliderBench::new(101, 1.0, false, scrub(3), 4, Instant::now());
        assert_eq!(b.scrub_anchors, vec![0.0, 0.5, 1.0]);
        assert_eq!(SliderBench::new(101, 1.0, false, scrub(5), 0, Instant::now()).scrub_anchors, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_eq!(SliderBench::new(101, 1.0, false, scrub(1), 0, Instant::now()).scrub_anchors, vec![0.5]);
        // Four jumps at 1/8, 3/8, 5/8, 7/8, visited first, last, second,
        // second to last.
        assert_eq!(b.jump_targets, vec![0.125, 0.875, 0.375, 0.625]);
        assert_eq!(b.index_of(0.875), 88);
        assert_eq!(SliderBench::new(140, 1.0, false, scrub(5), 20, Instant::now()).jump_targets.len(), 20);
    }

    #[test]
    fn sweep_goes_right_releases_refills_then_comes_back() {
        let t0 = Instant::now();
        let mut b = settled_bench(101, 0, 0, t0);
        // Halfway through the second: halfway along the rail, not released.
        let d = b.drag(ms(t0, 500)).unwrap();
        assert!((d.t - 0.5).abs() < 1e-6 && !d.released);
        assert_eq!(b.tick(ms(t0, 500), Some(d), frame(true, true, 50, false)), None);
        // At the end: released at t = 1.
        let d = b.drag(ms(t0, 1000)).unwrap();
        assert_eq!(d, BenchDrag { t: 1.0, released: true });
        assert_eq!(b.tick(ms(t0, 1000), Some(d), frame(true, false, 100, false)), None);
        // Finger up while the window refills.
        assert_eq!(b.drag(ms(t0, 1010)), None);
        assert_eq!(b.tick(ms(t0, 1010), None, frame(false, false, 100, false)), None);
        // Refilled: the phase is not over, the return pass starts here.
        assert_eq!(b.tick(ms(t0, 1200), None, frame(false, false, 100, true)), None);
        let back = b.drag(ms(t0, 1700)).unwrap();
        assert!((back.t - 0.5).abs() < 1e-6 && !back.released, "{back:?}");
        b.tick(ms(t0, 1700), Some(back), frame(true, true, 50, false));
        let home = b.drag(ms(t0, 2200)).unwrap();
        assert_eq!(home, BenchDrag { t: 0.0, released: true });
        b.tick(ms(t0, 2200), Some(home), frame(true, true, 0, false));
        assert_eq!(b.tick(ms(t0, 2300), None, frame(false, false, 0, true)), Some(PhaseEnd(Phase::Sweep)));
        assert!(b.is_done(), "no anchors and no jumps: done after the sweep");
        let r = b.report().sweep.unwrap();
        assert_eq!(r.positions, 4);
        assert_eq!(r.images_shown, 3);
        assert_eq!(r.releases, 2);
        assert_eq!(r.refill_ms.count, 2);
        assert_eq!(r.refill_ms.max_ms, 200.0);
    }

    #[test]
    fn scrub_repeats_its_passes_across_the_span() {
        let t0 = Instant::now();
        let params = ScrubParams { anchors: 1, span: 0.4, passes: 2, secs: 2.0 };
        let mut b = SliderBench::new(101, 1.0, false, params, 0, t0);
        b.tick_settle(t0, true, true);
        let s0 = finish_sweep(&mut b, t0);
        assert_eq!(b.phase(), Phase::Scrub);
        // One anchor sits at the middle, half span 0.2. Each pass is
        // anchor -> +0.2 -> -0.2 -> anchor over one second; two passes.
        let at = |ms_: u64| b.drag(s0 + Duration::from_millis(ms_)).unwrap().t;
        assert!((at(333) - 0.7).abs() < 2e-3, "{}", at(333));
        assert!((at(667) - 0.3).abs() < 2e-3, "{}", at(667));
        assert!((at(1000) - 0.5).abs() < 2e-3, "{}", at(1000));
        assert!((at(1333) - 0.7).abs() < 2e-3, "second pass peak: {}", at(1333));
        let end = b.drag(s0 + Duration::from_millis(2000)).unwrap();
        assert!(end.released && (end.t - 0.5).abs() < 1e-6);
    }

    #[test]
    fn scrub_moves_to_the_next_anchor_after_each_refill() {
        let t0 = Instant::now();
        let mut b = settled_bench(101, 2, 0, t0);
        let s0 = finish_sweep(&mut b, t0);
        assert_eq!(b.phase(), Phase::Scrub);
        // Two anchors: the first image and the last. Around the first,
        // the left half of the motion clamps at the rail start.
        let first = b.drag(s0).unwrap();
        assert_eq!(first.t, 0.0);
        let trough = b.drag(s0 + Duration::from_millis(667)).unwrap();
        assert_eq!(trough.t, 0.0, "clamped at the rail start");
        let end = b.drag(s0 + Duration::from_millis(1000)).unwrap();
        assert!(end.released);
        b.tick(s0 + Duration::from_millis(1000), Some(end), frame(true, true, 0, false));
        // Refilled: on to the second anchor, a fresh press.
        assert_eq!(b.tick(s0 + Duration::from_millis(1050), None, frame(false, false, 0, true)), None);
        assert_eq!(b.phase(), Phase::Scrub);
        let d = b.drag(s0 + Duration::from_millis(1050)).unwrap();
        assert_eq!(d.t, 1.0, "{d:?}");
    }

    #[test]
    fn jump_measures_click_to_target_on_screen_then_refill() {
        let t0 = Instant::now();
        let mut b = settled_bench(101, 0, 2, t0);
        let j0 = finish_sweep(&mut b, t0);
        assert_eq!(b.phase(), Phase::Jump);
        // Two jumps: 1/4 and 3/4 of the rail.
        let click = b.drag(j0).unwrap();
        assert_eq!(click, BenchDrag { t: 0.25, released: true });
        let target = b.index_of(0.25);
        assert_eq!(target, 25);
        // The sync decode blocks the click frame; the next tick shows it.
        assert_eq!(b.tick(j0, Some(click), frame(true, false, 0, false)), None);
        assert_eq!(b.drag(j0 + Duration::from_millis(90)), None);
        assert_eq!(b.tick(j0 + Duration::from_millis(90), None, frame(false, true, target, false)), None);
        assert_eq!(b.tick(j0 + Duration::from_millis(300), None, frame(false, false, target, true)), None);
        // Second jump, to 3/4.
        let click2 = b.drag(j0 + Duration::from_millis(300)).unwrap();
        assert_eq!(click2.t, 0.75);
        let target2 = b.index_of(0.75);
        b.tick(j0 + Duration::from_millis(300), Some(click2), frame(true, true, target2, false));
        assert_eq!(b.tick(j0 + Duration::from_millis(310), None, frame(false, false, target2, true)), Some(PhaseEnd(Phase::Jump)));
        assert!(b.is_done());
        let r = b.report().jump.unwrap();
        assert_eq!(r.jump_ms.unwrap().max_ms, 90.0);
        assert_eq!(r.refill_ms.count, 2);
        assert_eq!(r.refill_ms.max_ms, 300.0);
    }

    #[test]
    fn every_phase_can_be_skipped() {
        let t0 = Instant::now();
        // Sweep skipped: settle goes straight to the scrub.
        let mut b = SliderBench::new(101, 1.0, true, scrub(2), 2, t0);
        b.tick_settle(t0, true, true);
        assert_eq!(b.phase(), Phase::Scrub);
        assert!(b.drag(t0).is_some());
        // Sweep and scrub skipped: straight to the jumps.
        let mut b = SliderBench::new(101, 1.0, true, scrub(0), 2, t0);
        b.tick_settle(t0, true, true);
        assert_eq!(b.phase(), Phase::Jump);
        // Everything skipped: done at settle.
        let mut b = SliderBench::new(101, 1.0, true, scrub(0), 0, t0);
        b.tick_settle(t0, true, true);
        assert!(b.is_done());
        let r = b.report();
        assert!(r.sweep.is_none() && r.scrub.is_none() && r.jump.is_none());
    }

    #[test]
    fn a_wait_that_never_ends_times_out_and_moves_on() {
        let t0 = Instant::now();
        let mut b = settled_bench(101, 0, 0, t0);
        let d = b.drag(ms(t0, 1000)).unwrap();
        b.tick(ms(t0, 1000), Some(d), frame(true, true, 100, false));
        assert_eq!(b.tick(ms(t0, 5000), None, frame(false, false, 100, false)), None);
        // Timed out after the first pass: the return pass still runs.
        assert_eq!(b.tick(ms(t0, 11_100), None, frame(false, false, 100, false)), None);
        assert!(b.drag(ms(t0, 11_100)).is_some(), "return pass begins");
        let d = b.drag(ms(t0, 12_200)).unwrap();
        assert!(d.released);
        b.tick(ms(t0, 12_200), Some(d), frame(true, true, 0, false));
        assert_eq!(b.tick(ms(t0, 12_300), None, frame(false, false, 0, true)), Some(PhaseEnd(Phase::Sweep)));
        assert!(b.report().sweep.unwrap().timed_out);
    }
}
