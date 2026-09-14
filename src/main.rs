#![windows_subsystem = "windows"]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;

use clap::Parser;
use eframe::{egui, egui_wgpu, wgpu};

use crate::settings::{AppSettings, GpuMemoryMode};

mod about;
mod animation;
mod app;
mod bench;
mod build_info;
mod cache;
mod decode;
mod file_io;
mod menu;
mod pane;
mod perf;
mod platform;
mod settings;
mod theme;
mod trash_bin;
mod view_animation;

#[derive(Parser)]
#[command(name = "viewskater-egui", about = "Fast image viewer")]
struct Args {
    /// Paths to image files or directories
    paths: Vec<PathBuf>,

    /// Run the slider preview benchmark on the given folder and exit.
    /// Simulates hovering the navigation slider and reports thumbnail
    /// latency stats to the log.
    #[arg(long)]
    bench_preview: bool,

    /// Run the keyboard navigation benchmark on the given folder and exit:
    /// skate to the end and back, then tap through at a human pace.
    /// Combines with --bench-preview (nav runs first).
    #[arg(long)]
    bench_nav: bool,

    /// Run the slider navigation benchmark on the given folder and exit:
    /// a sweep across the rail, scrubs around scattered positions, and
    /// clicks. Combines with --bench-nav (nav runs first in each run).
    #[arg(long)]
    bench_slider: bool,

    /// Seconds the --bench-slider sweep takes to cross the rail.
    #[arg(long, default_value_t = 4.0, value_name = "SECS")]
    bench_sweep_secs: f64,

    /// Number of scrub gestures in --bench-slider (0 skips the phase).
    #[arg(long, default_value_t = 10, value_name = "N")]
    bench_scrub_anchors: usize,

    /// Number of click-to-jump gestures in --bench-slider (0 skips).
    #[arg(long, default_value_t = 20, value_name = "N")]
    bench_jumps: usize,

    /// Folder to benchmark. Repeat the flag to run several folders in one
    /// go; the positional path is then ignored. A summary averaged over
    /// all runs is written at the end.
    #[arg(long, value_name = "DIR")]
    bench_dir: Vec<PathBuf>,

    /// Only skate through the first N images of the folder (and back).
    /// Default is the whole folder.
    #[arg(long, value_name = "N")]
    bench_max_images: Option<usize>,

    /// Steps per second in the tap phase of --bench-nav.
    #[arg(long, default_value_t = 6.0, value_name = "PER_SEC")]
    bench_tap_rate: f64,

    /// Add a tap phase to --bench-nav: N single steps at --bench-tap-rate,
    /// measuring press-to-image latency. Off by default; useful for slow
    /// sources (RAW, JPEG 2000, network shares) where a decode can take
    /// longer than the gap between taps.
    #[arg(long, default_value_t = bench::nav::DEFAULT_TAP_STEPS, value_name = "N")]
    bench_tap_steps: usize,

    /// Repeat the benchmarks this many times in one process, reopening the
    /// folder between runs.
    #[arg(long, default_value_t = 1, value_name = "N")]
    bench_runs: usize,

    /// Write a JSON and a markdown report per run into this directory.
    #[arg(long, value_name = "DIR")]
    bench_out: Option<PathBuf>,

    /// Free text copied into the report header, e.g. "cold" or "warm".
    #[arg(long, value_name = "TEXT")]
    bench_label: Option<String>,
}

/// Configure eframe's wgpu setup with the user-selected MemoryHints. The hint
/// controls gpu_allocator block sizes:
///
/// - Performance: ~256 MB blocks (wgpu default). Largest memory footprint,
///   fastest texture allocation.
/// - Balanced: 64 MB device / 32 MB host blocks via Manual hint. Fits two
///   4K textures per block via sub-allocation. Recommended default.
/// - LowMemory: 8 MB device / 4 MB host blocks. A 4K RGBA texture (31.6 MB)
///   exceeds the block size, forcing dedicated allocations per texture and
///   degrading keyboard navigation performance.
fn build_wgpu_setup(mode: GpuMemoryMode) -> egui_wgpu::WgpuSetup {
    const MB: u64 = 1024 * 1024;
    let memory_hints = match mode {
        GpuMemoryMode::Performance => wgpu::MemoryHints::Performance,
        GpuMemoryMode::Balanced => wgpu::MemoryHints::Manual {
            suballocated_device_memory_block_size: (64 * MB)..(128 * MB),
        },
        GpuMemoryMode::LowMemory => wgpu::MemoryHints::MemoryUsage,
    };

    egui_wgpu::WgpuSetupCreateNew {
        device_descriptor: Arc::new(move |adapter| {
            let base_limits = if adapter.get_info().backend == wgpu::Backend::Gl {
                wgpu::Limits::downlevel_webgl2_defaults()
            } else {
                wgpu::Limits::default()
            };

            wgpu::DeviceDescriptor {
                label: Some("viewskater wgpu device"),
                required_features: wgpu::Features::default(),
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: 8192,
                    ..base_limits
                },
                memory_hints: memory_hints.clone(),
            }
        }),
        ..Default::default()
    }
    .into()
}

fn load_icon() -> Option<egui::IconData> {
    static ICON: &[u8] = include_bytes!("../assets/icon_256.png");
    let img = image::load_from_memory(ICON).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

fn main() -> eframe::Result {
    let app_start = std::time::Instant::now();
    let log_buffer = file_io::setup_logger();
    file_io::setup_panic_hook(log_buffer.clone());
    let args = Args::parse();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 720.0])
        .with_drag_and_drop(true)
        .with_app_id("viewskater-egui");

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    // Build the wgpu setup using the user-selected memory mode from settings.
    // The wgpu device is created once at startup and cannot be reconfigured
    // at runtime, so changes to gpu_memory_mode only take effect on next launch.
    let settings = AppSettings::load();
    let wgpu_setup = build_wgpu_setup(settings.gpu_memory_mode);

    let wgpu_options = egui_wgpu::WgpuConfiguration {
        desired_maximum_frame_latency: Some(1),
        wgpu_setup,
        on_surface_error: std::sync::Arc::new(|err| match err {
            wgpu::SurfaceError::Outdated => {
                tracing::warn!("wgpu: surface Outdated, recreating");
                egui_wgpu::SurfaceErrorAction::RecreateSurface
            }
            other => {
                tracing::warn!("wgpu: surface error: {other}, skipping frame");
                egui_wgpu::SurfaceErrorAction::SkipFrame
            }
        }),
        ..Default::default()
    };

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        dithering: false,
        wgpu_options,
        ..Default::default()
    };

    let (file_tx, file_rx) = mpsc::channel::<PathBuf>();
    #[cfg(target_os = "macos")]
    {
        platform::macos::set_file_channel(file_tx.clone());
        // Must run before eframe::run_native so the observer is registered
        // before AppKit starts -finishLaunching and dispatches the initial
        // openFiles: event.
        platform::macos::install_launch_observer();
    }
    // Silence unused warnings on non-macOS targets.
    #[cfg(not(target_os = "macos"))]
    let _ = file_tx;

    eframe::run_native(
        "viewskater-egui",
        options,
        Box::new(move |cc| {
            #[cfg(target_os = "macos")]
            platform::macos::register_file_handler();
            Ok(Box::new(app::App::new(
                cc,
                args.paths,
                log_buffer,
                settings,
                file_rx,
                bench::BenchOptions {
                    nav: args.bench_nav,
                    slider: args.bench_slider,
                    preview: args.bench_preview,
                    dirs: args.bench_dir,
                    max_images: args.bench_max_images,
                    tap_rate: args.bench_tap_rate,
                    tap_steps: args.bench_tap_steps,
                    sweep_secs: args.bench_sweep_secs,
                    scrub_anchors: args.bench_scrub_anchors,
                    jumps: args.bench_jumps,
                    runs: args.bench_runs.max(1),
                    out_dir: args.bench_out,
                    label: args.bench_label,
                },
                app_start,
            )))
        }),
    )
}
