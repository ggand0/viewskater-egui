use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fs::{DirEntry, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, Once};
use std::time::SystemTime;

use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader, ImageResult};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use crate::metadata::{self, ExifData, MetadataRecord};
use crate::settings::{ImageDiscoveryOptions, ImageSortKey, SortDirection};

const APP_NAME: &str = "viewskater-egui";

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jxl", "png", "apng", "bmp", "webp", "gif", "tiff", "tif", "qoi", "tga",
];
const ANIMATION_CAPABLE_EXTENSIONS: &[&str] = &["gif", "png", "apng", "webp"];

static REGISTER_IMAGE_DECODERS: Once = Once::new();

pub fn is_supported_image(path: &Path) -> bool {
    has_extension(path, SUPPORTED_EXTENSIONS)
}

pub fn may_have_animation(path: &Path) -> bool {
    has_extension(path, ANIMATION_CAPABLE_EXTENSIONS)
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| extensions.iter().any(|supported| ext.eq_ignore_ascii_case(supported)))
}

pub fn enumerate_images(dir: &Path, opts: ImageDiscoveryOptions) -> Vec<PathBuf> {
    let entries = enumerate_images_inner(dir, opts);

    let sort_order = &opts.sort_order;
    let paths = match sort_order.key {
        ImageSortKey::Name => sort_paths(entries, sort_order.direction, compare_names),
        ImageSortKey::Extension => sort_paths(entries, sort_order.direction, compare_extensions),
        ImageSortKey::Modified => {
            sort_files(entries, sort_order.direction, |a, b| a.modified.cmp(&b.modified))
        }
        ImageSortKey::Created => {
            sort_files(entries, sort_order.direction, |a, b| a.created.cmp(&b.created))
        }
        ImageSortKey::Size => sort_files(entries, sort_order.direction, |a, b| a.size.cmp(&b.size)),
    };

    log::info!("Found {} images in {}", paths.len(), dir.display());
    paths
}

fn enumerate_images_inner(dir: &Path, opts: ImageDiscoveryOptions) -> Vec<DirEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::warn!("Failed to read directory: {}", dir.display());
        return Vec::new();
    };

    let mut retval = Vec::<DirEntry>::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        match entry.file_type() {
            Ok(ftype) => {
                if !opts.include_hidden {
                    let is_hidden = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'));

                    if is_hidden {
                        continue;
                    }
                };

                if ftype.is_file() && is_supported_image(&path) {
                    retval.push(entry);
                } else if ftype.is_dir() && opts.recursive {
                    retval.append(&mut enumerate_images_inner(&path, opts));
                }
            }
            Err(err) => {
                log::warn!("Failed to get file type for {}: {}", path.display(), err);
            }
        }
    }

    retval
}

struct ImageFile {
    path: PathBuf,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    size: Option<u64>,
}

impl ImageFile {
    fn new(entry: DirEntry) -> Self {
        let metadata = entry.metadata().ok();
        Self {
            path: entry.path(),
            modified: metadata.as_ref().and_then(|m| m.modified().ok()),
            created: metadata.as_ref().and_then(|m| m.created().ok()),
            size: metadata.as_ref().map(|m| m.len()),
        }
    }
}

fn sort_paths(
    entries: Vec<DirEntry>,
    sort_direction: SortDirection,
    compare: impl Fn(&Path, &Path) -> Ordering,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = entries.into_iter().map(|entry| entry.path()).collect();
    paths.sort_by(|a, b| {
        apply_sort_direction(compare(a, b), sort_direction)
            .then_with(|| apply_sort_direction(compare_names(a, b), sort_direction))
    });
    paths
}

fn sort_files(
    entries: Vec<DirEntry>,
    sort_direction: SortDirection,
    compare: impl Fn(&ImageFile, &ImageFile) -> Ordering,
) -> Vec<PathBuf> {
    let mut images: Vec<ImageFile> = entries.into_iter().map(ImageFile::new).collect();
    images.sort_by(|a, b| {
        apply_sort_direction(compare(a, b), sort_direction)
            .then_with(|| apply_sort_direction(compare_names(&a.path, &b.path), sort_direction))
    });
    images.into_iter().map(|image| image.path).collect()
}

fn apply_sort_direction(ordering: Ordering, sort_direction: SortDirection) -> Ordering {
    match sort_direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn compare_names(a: &Path, b: &Path) -> Ordering {
    natord::compare(
        &a.as_os_str().to_string_lossy(),
        &b.as_os_str().to_string_lossy(),
    )
}

fn compare_extensions(a: &Path, b: &Path) -> Ordering {
    natord::compare(
        &a.extension().unwrap_or_default().to_string_lossy(),
        &b.extension().unwrap_or_default().to_string_lossy(),
    )
}

fn ensure_image_decoders_registered() {
    REGISTER_IMAGE_DECODERS.call_once(|| {
        jxl_oxide::integration::register_image_decoding_hook();
    });
}

/// A file opened for display: the decoded pixels, or the error, and the
/// facts about the file either way.
pub struct LoadedImage {
    pub image: ImageResult<DynamicImage>,
    pub record: Arc<MetadataRecord>,
}

/// Open the file, read its facts and its EXIF block, then decode the
/// pixels. Every place that decodes an image for display goes through
/// here, so the record exists for anything that can be on screen, and
/// it exists when the pixels fail too.
///
/// The EXIF bytes come from the decoder that is about to decode the
/// pixels: for JPEG the file is already in memory, for PNG the chunk was
/// read with the header, for WebP it is one small read, for JXL the
/// container is read for decoding anyway. There is no second pass over
/// the file.
pub fn load_image(path: &Path) -> LoadedImage {
    ensure_image_decoders_registered();
    let mut record = MetadataRecord::default();
    if let Ok(meta) = std::fs::metadata(path) {
        record.file_size = Some(meta.len());
        record.modified = meta.modified().ok().map(local_time_text);
    }
    let image = decode_into(path, &mut record);
    LoadedImage { image, record: Arc::new(record) }
}

fn decode_into(path: &Path, record: &mut MetadataRecord) -> ImageResult<DynamicImage> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let format = reader.format();
    record.format = format_name(format, path);
    let mut decoder = reader.into_decoder()?;
    match decoder.exif_metadata() {
        Ok(Some(bytes)) => record.exif = metadata::parse_exif(bytes),
        Ok(None) => {}
        Err(e) => {
            log::debug!("EXIF read failed for {}: {e}", path.display());
            record.exif = ExifData::Unreadable;
        }
    }
    // The allocation check `ImageReader::decode` makes before decoding;
    // `into_decoder` leaves it to the caller.
    let mut limits = image::Limits::default();
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    let image = DynamicImage::from_decoder(decoder);

    // A PNG may carry its eXIf chunk after the pixel data; ImageMagick
    // writes it there. The decoder only knew the chunks before the first
    // IDAT when it was asked above. Looked for after the decode, when the
    // file is in the OS cache, so it costs one read of the file's tail.
    if format == Some(ImageFormat::Png) && record.exif == ExifData::None {
        if let Some(bytes) = png_trailing_exif(path) {
            record.exif = metadata::parse_exif(bytes);
        }
    }
    image
}

/// How much of a PNG's end is searched for a trailing eXIf chunk. EXIF
/// blocks are at most a few tens of kilobytes and only text chunks and
/// IEND follow them.
const PNG_TAIL_BYTES: u64 = 128 * 1024;

fn png_trailing_exif(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(PNG_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut tail).ok()?;
    find_trailing_exif(&tail).map(<[u8]>::to_vec)
}

/// The payload of an eXIf chunk in the tail of a PNG file. Chunks can only
/// be walked forwards, and the tail may start in the middle of compressed
/// pixel data that happens to contain the letters "eXIf". So a candidate
/// is accepted only when the chunks after it, walked by their length
/// fields, arrive at an empty IEND chunk.
fn find_trailing_exif(tail: &[u8]) -> Option<&[u8]> {
    let mut search_end = tail.len();
    while let Some(pos) = tail[..search_end].windows(4).rposition(|w| w == b"eXIf") {
        if let Some(payload) = pos.checked_sub(4).and_then(|start| exif_if_chunks_reach_iend(tail, start)) {
            return Some(payload);
        }
        search_end = pos;
    }
    None
}

/// Walk chunks from `first`, which claims to be an eXIf chunk. Returns its
/// payload if the walk ends at an empty IEND inside `tail`.
fn exif_if_chunks_reach_iend(tail: &[u8], first: usize) -> Option<&[u8]> {
    let mut at = first;
    let mut payload = None;
    loop {
        let header = tail.get(at..at.checked_add(8)?)?;
        let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let kind = &header[4..8];
        let data_end = at.checked_add(8)?.checked_add(len)?;
        let next = data_end.checked_add(4)?; // the CRC
        if next > tail.len() {
            return None;
        }
        if at == first {
            payload = Some(&tail[at + 8..data_end]);
        }
        if kind == b"IEND" {
            return if len == 0 { payload } else { None };
        }
        at = next;
    }
}

/// The container format for the File section. Formats decoded through a
/// hook (JXL) have no `ImageFormat`, so the extension names them.
fn format_name(format: Option<ImageFormat>, path: &Path) -> Option<String> {
    let name = match format {
        Some(ImageFormat::Jpeg) => "JPEG",
        Some(ImageFormat::Png) => "PNG",
        Some(ImageFormat::WebP) => "WebP",
        Some(ImageFormat::Gif) => "GIF",
        Some(ImageFormat::Tiff) => "TIFF",
        Some(ImageFormat::Bmp) => "BMP",
        Some(ImageFormat::Qoi) => "QOI",
        Some(ImageFormat::Tga) => "TGA",
        Some(other) => return Some(format!("{other:?}").to_uppercase()),
        None => return path.extension().map(|e| e.to_string_lossy().to_uppercase()),
    };
    Some(name.to_string())
}

fn local_time_text(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(time)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

pub fn open_animation_frames(path: &Path) -> ImageResult<Option<image::Frames<'static>>> {
    let format = ImageReader::open(path)?.with_guessed_format()?.format();
    let file = || {
        File::open(path)
            .map(BufReader::new)
            .map_err(image::ImageError::IoError)
    };

    match format {
        Some(ImageFormat::Gif) => {
            let decoder = image::codecs::gif::GifDecoder::new(file()?)?;
            Ok(Some(decoder.into_frames()))
        }
        Some(ImageFormat::Png) => {
            let decoder = image::codecs::png::PngDecoder::new(file()?)?;
            if decoder.is_apng()? {
                Ok(Some(decoder.apng()?.into_frames()))
            } else {
                Ok(None)
            }
        }
        Some(ImageFormat::WebP) => {
            let decoder = image::codecs::webp::WebPDecoder::new(file()?)?;
            if decoder.has_animation() {
                Ok(Some(decoder.into_frames()))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A file whose pixels cannot be decoded still gets its file facts,
    /// so the panel and the footer have something to show next to
    /// "Failed to load image".
    #[test]
    fn record_exists_when_the_pixels_fail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.jpg");
        std::fs::write(&path, b"this is not a jpeg").unwrap();

        let loaded = load_image(&path);

        assert!(loaded.image.is_err());
        assert_eq!(loaded.record.file_size, Some(18));
        assert!(loaded.record.modified.is_some());
        assert_eq!(loaded.record.exif, ExifData::None);
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC, not checked
        out
    }

    #[test]
    fn trailing_exif_is_found_behind_the_pixel_data() {
        // The end of some IDAT data that happens to contain "eXIf", then
        // the real chunk, a text chunk and IEND, as ImageMagick writes.
        let mut tail = b"\x9c\x00\x00\x10\x00eXIf compressed bytes that are not a chunk".to_vec();
        tail.extend(chunk(b"eXIf", b"MM\0*real exif"));
        tail.extend(chunk(b"tEXt", b"exif:Make\0Apple"));
        tail.extend(chunk(b"IEND", b""));
        assert_eq!(find_trailing_exif(&tail), Some(&b"MM\0*real exif"[..]));

        // Only the decoy: nothing.
        let mut decoy = b"\x00\x00\x00\x05eXIfabcde".to_vec();
        decoy.extend(chunk(b"IDAT", b"more pixels"));
        assert_eq!(find_trailing_exif(&decoy), None);

        // No eXIf at all.
        let mut plain = chunk(b"IDAT", b"pixels");
        plain.extend(chunk(b"IEND", b""));
        assert_eq!(find_trailing_exif(&plain), None);

        // A chunk whose length runs past the end is refused.
        let mut cut = (1000u32).to_be_bytes().to_vec();
        cut.extend_from_slice(b"eXIfshort");
        assert_eq!(find_trailing_exif(&cut), None);
    }

    /// Reads real photos and prints their records, to compare with
    /// exiftool or `identify -format '%[EXIF:*]'`:
    ///
    ///     VIEWSKATER_EXIF_FILES=a.jpg:b.jpg cargo test real_photos -- --ignored --nocapture
    ///
    /// Set VIEWSKATER_EXIF_TAGS=1 to print every tag as the panel lists it.
    #[test]
    #[ignore]
    fn real_photos_have_camera_fields() {
        let Ok(list) = std::env::var("VIEWSKATER_EXIF_FILES") else {
            eprintln!("VIEWSKATER_EXIF_FILES is not set");
            return;
        };
        let print_tags = std::env::var("VIEWSKATER_EXIF_TAGS").is_ok();
        for path in list.split(':').filter(|p| !p.is_empty()) {
            let loaded = load_image(Path::new(path));
            assert!(loaded.image.is_ok(), "{path}: {:?}", loaded.image.err());
            let record = &loaded.record;
            eprintln!("{path}");
            eprintln!("  file: {:?} bytes, modified {:?}, {:?}", record.file_size, record.modified, record.format);
            let ExifData::Present(exif) = &record.exif else {
                panic!("{path}: {:?}", record.exif);
            };
            eprintln!("  camera:      {:?}", exif.camera);
            eprintln!("  lens:        {:?}", exif.lens);
            eprintln!("  focal:       {:?}", exif.focal_length);
            eprintln!("  aperture:    {:?}", exif.aperture);
            eprintln!("  shutter:     {:?}", exif.shutter);
            eprintln!("  iso:         {:?}", exif.iso);
            eprintln!("  bias:        {:?}", exif.exposure_bias);
            eprintln!("  date:        {:?}", exif.date_taken);
            eprintln!("  orientation: {:?}", exif.orientation);
            eprintln!("  location:    {:?}", exif.location);
            eprintln!("  tags:        {}", exif.tags.len());
            if print_tags {
                for (name, value) in &exif.tags {
                    eprintln!("    {name} = {value}");
                }
            }
            assert!(exif.camera.is_some(), "{path}: no camera");
        }
    }
}
