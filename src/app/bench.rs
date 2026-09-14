//! Drives the in-app benchmarks from `App::update`.
//!
//! `--bench-nav` replaces the keyboard for the duration of the run: every
//! frame the bench says what a held key would do, the app runs the same
//! `step_navigation` a key press runs, and the outcome goes back to the
//! bench. When the run ends the report is logged, written if `--bench-out`
//! was given, and the next run or benchmark starts, or the app closes.

use std::time::Instant;

use eframe::egui;

use crate::bench::nav::{Drive, NavBench};
use crate::bench::report::{BenchReport, Header, Summary};

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
        if self.bench_opts.nav {
            self.start_nav_bench(self.app_start);
        } else if self.bench_opts.preview {
            self.start_preview_bench();
        }
    }

    /// `run_start`: process start for the first run, the folder reopen
    /// for every later run, so settle times stay comparable.
    fn start_nav_bench(&mut self, run_start: Instant) {
        let n = self.panes[0].image_paths.len();
        log::info!(
            "nav bench: run {}/{} on {n} images",
            self.bench_run,
            self.bench_opts.runs
        );
        self.panes[0].set_decode_sampling(true);
        self.nav_bench = Some(NavBench::new(
            n,
            self.settings.cache_count,
            self.bench_opts.max_images,
            self.bench_opts.tap_rate,
            self.bench_opts.tap_steps,
            run_start,
        ));
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

        let ended = match bench.drive(now) {
            Drive::Settle => {
                let pane = &self.panes[0];
                let has_texture = pane.current_texture.is_some();
                // The new window is full and no decode thread is alive
                // anywhere, including leftovers from the previous run whose
                // cache was dropped at reopen.
                let settled = pane.is_settled() && crate::cache::active_decode_threads() == 0;
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
            nav: Some(bench.report()),
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
            self.start_nav_bench(reopened);
        } else if let Some(next) = self.bench_opts.dirs.get(self.bench_dir_idx + 1).cloned() {
            self.bench_dir_idx += 1;
            self.bench_run = 1;
            let opened = Instant::now();
            self.panes[0].open_path(&next, ctx, options);
            if self.panes[0].image_paths.len() < 2 {
                log::error!("bench: {} has fewer than 2 images, skipping", next.display());
                self.finish_all_benchmarks(ctx);
            } else {
                self.start_nav_bench(opened);
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
