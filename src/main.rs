use std::path::PathBuf;

use clap::Parser;
use eframe::egui;

mod app;
mod build_info;
mod cache;
mod decode;
mod file_io;
mod menu;
mod pane;
mod perf;

#[derive(Parser)]
#[command(name = "viewskater-egui", about = "Fast image viewer")]
struct Args {
    /// Paths to image files or directories
    paths: Vec<PathBuf>,
}

fn main() -> eframe::Result {
    env_logger::init();
    let args = Args::parse();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "viewskater-egui",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, args.paths)))),
    )
}
