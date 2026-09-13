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
    /// From process start to the first frame that showed an image.
    pub first_image_ms: Option<f64>,
    /// From process start to the first frame with a full sliding window.
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

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BenchReport {
    pub header: Header,
    pub nav: Option<NavReport>,
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
                "settle: first image {}, window full {}{}\n",
                opt_ms(nav.settle.first_image_ms),
                opt_ms(nav.settle.settled_ms),
                flag(nav.settle.timed_out),
            ));
            for s in [&nav.skate_right, &nav.skate_left].into_iter().flatten() {
                out.push_str(&format!(
                    "skate {}: {} images in {:.2}s = {:.1} img/s (+{} prefetched), stalls {}/{} frames ({:.0}%), \
                     frame p50={:.1} p99={:.1} max={:.1} ms, decode n={} p50={:.1} p95={:.1} max={:.1} ms, \
                     cpu {}s, peak rss {:.0} MB gpu {:.0} MB{}\n",
                    s.direction, s.images, s.wall_secs, s.images_per_sec, s.skipped_images,
                    s.stall_frames, s.frames, s.stall_share * 100.0,
                    s.frame_ms.median_ms, s.frame_ms.p99_ms, s.frame_ms.max_ms,
                    s.decode_ms.count, s.decode_ms.median_ms, s.decode_ms.p95_ms, s.decode_ms.max_ms,
                    opt_secs(s.cpu_secs), s.peak_rss_mb, s.peak_gpu_mb, flag(s.timed_out),
                ));
            }
            if let Some(t) = &nav.tap {
                out.push_str(&format!(
                    "tap {:.1}/s: {} steps in {:.2}s, press-to-image p50={:.1} p95={:.1} max={:.1} ms, \
                     stalls {}/{} frames, frame p50={:.1} p99={:.1} max={:.1} ms, decode n={} p50={:.1} p95={:.1} max={:.1} ms, \
                     cpu {}s, peak rss {:.0} MB gpu {:.0} MB{}\n",
                    t.rate_per_sec, t.steps, t.wall_secs,
                    t.step_latency_ms.median_ms, t.step_latency_ms.p95_ms, t.step_latency_ms.max_ms,
                    t.stall_frames, t.frames,
                    t.frame_ms.median_ms, t.frame_ms.p99_ms, t.frame_ms.max_ms,
                    t.decode_ms.count, t.decode_ms.median_ms, t.decode_ms.p95_ms, t.decode_ms.max_ms,
                    opt_secs(t.cpu_secs), t.peak_rss_mb, t.peak_gpu_mb, flag(t.timed_out),
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
                 | Phase | Images | Rate | Stalls | Frame ms p50 / p99 / max | Decode ms n, p50 / p95 / max | CPU s | Peak RSS MB | Peak GPU MB |\n\
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
                    "| tap {:.0}/s{} | {} steps | press-to-image {:.1} / {:.1} / {:.1} ms (p50 / p95 / max) | {}/{} | {:.1} / {:.1} / {:.1} | {}, {:.1} / {:.1} / {:.1} | {} | {:.0} | {:.0} |\n",
                    t.rate_per_sec, flag(t.timed_out), t.steps,
                    t.step_latency_ms.median_ms, t.step_latency_ms.p95_ms, t.step_latency_ms.max_ms,
                    t.stall_frames, t.frames,
                    t.frame_ms.median_ms, t.frame_ms.p99_ms, t.frame_ms.max_ms,
                    t.decode_ms.count, t.decode_ms.median_ms, t.decode_ms.p95_ms, t.decode_ms.max_ms,
                    opt_secs(t.cpu_secs), t.peak_rss_mb, t.peak_gpu_mb,
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

    fn sample() -> BenchReport {
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
