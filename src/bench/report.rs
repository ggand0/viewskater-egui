//! Benchmark report: one JSON file and one markdown file per run, plus the
//! text logged at the end. Everything needed to reproduce a run is in the
//! header: commit, profile, machine, settings, folder.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::LatencyStats;
use crate::build_info::BuildInfo;
use crate::settings::AppSettings;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SettingsSnapshot {
    pub cache_count: usize,
    pub decode_threads: usize,
    pub lru_budget_mb: usize,
    pub gpu_memory_mode: String,
    pub preview_budget_mb: usize,
}

impl SettingsSnapshot {
    pub fn from(settings: &AppSettings) -> Self {
        Self {
            cache_count: settings.cache_count,
            decode_threads: settings.decode_threads,
            lru_budget_mb: settings.lru_budget_mb,
            gpu_memory_mode: format!("{:?}", settings.gpu_memory_mode),
            preview_budget_mb: settings.preview_budget_mb,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Header {
    pub date_utc: String,
    pub version: String,
    pub git_hash: String,
    pub profile: String,
    pub platform: String,
    pub hostname: String,
    pub folder: PathBuf,
    pub image_count: usize,
    pub total_bytes: u64,
    pub label: Option<String>,
    pub run: usize,
    pub runs: usize,
    pub settings: SettingsSnapshot,
}

impl Header {
    pub fn new(
        folder: &Path,
        image_paths: &[PathBuf],
        settings: &AppSettings,
        label: Option<String>,
        run: usize,
        runs: usize,
    ) -> Self {
        let total_bytes = image_paths
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        Self {
            date_utc: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            version: BuildInfo::version().to_string(),
            git_hash: BuildInfo::git_hash_short().to_string(),
            profile: BuildInfo::build_profile().to_string(),
            platform: BuildInfo::target_platform().to_string(),
            hostname: sysinfo::System::host_name().unwrap_or_else(|| "unknown".into()),
            folder: folder.to_path_buf(),
            image_count: image_paths.len(),
            total_bytes,
            label,
            run,
            runs,
            settings: SettingsSnapshot::from(settings),
        }
    }

    fn folder_name(&self) -> String {
        self.folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "folder".into())
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SettleReport {
    /// From run start to the first frame that showed an image. Run start
    /// is process start for the first run and the folder reopen for later
    /// runs, so only the first run includes window creation.
    pub first_image_ms: Option<f64>,
    /// From run start to the first frame with a full sliding window.
    pub settled_ms: Option<f64>,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SkateReport {
    pub direction: String,
    /// Images shown and counted in the rate.
    pub images: usize,
    /// Images shown but excluded from the rate: prefetched before the phase.
    pub skipped_images: usize,
    pub wall_secs: f64,
    pub images_per_sec: f64,
    /// Frames after the skipped images, and how many of them moved nothing
    /// because the next texture was not ready.
    pub frames: usize,
    pub stall_frames: usize,
    pub stall_share: f64,
    pub frame_ms: LatencyStats,
    pub decode_ms: LatencyStats,
    pub cpu_secs: Option<f64>,
    pub peak_rss_mb: f64,
    pub peak_gpu_mb: f64,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapReport {
    pub rate_per_sec: f64,
    pub steps: usize,
    pub wall_secs: f64,
    /// Press to image shown.
    pub step_latency_ms: LatencyStats,
    pub frames: usize,
    pub stall_frames: usize,
    pub frame_ms: LatencyStats,
    pub decode_ms: LatencyStats,
    pub cpu_secs: Option<f64>,
    pub peak_rss_mb: f64,
    pub peak_gpu_mb: f64,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NavReport {
    pub images: usize,
    pub settle: SettleReport,
    pub skate_right: Option<SkateReport>,
    pub skate_left: Option<SkateReport>,
    pub tap: Option<TapReport>,
}

/// Main-thread sync loads from `Pane::load_sync` during one phase.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct SyncStats {
    pub count: usize,
    pub decode_ms: LatencyStats,
    pub convert_ms: LatencyStats,
    /// Time to hand the pixels to egui. egui queues the GPU copy for the
    /// end of the frame, so this is near zero and the real upload cost
    /// shows in frame time. Kept for completeness, not printed.
    pub upload_ms: LatencyStats,
    /// decode + convert + upload: how long the frame was blocked.
    pub total_ms: LatencyStats,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SliderPhaseReport {
    pub phase: String,
    pub wall_secs: f64,
    /// Slider positions visited (target index changed).
    pub positions: usize,
    /// Positions that put an image on screen; the rest were skipped by
    /// the 10 ms throttle or had no texture.
    pub images_shown: usize,
    pub display_ratio: f64,
    pub sync: SyncStats,
    pub lru_hits: usize,
    pub releases: usize,
    /// Release to a full sliding window.
    pub refill_ms: LatencyStats,
    /// Jump phase only: click to the target image being on screen.
    pub jump_ms: Option<LatencyStats>,
    pub frame_ms: LatencyStats,
    pub bg_decode_ms: LatencyStats,
    pub cpu_secs: Option<f64>,
    pub peak_rss_mb: f64,
    pub peak_gpu_mb: f64,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SliderReport {
    pub images: usize,
    pub sweep_secs: f64,
    pub scrub_anchors: usize,
    pub jumps: usize,
    pub settle_first_image_ms: Option<f64>,
    pub settle_settled_ms: Option<f64>,
    pub settle_timed_out: bool,
    pub sweep: Option<SliderPhaseReport>,
    pub scrub: Option<SliderPhaseReport>,
    pub jump: Option<SliderPhaseReport>,
}

impl SliderReport {
    fn phases(&self) -> impl Iterator<Item = &SliderPhaseReport> {
        [&self.sweep, &self.scrub, &self.jump].into_iter().flatten()
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BenchReport {
    pub header: Header,
    pub nav: Option<NavReport>,
    pub slider: Option<SliderReport>,
}

fn opt_secs(v: Option<f64>) -> String {
    v.map_or("n/a".into(), |s| format!("{s:.2}"))
}

fn opt_ms(v: Option<f64>) -> String {
    v.map_or("n/a".into(), |s| format!("{s:.0} ms"))
}

fn flag(timed_out: bool) -> &'static str {
    if timed_out { " (timed out)" } else { "" }
}

/// Bold cyan when the log goes to a terminal, so the headline numbers
/// stand out. Plain when redirected.
fn highlight(text: String) -> String {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        format!("\x1b[1;36m{text}\x1b[0m")
    } else {
        text
    }
}

impl BenchReport {
    /// Multi-line text for the log.
    pub fn to_text(&self) -> String {
        let h = &self.header;
        let mut out = format!(
            "bench report: {} ({} images, {:.0} MB) run {}/{}{}\n\
             {} {} {} {} on {}; cache_count={} decode_threads={} lru_budget_mb={} gpu={}\n",
            h.folder.display(),
            h.image_count,
            h.total_bytes as f64 / (1024.0 * 1024.0),
            h.run,
            h.runs,
            h.label.as_deref().map_or(String::new(), |l| format!(" [{l}]")),
            h.version,
            h.git_hash,
            h.profile,
            h.platform,
            h.hostname,
            h.settings.cache_count,
            h.settings.decode_threads,
            h.settings.lru_budget_mb,
            h.settings.gpu_memory_mode,
        );
        if let Some(nav) = &self.nav {
            out.push_str(&format!(
                "settle: first image {}, window full {}{}; p50 below means median\n",
                opt_ms(nav.settle.first_image_ms),
                opt_ms(nav.settle.settled_ms),
                flag(nav.settle.timed_out),
            ));
            for s in [&nav.skate_right, &nav.skate_left].into_iter().flatten() {
                out.push_str(&format!(
                    "skate {}: {} images in {:.2}s = {} (+{} prefetched), stalls {}/{} frames ({}), \
                     frame p50={:.1} p99={:.1} max={:.1} ms, decode n={} p50={:.1} p95={:.1} max={:.1} ms, \
                     cpu {}s, peak rss {:.0} MB gpu {:.0} MB{}\n",
                    s.direction, s.images, s.wall_secs,
                    highlight(format!("{:.1} img/s", s.images_per_sec)),
                    s.skipped_images,
                    s.stall_frames, s.frames,
                    highlight(format!("{:.0}%", s.stall_share * 100.0)),
                    s.frame_ms.median_ms, s.frame_ms.p99_ms, s.frame_ms.max_ms,
                    s.decode_ms.count, s.decode_ms.median_ms, s.decode_ms.p95_ms, s.decode_ms.max_ms,
                    opt_secs(s.cpu_secs), s.peak_rss_mb, s.peak_gpu_mb, flag(s.timed_out),
                ));
            }
            if let Some(t) = &nav.tap {
                out.push_str(&format!(
                    "tap {:.1}/s: {} steps in {:.2}s, press-to-image p50={} p95={} max={:.1} ms, \
                     stalls {}/{} frames, frame p50={:.1} p99={:.1} max={:.1} ms, decode n={} p50={:.1} p95={:.1} max={:.1} ms, \
                     cpu {}s, peak rss {:.0} MB gpu {:.0} MB{}\n",
                    t.rate_per_sec, t.steps, t.wall_secs,
                    highlight(format!("{:.1}", t.step_latency_ms.median_ms)),
                    highlight(format!("{:.1}", t.step_latency_ms.p95_ms)),
                    t.step_latency_ms.max_ms,
                    t.stall_frames, t.frames,
                    t.frame_ms.median_ms, t.frame_ms.p99_ms, t.frame_ms.max_ms,
                    t.decode_ms.count, t.decode_ms.median_ms, t.decode_ms.p95_ms, t.decode_ms.max_ms,
                    opt_secs(t.cpu_secs), t.peak_rss_mb, t.peak_gpu_mb, flag(t.timed_out),
                ));
            }
        }
        if let Some(sl) = &self.slider {
            out.push_str(&format!(
                "slider (sweep {:.1}s, {} scrub anchors, {} jumps): settle first image {}, window full {}{}\n",
                sl.sweep_secs, sl.scrub_anchors, sl.jumps,
                opt_ms(sl.settle_first_image_ms), opt_ms(sl.settle_settled_ms), flag(sl.settle_timed_out),
            ));
            for ph in sl.phases() {
                let jump = ph.jump_ms.map_or(String::new(), |j| {
                    format!(", click-to-image p50={} p95={:.1} max={:.1} ms", highlight(format!("{:.1}", j.median_ms)), j.p95_ms, j.max_ms)
                });
                out.push_str(&format!(
                    "slider {}: {} positions, {} shown ({}) in {:.2}s, sync loads n={} block p50={} p95={:.1} max={:.1} ms \
                     (decode {:.1} convert {:.1} p50), lru hits {}, refill after {} releases p50={:.0} max={:.0} ms{}, \
                     frame p50={:.1} p99={:.1} max={:.1} ms, bg decode n={} p50={:.1} ms, cpu {}s, peak rss {:.0} MB gpu {:.0} MB{}\n",
                    ph.phase, ph.positions, ph.images_shown,
                    highlight(format!("{:.0}%", ph.display_ratio * 100.0)),
                    ph.wall_secs,
                    ph.sync.count,
                    highlight(format!("{:.1}", ph.sync.total_ms.median_ms)),
                    ph.sync.total_ms.p95_ms, ph.sync.total_ms.max_ms,
                    ph.sync.decode_ms.median_ms, ph.sync.convert_ms.median_ms,
                    ph.lru_hits,
                    ph.releases, ph.refill_ms.median_ms, ph.refill_ms.max_ms,
                    jump,
                    ph.frame_ms.median_ms, ph.frame_ms.p99_ms, ph.frame_ms.max_ms,
                    ph.bg_decode_ms.count, ph.bg_decode_ms.median_ms,
                    opt_secs(ph.cpu_secs), ph.peak_rss_mb, ph.peak_gpu_mb, flag(ph.timed_out),
                ));
            }
        }
        out
    }

    /// Markdown with a header list and one table row per phase, shaped to
    /// paste into a devlog or FPS_BASELINES.md.
    pub fn to_markdown(&self) -> String {
        let h = &self.header;
        let mut out = format!(
            "# Benchmark: {} run {}/{}\n\n\
             - Date: {}\n\
             - Build: {} {} {} {}\n\
             - Machine: {}\n\
             - Folder: `{}` ({} images, {:.0} MB)\n\
             - Settings: cache_count {}, decode_threads {}, lru_budget_mb {}, gpu {}\n",
            h.folder_name(),
            h.run,
            h.runs,
            h.date_utc,
            h.version,
            h.git_hash,
            h.profile,
            h.platform,
            h.hostname,
            h.folder.display(),
            h.image_count,
            h.total_bytes as f64 / (1024.0 * 1024.0),
            h.settings.cache_count,
            h.settings.decode_threads,
            h.settings.lru_budget_mb,
            h.settings.gpu_memory_mode,
        );
        if let Some(label) = &h.label {
            out.push_str(&format!("- Label: {label}\n"));
        }
        if let Some(nav) = &self.nav {
            out.push_str(&format!(
                "\n## Keyboard navigation\n\n\
                 Settle: first image {}, window full {}{}.\n\n\
                 | Phase | Images | Rate | Stalls | Frame ms p50(median) / p99 / max | Decode ms n, p50(median) / p95 / max | CPU s | Peak RSS MB | Peak GPU MB |\n\
                 |---|---|---|---|---|---|---|---|---|\n",
                opt_ms(nav.settle.first_image_ms),
                opt_ms(nav.settle.settled_ms),
                flag(nav.settle.timed_out),
            ));
            for s in [&nav.skate_right, &nav.skate_left].into_iter().flatten() {
                out.push_str(&format!(
                    "| skate {}{} | {} (+{} prefetched) | {:.1} img/s | {}/{} ({:.0}%) | {:.1} / {:.1} / {:.1} | {}, {:.1} / {:.1} / {:.1} | {} | {:.0} | {:.0} |\n",
                    s.direction, flag(s.timed_out), s.images, s.skipped_images, s.images_per_sec,
                    s.stall_frames, s.frames, s.stall_share * 100.0,
                    s.frame_ms.median_ms, s.frame_ms.p99_ms, s.frame_ms.max_ms,
                    s.decode_ms.count, s.decode_ms.median_ms, s.decode_ms.p95_ms, s.decode_ms.max_ms,
                    opt_secs(s.cpu_secs), s.peak_rss_mb, s.peak_gpu_mb,
                ));
            }
            if let Some(t) = &nav.tap {
                out.push_str(&format!(
                    "| tap {:.0}/s{} | {} steps | press-to-image {:.1} / {:.1} / {:.1} ms (p50(median) / p95 / max) | {}/{} | {:.1} / {:.1} / {:.1} | {}, {:.1} / {:.1} / {:.1} | {} | {:.0} | {:.0} |\n",
                    t.rate_per_sec, flag(t.timed_out), t.steps,
                    t.step_latency_ms.median_ms, t.step_latency_ms.p95_ms, t.step_latency_ms.max_ms,
                    t.stall_frames, t.frames,
                    t.frame_ms.median_ms, t.frame_ms.p99_ms, t.frame_ms.max_ms,
                    t.decode_ms.count, t.decode_ms.median_ms, t.decode_ms.p95_ms, t.decode_ms.max_ms,
                    opt_secs(t.cpu_secs), t.peak_rss_mb, t.peak_gpu_mb,
                ));
            }
        }
        if let Some(sl) = &self.slider {
            out.push_str(&format!(
                "\n## Slider navigation\n\n\
                 Sweep {:.1} s, {} scrub anchors, {} jumps. Settle: first image {}, window full {}{}.\n\n\
                 | Phase | Positions | Shown | Sync block ms n, p50(median) / p95 / max | Sync p50 decode / convert ms | LRU hits | Refill ms p50 / max (releases) | Click-to-image ms p50 / p95 / max | Frame ms p50 / p99 / max | CPU s | Peak RSS MB | Peak GPU MB |\n\
                 |---|---|---|---|---|---|---|---|---|---|---|---|\n",
                sl.sweep_secs, sl.scrub_anchors, sl.jumps,
                opt_ms(sl.settle_first_image_ms), opt_ms(sl.settle_settled_ms), flag(sl.settle_timed_out),
            ));
            for ph in sl.phases() {
                let jump = ph.jump_ms.map_or("-".to_string(), |j| format!("{:.1} / {:.1} / {:.1}", j.median_ms, j.p95_ms, j.max_ms));
                out.push_str(&format!(
                    "| {}{} | {} | {} ({:.0}%) | {}, {:.1} / {:.1} / {:.1} | {:.1} / {:.1} | {} | {:.0} / {:.0} ({}) | {} | {:.1} / {:.1} / {:.1} | {} | {:.0} | {:.0} |\n",
                    ph.phase, flag(ph.timed_out), ph.positions, ph.images_shown, ph.display_ratio * 100.0,
                    ph.sync.count, ph.sync.total_ms.median_ms, ph.sync.total_ms.p95_ms, ph.sync.total_ms.max_ms,
                    ph.sync.decode_ms.median_ms, ph.sync.convert_ms.median_ms,
                    ph.lru_hits,
                    ph.refill_ms.median_ms, ph.refill_ms.max_ms, ph.releases,
                    jump,
                    ph.frame_ms.median_ms, ph.frame_ms.p99_ms, ph.frame_ms.max_ms,
                    opt_secs(ph.cpu_secs), ph.peak_rss_mb, ph.peak_gpu_mb,
                ));
            }
        }
        out
    }

    /// Write `<dir>/<yyyymmdd_HHMMSS>_<host>_<folder>_run<n>.json` and `.md`.
    /// Creates `dir` if needed. Returns the two paths.
    pub fn write(&self, dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
        std::fs::create_dir_all(dir)?;
        let h = &self.header;
        let stem = format!(
            "{}_{}_{}_run{}",
            chrono::Local::now().format("%Y%m%d_%H%M%S"),
            h.hostname,
            h.folder_name(),
            h.run,
        );
        let json_path = dir.join(format!("{stem}.json"));
        let md_path = dir.join(format!("{stem}.md"));
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::File::create(&json_path)?.write_all(json.as_bytes())?;
        std::fs::File::create(&md_path)?.write_all(self.to_markdown().as_bytes())?;
        Ok((json_path, md_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn sample() -> BenchReport {
        let skate = SkateReport {
            direction: "right".into(),
            images: 95,
            skipped_images: 5,
            wall_secs: 1.5,
            images_per_sec: 63.3,
            frames: 120,
            stall_frames: 25,
            stall_share: 25.0 / 120.0,
            frame_ms: LatencyStats::from_ms(&[16.0, 17.0, 33.0]),
            decode_ms: LatencyStats::from_ms(&[40.0, 45.0, 80.0]),
            cpu_secs: Some(2.5),
            peak_rss_mb: 800.0,
            peak_gpu_mb: 400.0,
            timed_out: false,
        };
        BenchReport {
            header: Header {
                date_utc: "2026-09-14 00:00:00 UTC".into(),
                version: "0.3.0".into(),
                git_hash: "abc1234".into(),
                profile: "opt-dev".into(),
                platform: "linux-x86_64".into(),
                hostname: "host".into(),
                folder: PathBuf::from("/data/4k_PNG_10MB"),
                image_count: 100,
                total_bytes: 1_048_576_000,
                label: Some("warm".into()),
                run: 1,
                runs: 2,
                settings: SettingsSnapshot {
                    cache_count: 5,
                    decode_threads: 10,
                    lru_budget_mb: 1024,
                    gpu_memory_mode: "Balanced".into(),
                    preview_budget_mb: 200,
                },
            },
            nav: Some(NavReport {
                images: 100,
                settle: SettleReport { first_image_ms: Some(350.0), settled_ms: Some(900.0), timed_out: false },
                skate_right: Some(skate.clone()),
                skate_left: Some(SkateReport { direction: "left".into(), ..skate }),
                tap: None,
            }),
            slider: None,
        }
    }

    #[test]
    fn json_round_trips_the_numbers() {
        let json = serde_json::to_string(&sample()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["header"]["image_count"], 100);
        assert_eq!(v["nav"]["skate_right"]["images"], 95);
        assert_eq!(v["nav"]["skate_left"]["direction"], "left");
        assert_eq!(v["nav"]["skate_right"]["frame_ms"]["p99_ms"], 33.0);
        assert!(v["nav"]["tap"].is_null());
    }

    #[test]
    fn markdown_has_one_row_per_phase() {
        let md = sample().to_markdown();
        assert!(md.starts_with("# Benchmark: 4k_PNG_10MB run 1/2"));
        assert!(md.contains("- Label: warm"));
        assert_eq!(md.matches("\n| skate ").count(), 2);
        assert!(!md.contains("| tap "));
    }

    #[test]
    fn write_creates_json_and_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let (json, md) = sample().write(dir.path()).unwrap();
        assert!(json.exists() && md.exists());
        assert!(json.file_name().unwrap().to_string_lossy().ends_with("_host_4k_PNG_10MB_run1.json"));
        let text = std::fs::read_to_string(&md).unwrap();
        assert!(text.contains("skate right"));
    }
}

/// Mean, min and max of one metric across the runs of one folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Spread {
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub n: usize,
}

impl Spread {
    pub fn of(values: impl IntoIterator<Item = f64>) -> Self {
        let mut s = Self { mean: 0.0, min: f64::INFINITY, max: f64::NEG_INFINITY, n: 0 };
        let mut sum = 0.0;
        for v in values {
            sum += v;
            s.min = s.min.min(v);
            s.max = s.max.max(v);
            s.n += 1;
        }
        if s.n == 0 {
            return Self::default();
        }
        s.mean = sum / s.n as f64;
        s
    }

    fn cell(&self, decimals: usize) -> String {
        if self.n <= 1 || self.min == self.max {
            format!("{:.*}", decimals, self.mean)
        } else {
            format!("{:.*} ({:.*} to {:.*})", decimals, self.mean, decimals, self.min, decimals, self.max)
        }
    }
}

/// One skate phase aggregated over runs.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct SkateSummary {
    pub direction: String,
    pub images: usize,
    pub images_per_sec: Spread,
    pub stall_share: Spread,
    pub frame_p50_ms: Spread,
    pub frame_p99_ms: Spread,
    pub decode_p50_ms: Spread,
    pub decode_p95_ms: Spread,
    pub cpu_secs: Spread,
    pub peak_rss_mb: Spread,
    pub timeouts: usize,
}

impl SkateSummary {
    fn of(runs: &[&SkateReport]) -> Self {
        Self {
            direction: runs[0].direction.clone(),
            images: runs[0].images,
            images_per_sec: Spread::of(runs.iter().map(|r| r.images_per_sec)),
            stall_share: Spread::of(runs.iter().map(|r| r.stall_share)),
            frame_p50_ms: Spread::of(runs.iter().map(|r| r.frame_ms.median_ms)),
            frame_p99_ms: Spread::of(runs.iter().map(|r| r.frame_ms.p99_ms)),
            decode_p50_ms: Spread::of(runs.iter().map(|r| r.decode_ms.median_ms)),
            decode_p95_ms: Spread::of(runs.iter().map(|r| r.decode_ms.p95_ms)),
            cpu_secs: Spread::of(runs.iter().filter_map(|r| r.cpu_secs)),
            peak_rss_mb: Spread::of(runs.iter().map(|r| r.peak_rss_mb)),
            timeouts: runs.iter().filter(|r| r.timed_out).count(),
        }
    }
}

/// The tap phase aggregated over runs.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapSummary {
    pub rate_per_sec: f64,
    pub steps: usize,
    pub latency_p50_ms: Spread,
    pub latency_p95_ms: Spread,
    pub latency_max_ms: Spread,
    pub frame_p99_ms: Spread,
    pub decode_p50_ms: Spread,
    pub cpu_secs: Spread,
    pub timeouts: usize,
}

impl TapSummary {
    fn of(runs: &[&TapReport]) -> Self {
        Self {
            rate_per_sec: runs[0].rate_per_sec,
            steps: runs[0].steps,
            latency_p50_ms: Spread::of(runs.iter().map(|r| r.step_latency_ms.median_ms)),
            latency_p95_ms: Spread::of(runs.iter().map(|r| r.step_latency_ms.p95_ms)),
            latency_max_ms: Spread::of(runs.iter().map(|r| r.step_latency_ms.max_ms)),
            frame_p99_ms: Spread::of(runs.iter().map(|r| r.frame_ms.p99_ms)),
            decode_p50_ms: Spread::of(runs.iter().map(|r| r.decode_ms.median_ms)),
            cpu_secs: Spread::of(runs.iter().filter_map(|r| r.cpu_secs)),
            timeouts: runs.iter().filter(|r| r.timed_out).count(),
        }
    }
}

/// One slider phase aggregated over runs.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct SliderPhaseSummary {
    pub phase: String,
    pub positions: usize,
    pub display_ratio: Spread,
    pub sync_block_p50_ms: Spread,
    pub sync_block_p95_ms: Spread,
    pub lru_hits: Spread,
    pub refill_p50_ms: Spread,
    pub jump_p50_ms: Option<Spread>,
    pub jump_p95_ms: Option<Spread>,
    pub frame_p99_ms: Spread,
    pub cpu_secs: Spread,
    pub timeouts: usize,
}

impl SliderPhaseSummary {
    fn of(runs: &[&SliderPhaseReport]) -> Self {
        let has_jump = runs.iter().any(|r| r.jump_ms.is_some());
        Self {
            phase: runs[0].phase.clone(),
            positions: runs[0].positions,
            display_ratio: Spread::of(runs.iter().map(|r| r.display_ratio)),
            sync_block_p50_ms: Spread::of(runs.iter().map(|r| r.sync.total_ms.median_ms)),
            sync_block_p95_ms: Spread::of(runs.iter().map(|r| r.sync.total_ms.p95_ms)),
            lru_hits: Spread::of(runs.iter().map(|r| r.lru_hits as f64)),
            refill_p50_ms: Spread::of(runs.iter().map(|r| r.refill_ms.median_ms)),
            jump_p50_ms: has_jump.then(|| Spread::of(runs.iter().filter_map(|r| r.jump_ms.map(|j| j.median_ms)))),
            jump_p95_ms: has_jump.then(|| Spread::of(runs.iter().filter_map(|r| r.jump_ms.map(|j| j.p95_ms)))),
            frame_p99_ms: Spread::of(runs.iter().map(|r| r.frame_ms.p99_ms)),
            cpu_secs: Spread::of(runs.iter().filter_map(|r| r.cpu_secs)),
            timeouts: runs.iter().filter(|r| r.timed_out).count(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FolderSummary {
    pub folder: PathBuf,
    pub image_count: usize,
    pub runs: usize,
    pub settle_first_image_ms: Spread,
    pub settle_settled_ms: Spread,
    pub skate_right: Option<SkateSummary>,
    pub skate_left: Option<SkateSummary>,
    pub tap: Option<TapSummary>,
    pub slider: Vec<SliderPhaseSummary>,
}

/// All runs of one invocation, averaged per folder. Written once at the
/// end as `<stamp>_<host>_summary.json` and `.md`.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Summary {
    pub date_utc: String,
    pub version: String,
    pub git_hash: String,
    pub profile: String,
    pub platform: String,
    pub hostname: String,
    pub label: Option<String>,
    pub settings: SettingsSnapshot,
    pub folders: Vec<FolderSummary>,
}

impl Summary {
    /// Groups `reports` by folder in first-seen order. Empty input gives an
    /// empty summary.
    pub fn of(reports: &[BenchReport]) -> Option<Self> {
        let first = reports.first()?;
        let mut folders: Vec<FolderSummary> = Vec::new();
        let mut order: Vec<PathBuf> = Vec::new();
        for r in reports {
            if !order.contains(&r.header.folder) {
                order.push(r.header.folder.clone());
            }
        }
        for folder in order {
            let runs: Vec<&BenchReport> = reports.iter().filter(|r| r.header.folder == folder).collect();
            let navs: Vec<&NavReport> = runs.iter().filter_map(|r| r.nav.as_ref()).collect();
            let rights: Vec<&SkateReport> = navs.iter().filter_map(|n| n.skate_right.as_ref()).collect();
            let lefts: Vec<&SkateReport> = navs.iter().filter_map(|n| n.skate_left.as_ref()).collect();
            let taps: Vec<&TapReport> = navs.iter().filter_map(|n| n.tap.as_ref()).collect();
            let sliders: Vec<&SliderReport> = runs.iter().filter_map(|r| r.slider.as_ref()).collect();
            let slider_phase = |pick: fn(&SliderReport) -> Option<&SliderPhaseReport>| {
                let phases: Vec<&SliderPhaseReport> = sliders.iter().filter_map(|s| pick(s)).collect();
                (!phases.is_empty()).then(|| SliderPhaseSummary::of(&phases))
            };
            folders.push(FolderSummary {
                folder: folder.clone(),
                image_count: runs[0].header.image_count,
                runs: runs.len(),
                settle_first_image_ms: Spread::of(navs.iter().filter_map(|n| n.settle.first_image_ms)),
                settle_settled_ms: Spread::of(navs.iter().filter_map(|n| n.settle.settled_ms)),
                skate_right: (!rights.is_empty()).then(|| SkateSummary::of(&rights)),
                skate_left: (!lefts.is_empty()).then(|| SkateSummary::of(&lefts)),
                tap: (!taps.is_empty()).then(|| TapSummary::of(&taps)),
                slider: [
                    slider_phase(|s| s.sweep.as_ref()),
                    slider_phase(|s| s.scrub.as_ref()),
                    slider_phase(|s| s.jump.as_ref()),
                ]
                .into_iter()
                .flatten()
                .collect(),
            });
        }
        let h = &first.header;
        Some(Self {
            date_utc: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            version: h.version.clone(),
            git_hash: h.git_hash.clone(),
            profile: h.profile.clone(),
            platform: h.platform.clone(),
            hostname: h.hostname.clone(),
            label: h.label.clone(),
            settings: h.settings.clone(),
            folders,
        })
    }

    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "# Benchmark summary\n\n- Date: {}\n- Build: {} {} {} {}\n- Machine: {}\n- Settings: cache_count {}, decode_threads {}, lru_budget_mb {}, gpu {}\n",
            self.date_utc, self.version, self.git_hash, self.profile, self.platform, self.hostname,
            self.settings.cache_count, self.settings.decode_threads, self.settings.lru_budget_mb,
            self.settings.gpu_memory_mode,
        );
        if let Some(label) = &self.label {
            out.push_str(&format!("- Label: {label}\n"));
        }
        out.push_str(
            "\nValues are the mean over runs, with the min to max range in brackets when there is more than one run. p50 means median.\n\n## Keyboard navigation, skate\n\n| Folder | Runs | Pass | Images | img/s | Stall % | Frame p50 ms | Frame p99 ms | Decode p50 ms | Decode p95 ms | CPU s | Peak RSS MB |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
        );
        for f in &self.folders {
            let name = f.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            for s in [&f.skate_right, &f.skate_left].into_iter().flatten() {
                let timeouts = if s.timeouts > 0 { format!(" ({} timed out)", s.timeouts) } else { String::new() };
                out.push_str(&format!(
                    "| {} | {} | {}{} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                    name, f.runs, s.direction, timeouts, s.images,
                    s.images_per_sec.cell(1),
                    Spread { mean: s.stall_share.mean * 100.0, min: s.stall_share.min * 100.0, max: s.stall_share.max * 100.0, n: s.stall_share.n }.cell(0),
                    s.frame_p50_ms.cell(1), s.frame_p99_ms.cell(1),
                    s.decode_p50_ms.cell(1), s.decode_p95_ms.cell(1),
                    s.cpu_secs.cell(2), s.peak_rss_mb.cell(0),
                ));
            }
        }
        out.push_str(
            "\n## Keyboard navigation, tap\n\n| Folder | Runs | Rate | Steps | Press-to-image p50 ms | p95 ms | max ms | Frame p99 ms | Decode p50 ms | CPU s |\n|---|---|---|---|---|---|---|---|---|---|\n",
        );
        for f in &self.folders {
            let name = f.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some(t) = &f.tap {
                let timeouts = if t.timeouts > 0 { format!(" ({} timed out)", t.timeouts) } else { String::new() };
                out.push_str(&format!(
                    "| {} | {} | {:.0}/s{} | {} | {} | {} | {} | {} | {} | {} |\n",
                    name, f.runs, t.rate_per_sec, timeouts, t.steps,
                    t.latency_p50_ms.cell(1), t.latency_p95_ms.cell(1), t.latency_max_ms.cell(1),
                    t.frame_p99_ms.cell(1), t.decode_p50_ms.cell(1), t.cpu_secs.cell(2),
                ));
            }
        }
        if self.folders.iter().any(|f| !f.slider.is_empty()) {
            out.push_str(
                "\n## Slider navigation\n\n\
                 | Folder | Runs | Phase | Positions | Shown % | Sync block p50 ms | Sync block p95 ms | LRU hits | Refill p50 ms | Click-to-image p50 ms | p95 ms | Frame p99 ms | CPU s |\n\
                 |---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
            );
            for f in &self.folders {
                let name = f.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                for ph in &f.slider {
                    let timeouts = if ph.timeouts > 0 { format!(" ({} timed out)", ph.timeouts) } else { String::new() };
                    let pct = Spread { mean: ph.display_ratio.mean * 100.0, min: ph.display_ratio.min * 100.0, max: ph.display_ratio.max * 100.0, n: ph.display_ratio.n };
                    out.push_str(&format!(
                        "| {} | {} | {}{} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                        name, f.runs, ph.phase, timeouts, ph.positions,
                        pct.cell(0), ph.sync_block_p50_ms.cell(1), ph.sync_block_p95_ms.cell(1),
                        ph.lru_hits.cell(0), ph.refill_p50_ms.cell(0),
                        ph.jump_p50_ms.map_or("-".to_string(), |s| s.cell(1)),
                        ph.jump_p95_ms.map_or("-".to_string(), |s| s.cell(1)),
                        ph.frame_p99_ms.cell(1), ph.cpu_secs.cell(2),
                    ));
                }
            }
        }
        out.push_str("\n## Settle\n\n| Folder | Runs | First image ms | Window full ms |\n|---|---|---|---|\n");
        for f in &self.folders {
            let name = f.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                name, f.runs, f.settle_first_image_ms.cell(0), f.settle_settled_ms.cell(0),
            ));
        }
        out
    }

    pub fn write(&self, dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
        std::fs::create_dir_all(dir)?;
        let stem = format!(
            "{}_{}_summary",
            chrono::Local::now().format("%Y%m%d_%H%M%S"),
            self.hostname,
        );
        let json_path = dir.join(format!("{stem}.json"));
        let md_path = dir.join(format!("{stem}.md"));
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::File::create(&json_path)?.write_all(json.as_bytes())?;
        std::fs::File::create(&md_path)?.write_all(self.to_markdown().as_bytes())?;
        Ok((json_path, md_path))
    }
}

#[cfg(test)]
mod summary_tests {
    use super::tests::sample;
    use super::*;

    #[test]
    fn spread_is_mean_min_max() {
        let s = Spread::of([10.0, 20.0, 60.0]);
        assert_eq!((s.mean, s.min, s.max, s.n), (30.0, 10.0, 60.0, 3));
        assert_eq!(Spread::of([]), Spread::default());
    }

    #[test]
    fn summary_groups_runs_by_folder_in_order() {
        let mut a1 = sample();
        let mut a2 = sample();
        a2.header.run = 2;
        if let Some(n) = a2.nav.as_mut() {
            n.skate_right.as_mut().unwrap().images_per_sec = 43.3;
        }
        let mut b = sample();
        b.header.folder = PathBuf::from("/data/small_images");
        let summary = Summary::of(&[a1.clone(), b, a2]).unwrap();
        assert_eq!(summary.folders.len(), 2);
        assert_eq!(summary.folders[0].runs, 2);
        assert_eq!(summary.folders[1].runs, 1);
        let right = summary.folders[0].skate_right.as_ref().unwrap();
        assert!((right.images_per_sec.mean - 53.3).abs() < 1e-9);
        assert_eq!(right.images_per_sec.min, 43.3);
        assert_eq!(right.images_per_sec.max, 63.3);
        a1.nav = None;
        assert!(Summary::of(&[]).is_none());
    }

    #[test]
    fn summary_aggregates_slider_phases() {
        let phase = |p50: f64| SliderPhaseReport {
            phase: "jump".into(),
            wall_secs: 3.0,
            positions: 20,
            images_shown: 20,
            display_ratio: 1.0,
            sync: SyncStats { count: 20, total_ms: LatencyStats::from_ms(&[p50]), ..Default::default() },
            lru_hits: 0,
            releases: 20,
            refill_ms: LatencyStats::from_ms(&[300.0]),
            jump_ms: Some(LatencyStats::from_ms(&[p50])),
            frame_ms: LatencyStats::from_ms(&[7.0]),
            bg_decode_ms: LatencyStats::default(),
            cpu_secs: Some(1.0),
            peak_rss_mb: 500.0,
            peak_gpu_mb: 200.0,
            timed_out: false,
        };
        let mut a = sample();
        a.slider = Some(SliderReport {
            images: 100, sweep_secs: 4.0, scrub_anchors: 0, jumps: 20,
            settle_first_image_ms: None, settle_settled_ms: None, settle_timed_out: false,
            sweep: None, scrub: None, jump: Some(phase(80.0)),
        });
        let mut b = a.clone();
        b.header.run = 2;
        b.slider.as_mut().unwrap().jump.as_mut().unwrap().jump_ms = Some(LatencyStats::from_ms(&[100.0]));
        let summary = Summary::of(&[a, b]).unwrap();
        let sl = &summary.folders[0].slider;
        assert_eq!(sl.len(), 1);
        assert_eq!(sl[0].phase, "jump");
        assert_eq!(sl[0].jump_p50_ms.unwrap().mean, 90.0);
        let md = summary.to_markdown();
        assert!(md.contains("| 4k_PNG_10MB | 2 | jump | 20 | 100 | 80.0 |"), "{md}");
        assert!(md.contains("| 90.0 (80.0 to 100.0) |"), "{md}");
    }

    #[test]
    fn summary_markdown_shows_ranges_only_for_repeated_runs() {
        let mut a2 = sample();
        a2.header.run = 2;
        a2.nav.as_mut().unwrap().skate_right.as_mut().unwrap().images_per_sec = 43.3;
        let md = Summary::of(&[sample(), a2]).unwrap().to_markdown();
        assert!(md.contains("| 4k_PNG_10MB | 2 | right | 95 | 53.3 (43.3 to 63.3) |"), "{md}");
        let single = Summary::of(&[sample()]).unwrap().to_markdown();
        assert!(single.contains("| 4k_PNG_10MB | 1 | right | 95 | 63.3 |"), "{single}");
    }
}
