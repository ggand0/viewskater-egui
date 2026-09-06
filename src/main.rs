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
                args.bench_preview,
            )))
        }),
    )
}
