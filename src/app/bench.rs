//! Drives the in-app benchmarks from `App::update`.
//!
//! A run is: `--bench-nav` if asked, then `--bench-slider` if asked, on
//! one folder. The nav bench replaces the keyboard: every frame it says
//! what a held key would do, the app runs the same `step_navigation` a key
//! press runs, and the outcome goes back. The slider bench replaces the
//! pointer on the slider the same way (`BenchDrag`). When a run ends its
//! report is printed, written if `--bench-out` was given, and the next
//! run, folder or benchmark starts, or the app closes.

use std::time::Instant;

use eframe::egui;

use crate::bench::nav::{Drive, NavBench};
use crate::bench::preview::PreviewBench;
use crate::bench::report::{BenchReport, Header, NavReport, SliderReport, Summary};
use crate::bench::slider::{BenchDrag, ScrubParams, SliderBench, SliderFrame};
use crate::bench::{BenchOptions, SkipPhase};

use super::App;

/// Everything the benchmarks keep on the app between frames.
pub(crate) struct BenchState {
    pub opts: BenchOptions,
    pub preview: Option<PreviewBench>,
    pub nav: Option<NavBench>,
    pub slider: Option<SliderBench>,
    /// Nav report of the current run, kept until the run's other modes
    /// finish and the report is assembled.
    run_nav_report: Option<NavReport>,
    /// 1-based index of the current run.
    run: usize,
    /// Index into `opts.dirs` of the folder being benchmarked.
    dir_idx: usize,
    /// Every finished run, for the summary written at the end.
    reports: Vec<BenchReport>,
    /// Process start, for the first run's time-to-first-image.
    app_start: Instant,
}

impl BenchState {
    pub fn new(opts: BenchOptions, app_start: Instant) -> Self {
        Self {
            opts,
            preview: None,
            nav: None,
            slider: None,
            run_nav_report: None,
            run: 0,
            dir_idx: 0,
            reports: Vec::new(),
            app_start,
        }
    }
}

impl App {
    /// Start whatever `--bench-*` asked for. Called once from `App::new`
    /// after the folder is open.
    pub(super) fn start_benchmarks(&mut self, ctx: &egui::Context) {
        if !self.bench.opts.any() {
            return;
        }
        // --bench-dir folders replace whatever the positional path opened.
        self.bench.dir_idx = 0;
        if let Some(dir) = self.bench.opts.dirs.first().cloned() {
            let options = self.current_discovery_options();
            self.panes.truncate(1);
            self.panes[0].open_path(&dir, ctx, options, &mut self.stars);
        }
        let n = self.panes[0].image_paths.len();
        if n < 2 {
            log::error!("benchmarks need a folder with at least 2 images");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.bench.run = 1;
        if self.bench.opts.nav || self.bench.opts.slider {
            self.start_run(self.bench.app_start);
        } else if self.bench.opts.preview {
            self.start_preview_bench();
        }
    }

    /// Begin one run on the open folder. `run_start`: process start for
    /// the first run, the folder reopen for every later run, so settle
    /// times stay comparable.
    fn start_run(&mut self, run_start: Instant) {
        self.bench.run_nav_report = None;
        self.panes[0].record_decode_times(true);
        if self.bench.opts.nav {
            self.start_nav_bench(run_start);
        } else {
            self.start_slider_bench(run_start);
        }
    }

    fn start_nav_bench(&mut self, run_start: Instant) {
        let n = self.panes[0].image_paths.len();
        log::info!(
            "nav bench: run {}/{} on {n} images",
            self.bench.run,
            self.bench.opts.runs
        );
        self.bench.nav = Some(NavBench::new(
            n,
            self.settings.cache_count,
            self.bench.opts.max_images,
            self.bench.opts.skips(SkipPhase::SkateLeft),
            self.bench.opts.tap_rate,
            self.bench.opts.tap_steps,
            run_start,
        ));
    }

    fn start_slider_bench(&mut self, run_start: Instant) {
        let n = self.panes[0].image_paths.len();
        log::info!(
            "slider bench: run {}/{} on {n} images",
            self.bench.run,
            self.bench.opts.runs
        );
        self.panes[0].record_sync_load_times(true);
        // Background decodes from the nav bench's last phase are not
        // this bench's; drop what was recorded. The LRU is emptied too so
        // the sweep does not hit images the keyboard bench loaded.
        let _ = self.panes[0].take_decode_times();
        self.panes[0].clear_decode_lru();
        let opts = &self.bench.opts;
        let scrub = ScrubParams {
            anchors: if opts.skips(SkipPhase::Scrub) { 0 } else { opts.scrub.anchors },
            ..opts.scrub
        };
        let jumps = if opts.skips(SkipPhase::Jump) { 0 } else { opts.jumps };
        self.bench.slider = Some(SliderBench::new(
            n,
            opts.sweep_secs,
            opts.skips(SkipPhase::Sweep),
            scrub,
            jumps,
            run_start,
        ));
    }

    /// Whether the pane is showing an image, and whether nothing is
    /// loading anywhere: the new window is full and no decode thread is
    /// alive, including leftovers from a previous run whose cache was
    /// dropped at reopen.
    fn pane_state(&self) -> (bool, bool) {
        let pane = &self.panes[0];
        let has_texture = pane.current_texture.is_some();
        let settled = pane.is_settled() && crate::cache::active_decode_threads() == 0;
        (has_texture, settled)
    }

    pub(super) fn start_preview_bench(&mut self) {
        let n = self.panes[0].image_paths.len();
        log::info!("preview bench: starting on {n} images");
        self.bench.preview = Some(PreviewBench::new(n));
    }

    /// One frame of `--bench-nav`. Runs instead of `handle_keyboard`.
    pub(super) fn tick_nav_bench(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let Some(bench) = self.bench.nav.as_mut() else {
            return;
        };
        let (rss, gpu) = self.perf.memory_bytes();
        bench.observe_memory(rss, gpu);

        let drive = bench.drive(now);
        let ended = match drive {
            Drive::Settle => {
                let (has_texture, settled) = self.pane_state();
                let bench = self.bench.nav.as_mut().expect("nav bench");
                bench.tick_settle(now, has_texture, settled)
            }
            Drive::Step(dir) => {
                let outcome = self.step_navigation(dir, ctx);
                let bench = self.bench.nav.as_mut().expect("nav bench");
                bench.tick_nav(now, outcome)
            }
            Drive::Idle => {
                bench.tick_idle(now);
                None
            }
            Drive::Done => None,
        };

        if let Some(end) = ended {
            // The decode times recorded during the phase that just ended
            // belong to it; taking them also clears the list for the next.
            let times = self.panes[0].take_decode_times();
            if let Some(bench) = self.bench.nav.as_mut() {
                // Settle's decodes are the initial window fill, not
                // navigation; set_decode_times ignores that phase.
                bench.set_decode_times(end.0, times);
            }
        }

        let done = self.bench.nav.as_ref().is_some_and(NavBench::is_done);
        if done {
            self.finish_nav_bench(ctx);
        }
        // Keep frames coming even when nothing on screen changed.
        ctx.request_repaint();
    }

    fn finish_nav_bench(&mut self, ctx: &egui::Context) {
        let Some(bench) = self.bench.nav.take() else {
            return;
        };
        self.bench.run_nav_report = Some(bench.report());
        if self.bench.opts.slider {
            // Same run, same run_start semantics: the slider bench settles
            // from now, since the folder was not reopened.
            self.start_slider_bench(Instant::now());
        } else {
            self.finish_run(None, ctx);
        }
    }

    /// One frame of `--bench-slider`, after the slider result was applied.
    /// `drag` is what the bench injected this frame.
    pub(super) fn tick_slider_bench(
        &mut self,
        now: Instant,
        drag: Option<BenchDrag>,
        target_changed: bool,
        shown: bool,
        ctx: &egui::Context,
    ) {
        let (rss, gpu) = self.perf.memory_bytes();
        let (has_texture, settled) = self.pane_state();
        let current_index = self.panes[0].current_index;
        let Some(bench) = self.bench.slider.as_mut() else {
            return;
        };
        bench.observe_memory(rss, gpu);
        let ended = if bench.phase_is_settle() {
            bench.tick_settle(now, has_texture, settled)
        } else {
            bench.tick(now, drag, SliderFrame { target_changed, shown, current_index, has_texture, settled })
        };
        if let Some(end) = ended {
            let (sync_loads, lru_hits) = self.panes[0].take_sync_load_times();
            let bg = self.panes[0].take_decode_times();
            if let Some(bench) = self.bench.slider.as_mut() {
                bench.set_timings(end.0, sync_loads, lru_hits, bg);
            }
            // Each phase starts from the same state: nothing the previous
            // phase loaded stays in the LRU to turn its loads into hits.
            self.panes[0].clear_decode_lru();
        }
        if self.bench.slider.as_ref().is_some_and(SliderBench::is_done) {
            let bench = self.bench.slider.take().expect("slider bench");
            self.panes[0].record_sync_load_times(false);
            self.finish_run(Some(bench.report()), ctx);
        }
        ctx.request_repaint();
    }

    /// Every mode of this run is done: assemble and write the report, then
    /// start the next run, the next folder, the preview bench, or close.
    fn finish_run(&mut self, slider: Option<SliderReport>, ctx: &egui::Context) {
        self.panes[0].record_decode_times(false);

        let folder = self.panes[0]
            .dir_path
            .clone()
            .unwrap_or_default();
        let report = BenchReport {
            header: Header::new(
                &folder,
                &self.panes[0].image_paths,
                &self.settings,
                self.bench.opts.label.clone(),
                self.bench.run,
                self.bench.opts.runs,
            ),
            nav: self.bench.run_nav_report.take(),
            slider,
        };
        // Straight to stderr, not through the logger: the tracing layer
        // escapes the color codes.
        eprintln!("{}", report.to_text());
        if let Some(dir) = &self.bench.opts.out_dir {
            match report.write(dir) {
                Ok((json, md)) => log::info!("bench report written: {} and {}", json.display(), md.display()),
                Err(e) => log::error!("bench report could not be written to {}: {e}", dir.display()),
            }
        }
        self.bench.reports.push(report);

        let options = self.current_discovery_options();
        if self.bench.run < self.bench.opts.runs {
            // Reopen the folder so the sliding window, LRU and thumbnails
            // start empty again. The OS page cache stays warm, which is
            // what a repeated run is for.
            self.bench.run += 1;
            let reopened = Instant::now();
            self.panes[0].open_path(&folder, ctx, options, &mut self.stars);
            self.start_run(reopened);
        } else if let Some(next) = self.bench.opts.dirs.get(self.bench.dir_idx + 1).cloned() {
            self.bench.dir_idx += 1;
            self.bench.run = 1;
            let opened = Instant::now();
            self.panes[0].open_path(&next, ctx, options, &mut self.stars);
            if self.panes[0].image_paths.len() < 2 {
                log::error!("bench: {} has fewer than 2 images, skipping", next.display());
                self.finish_all_benchmarks(ctx);
            } else {
                self.start_run(opened);
            }
        } else {
            self.finish_all_benchmarks(ctx);
        }
    }

    /// Every nav run is done: write the cross-run summary, then hand over
    /// to the preview bench or close.
    fn finish_all_benchmarks(&mut self, ctx: &egui::Context) {
        if let Some(summary) = Summary::of(&self.bench.reports) {
            if self.bench.reports.len() > 1 {
                eprintln!("{}", summary.to_markdown());
            }
            if let Some(dir) = &self.bench.opts.out_dir {
                match summary.write(dir) {
                    Ok((json, md)) => log::info!("bench summary written: {} and {}", json.display(), md.display()),
                    Err(e) => log::error!("bench summary could not be written to {}: {e}", dir.display()),
                }
            }
        }
        if self.bench.opts.preview {
            self.start_preview_bench();
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
