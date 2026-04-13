#![windows_subsystem = "windows"]

use std::path::PathBuf;

use clap::Parser;
use eframe::{egui, egui_wgpu, wgpu};

use crate::settings::{AppSettings, GpuMemoryMode};

mod about;
mod app;
mod build_info;
mod cache;
mod decode;
mod file_io;
mod menu;
mod pane;
mod perf;
mod settings;
mod theme;

#[derive(Parser)]
#[command(name = "viewskater-egui", about = "Fast image viewer")]
struct Args {
    /// Paths to image files or directories
    paths: Vec<PathBuf>,

    /// Rendering backend: "glow" (OpenGL, default) or "wgpu"
    #[arg(long, default_value = "glow")]
    renderer: String,
}

/// Create a wgpu Instance/Adapter/Device/Queue with the user-selected
/// MemoryHints. The hint controls gpu_allocator block sizes:
///
/// - Performance: ~256 MB blocks (wgpu default). Largest memory footprint,
///   fastest texture allocation.
/// - Balanced: 64 MB device / 32 MB host blocks via Manual hint. Fits two
///   4K textures per block via sub-allocation. Recommended default.
/// - LowMemory: 8 MB device / 4 MB host blocks. A 4K RGBA texture (31.6 MB)
///   exceeds the block size, forcing dedicated allocations per texture and
///   degrading keyboard navigation performance.
fn build_wgpu_setup(mode: GpuMemoryMode) -> egui_wgpu::WgpuSetupExisting {
    const MB: u64 = 1024 * 1024;
    let memory_hints = match mode {
        GpuMemoryMode::Performance => wgpu::MemoryHints::Performance,
        GpuMemoryMode::Balanced => wgpu::MemoryHints::Manual {
            suballocated_device_memory_block_size: (64 * MB)..(128 * MB),
        },
        GpuMemoryMode::LowMemory => wgpu::MemoryHints::MemoryUsage,
    };

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env()
            .unwrap_or(wgpu::Backends::PRIMARY | wgpu::Backends::GL),
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        backend_options: wgpu::BackendOptions::from_env_or_default(),
    });

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("Failed to find a wgpu adapter");

    let base_limits = if adapter.get_info().backend == wgpu::Backend::Gl {
        wgpu::Limits::downlevel_webgl2_defaults()
    } else {
        wgpu::Limits::default()
    };

    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("viewskater wgpu device"),
            required_features: wgpu::Features::default(),
            required_limits: wgpu::Limits {
                max_texture_dimension_2d: 8192,
                ..base_limits
            },
            memory_hints,
        },
        None,
    ))
    .expect("Failed to create wgpu device");

    egui_wgpu::WgpuSetupExisting {
        instance,
        adapter,
        device,
        queue,
    }
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

    let renderer = match args.renderer.as_str() {
        "wgpu" => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    };

    // Build the wgpu setup using the user-selected memory mode from settings.
    // The wgpu device is created once at startup and cannot be reconfigured
    // at runtime, so changes to gpu_memory_mode only take effect on next launch.
    let settings = AppSettings::load();
    let wgpu_setup = build_wgpu_setup(settings.gpu_memory_mode);

    let wgpu_options = egui_wgpu::WgpuConfiguration {
        desired_maximum_frame_latency: Some(1),
        wgpu_setup: egui_wgpu::WgpuSetup::Existing(wgpu_setup),
        ..Default::default()
    };

    let options = eframe::NativeOptions {
        viewport,
        renderer,
        dithering: false,
        wgpu_options,
        ..Default::default()
    };

    eframe::run_native(
        "viewskater-egui",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, args.paths, log_buffer)))),
    )
}
