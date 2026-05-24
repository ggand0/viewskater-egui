use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, Once};

use image::{DynamicImage, ImageReader, ImageResult};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

const APP_NAME: &str = "viewskater-egui";

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jxl", "png", "bmp", "webp", "gif", "tiff", "tif", "qoi", "tga",
];

static REGISTER_IMAGE_DECODERS: Once = Once::new();

pub fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| SUPPORTED_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

pub fn enumerate_images(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::warn!("Failed to read directory: {}", dir.display());
        return Vec::new();
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let not_hidden = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('.'));
            not_hidden && is_supported_image(p)
        })
        .collect();

    paths.sort_by(|a, b| {
        natord::compare(
            &a.file_name().unwrap_or_default().to_string_lossy(),
            &b.file_name().unwrap_or_default().to_string_lossy(),
        )
    });

    log::info!("Found {} images in {}", paths.len(), dir.display());
    paths
}

fn ensure_image_decoders_registered() {
    REGISTER_IMAGE_DECODERS.call_once(|| {
        jxl_oxide::integration::register_image_decoding_hook();
    });
}

/// Convenience wrapper around ImageReader::open().with_guessed_format().decode()
pub fn open_image(path: &Path) -> ImageResult<DynamicImage> {
    ensure_image_decoders_registered();
    ImageReader::open(path)?.with_guessed_format()?.decode()
}

/// Resolve a CLI path to a directory and an optional target filename.
/// If path is a file, returns its parent directory and the filename.
/// If path is a directory, returns it directly.
pub fn resolve_path(path: &Path) -> (PathBuf, Option<String>) {
    if path.is_file() {
        let dir = path.parent().unwrap_or(path).to_path_buf();
        let filename = path.file_name().map(|f| f.to_string_lossy().into_owned());
        (dir, filename)
    } else {
        (path.to_path_buf(), None)
    }
}

// --- Logging ---

const MAX_LOG_LINES: usize = 1000;

/// A tracing Layer that captures log events into an in-memory circular buffer.
struct BufferLayer {
    buffer: Arc<Mutex<VecDeque<String>>>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for BufferLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        // For tracing-log bridged events, metadata.target() is "log";
        // the real target is in the "log.target" field.
        let target = visitor.log_target.as_deref().unwrap_or(metadata.target());
        if !target.starts_with("viewskater_egui") {
            return;
        }

        let message = format!("{:<5} {}", metadata.level(), visitor.message);

        let mut buf = self.buffer.lock().unwrap();
        if buf.len() == MAX_LOG_LINES {
            buf.pop_front();
        }
        buf.push_back(message);
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    log_target: Option<String>,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "log.target" => self.log_target = Some(value.to_string()),
            _ => {}
        }
    }
}

pub fn setup_logger() -> Arc<Mutex<VecDeque<String>>> {
    let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG_LINES)));

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("viewskater_egui=info"));

    let buffer_layer = BufferLayer {
        buffer: buffer.clone(),
    };

    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(env_filter))
        .with(buffer_layer)
        .init();

    buffer
}

pub fn get_log_directory() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_NAME)
        .join("logs")
}

pub fn setup_panic_hook(log_buffer: Arc<Mutex<VecDeque<String>>>) {
    let log_file_path = get_log_directory().join("panic.log");
    std::fs::create_dir_all(log_file_path.parent().unwrap())
        .expect("Failed to create log directory");

    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let Ok(mut file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_file_path)
        else {
            eprintln!("Failed to open panic log file: {}", log_file_path.display());
            return;
        };

        let _ = writeln!(file, "Panic occurred: {}", info);
        let _ = writeln!(file, "Backtrace:\n{}\n", backtrace);
        let _ = writeln!(file, "Last {} log entries:\n", MAX_LOG_LINES);

        if let Ok(buffer) = log_buffer.lock() {
            for entry in buffer.iter() {
                let _ = writeln!(file, "{}", entry);
            }
        }
    }));
}

/// Dumps the in-memory log buffer to `debug.log` and opens the log directory.
pub fn export_and_open_debug_logs(log_buffer: &Arc<Mutex<VecDeque<String>>>) {
    let log_dir = get_log_directory();
    if std::fs::create_dir_all(&log_dir).is_err() {
        log::error!("Failed to create log directory: {}", log_dir.display());
        return;
    }

    let debug_log_path = log_dir.join("debug.log");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&debug_log_path);

    match file {
        Ok(mut file) => {
            let buffer = log_buffer.lock().unwrap();
            for entry in buffer.iter() {
                let _ = writeln!(file, "{}", entry);
            }
            let _ = file.flush();
            // Drop lock before logging to avoid deadlock (log call re-enters BufferLayer)
            drop(buffer);
            log::info!("Debug logs exported to: {}", debug_log_path.display());
        }
        Err(e) => {
            log::error!("Failed to export debug logs: {}", e);
            return;
        }
    }

    open_in_file_explorer(&log_dir.to_string_lossy());
}

pub fn open_in_file_explorer(path: &str) {
    if cfg!(target_os = "windows") {
        let _ = Command::new("explorer").arg(path).spawn();
    } else if cfg!(target_os = "macos") {
        let _ = Command::new("open").arg(path).spawn();
    } else if cfg!(target_os = "linux") {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}
