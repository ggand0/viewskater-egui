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
use crate::bench::report::{BenchReport, Header, Summary};
use crate::bench::slider::{BenchDrag, SliderBench, SliderFrame};

use super::App;

impl App {
    /// Start whatever `--bench-*` asked for. Called once from `App::new`
    /// after the folder is open.
    pub(super) fn start_benchmarks(&mut self, ctx: &egui::Context) {
        if !self.bench_opts.any() {
            return;
        }
        // --bench-dir folders replace whatever the positional path opened.
        self.bench_dir_idx = 0;
        if let Some(dir) = self.bench_opts.dirs.first().cloned() {
            let options = self.current_discovery_options();
            self.panes.truncate(1);
            self.panes[0].open_path(&dir, ctx, options);
        }
        let n = self.panes[0].image_paths.len();
        if n < 2 {
            log::error!("benchmarks need a folder with at least 2 images");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.bench_run = 1;
        if self.bench_opts.nav || self.bench_opts.slider {
            self.start_run(self.app_start);
        } else if self.bench_opts.preview {
            self.start_preview_bench();
        }
    }

    /// Begin one run on the open folder. `run_start`: process start for
    /// the first run, the folder reopen for every later run, so settle
    /// times stay comparable.
    fn start_run(&mut self, run_start: Instant) {
        self.run_nav_report = None;
        self.panes[0].set_decode_sampling(true);
        if self.bench_opts.nav {
            self.start_nav_bench(run_start);
        } else {
            self.start_slider_bench(run_start);
        }
    }

    fn start_nav_bench(&mut self, run_start: Instant) {
        let n = self.panes[0].image_paths.len();
        log::info!(
            "nav bench: run {}/{} on {n} images",
            self.bench_run,
            self.bench_opts.runs
        );
        self.nav_bench = Some(NavBench::new(
            n,
            self.settings.cache_count,
            self.bench_opts.max_images,
            self.bench_opts.tap_rate,
            self.bench_opts.tap_steps,
            run_start,
        ));
    }

    fn start_slider_bench(&mut self, run_start: Instant) {
        let n = self.panes[0].image_paths.len();
        log::info!(
            "slider bench: run {}/{} on {n} images",
            self.bench_run,
            self.bench_opts.runs
        );
        self.panes[0].set_sync_sampling(true);
        // Background decodes from the nav bench's last phase are not
        // this bench's; start the sink clean.
        let _ = self.panes[0].take_decode_samples();
        self.slider_bench = Some(SliderBench::new(
            n,
            self.bench_opts.sweep_secs,
            self.bench_opts.scrub_anchors,
            self.bench_opts.jumps,
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
        self.preview_bench = Some(crate::bench::preview::PreviewBench::new(n));
    }

    /// One frame of `--bench-nav`. Runs instead of `handle_keyboard`.
    pub(super) fn tick_nav_bench(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let Some(bench) = self.nav_bench.as_mut() else {
            return;
        };
        let (rss, gpu) = self.perf.memory_bytes();
        bench.observe_memory(rss, gpu);

        let drive = bench.drive(now);
        let ended = match drive {
            Drive::Settle => {
                let (has_texture, settled) = self.pane_state();
                let bench = self.nav_bench.as_mut().expect("nav bench");
                bench.tick_settle(now, has_texture, settled)
            }
            Drive::Step(dir) => {
                let outcome = self.step_navigation(dir, ctx);
                let bench = self.nav_bench.as_mut().expect("nav bench");
                bench.tick_nav(now, outcome)
            }
            Drive::Idle => {
                bench.tick_idle(now);
                None
            }
            Drive::Done => None,
        };

        if let Some(end) = ended {
            // The samples collected during the phase that just ended belong
            // to it; taking them also clears the sink for the next phase.
            let samples = self.panes[0].take_decode_samples();
            if let Some(bench) = self.nav_bench.as_mut() {
                // Settle's samples are the initial window fill, not
                // navigation; set_decode_samples ignores that phase.
                bench.set_decode_samples(end.0, samples);
            }
        }

        let done = self.nav_bench.as_ref().is_some_and(NavBench::is_done);
        if done {
            self.finish_nav_bench(ctx);
        }
        // Keep frames coming even when nothing on screen changed.
        ctx.request_repaint();
    }

    fn finish_nav_bench(&mut self, ctx: &egui::Context) {
        let Some(bench) = self.nav_bench.take() else {
            return;
        };
        self.run_nav_report = Some(bench.report());
        if self.bench_opts.slider {
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
        let Some(bench) = self.slider_bench.as_mut() else {
            return;
        };
        bench.observe_memory(rss, gpu);
        let ended = if bench.phase_is_settle() {
            bench.tick_settle(now, has_texture, settled)
        } else {
            bench.tick(now, drag, SliderFrame { target_changed, shown, current_index, has_texture, settled })
        };
        if let Some(end) = ended {
            let (sync, lru_hits) = self.panes[0].take_sync_samples();
            let bg = self.panes[0].take_decode_samples();
            if let Some(bench) = self.slider_bench.as_mut() {
                bench.set_samples(end.0, sync, lru_hits, bg);
            }
        }
        if self.slider_bench.as_ref().is_some_and(SliderBench::is_done) {
            let bench = self.slider_bench.take().expect("slider bench");
            self.panes[0].set_sync_sampling(false);
            self.finish_run(Some(bench.report()), ctx);
        }
        ctx.request_repaint();
    }

    /// Every mode of this run is done: assemble and write the report, then
    /// start the next run, the next folder, the preview bench, or close.
    fn finish_run(&mut self, slider: Option<crate::bench::report::SliderReport>, ctx: &egui::Context) {
        self.panes[0].set_decode_sampling(false);

        let folder = self.panes[0]
            .dir_path
            .clone()
            .unwrap_or_default();
        let report = BenchReport {
            header: Header::new(
                &folder,
                &self.panes[0].image_paths,
                &self.settings,
                self.bench_opts.label.clone(),
                self.bench_run,
                self.bench_opts.runs,
            ),
            nav: self.run_nav_report.take(),
            slider,
        };
        // Straight to stderr, not through the logger: the tracing layer
        // escapes the color codes.
        eprintln!("{}", report.to_text());
        if let Some(dir) = &self.bench_opts.out_dir {
            match report.write(dir) {
                Ok((json, md)) => log::info!("bench report written: {} and {}", json.display(), md.display()),
                Err(e) => log::error!("bench report could not be written to {}: {e}", dir.display()),
            }
        }
        self.bench_reports.push(report);

        let options = self.current_discovery_options();
        if self.bench_run < self.bench_opts.runs {
            // Reopen the folder so the sliding window, LRU and thumbnails
            // start empty again. The OS page cache stays warm, which is
            // what a repeated run is for.
            self.bench_run += 1;
            let reopened = Instant::now();
            self.panes[0].open_path(&folder, ctx, options);
            self.start_run(reopened);
        } else if let Some(next) = self.bench_opts.dirs.get(self.bench_dir_idx + 1).cloned() {
            self.bench_dir_idx += 1;
            self.bench_run = 1;
            let opened = Instant::now();
            self.panes[0].open_path(&next, ctx, options);
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
        if let Some(summary) = Summary::of(&self.bench_reports) {
            if self.bench_reports.len() > 1 {
                eprintln!("{}", summary.to_markdown());
            }
            if let Some(dir) = &self.bench_opts.out_dir {
                match summary.write(dir) {
                    Ok((json, md)) => log::info!("bench summary written: {} and {}", json.display(), md.display()),
                    Err(e) => log::error!("bench summary could not be written to {}: {e}", dir.display()),
                }
            }
        }
        if self.bench_opts.preview {
            self.start_preview_bench();
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
