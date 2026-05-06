use image::DynamicImage;
use eframe::egui;
use eframe::egui::ColorImage;
use image::imageops::FilterType;

/// Maximum texture dimension supported by most GPUs. Images exceeding this
/// in either dimension are downscaled to fit before uploading to the GPU.
const MAX_TEXTURE_SIZE: u32 = 8192;

/// Convert a DynamicImage directly to egui's ColorImage, bypassing both
/// image crate v0.25's slow CICP color space conversion and egui's
/// per-pixel `from_rgba_unmultiplied` conversion. Goes straight from
/// decoded pixel data to `Vec<Color32>`.
///
/// Images larger than `MAX_TEXTURE_SIZE` in either dimension are
/// automatically downscaled to prevent GPU texture allocation failures.
pub fn image_to_color_image(img: DynamicImage) -> ColorImage {
    let img = downscale_if_needed(img);
    convert_image(img)
}

fn convert_image(img: DynamicImage) -> ColorImage {
    match img {
        DynamicImage::ImageRgb8(buf) => {
            let w = buf.width() as usize;
            let h = buf.height() as usize;
            let rgb = buf.into_raw();
            let pixels: Vec<egui::Color32> = rgb
                .chunks_exact(3)
                .map(|c| egui::Color32::from_rgb(c[0], c[1], c[2]))
                .collect();
            egui::ColorImage {
                size: [w, h],
                pixels,
            }
        }
        DynamicImage::ImageRgba8(buf) => {
            let w = buf.width() as usize;
            let h = buf.height() as usize;
            let rgba = buf.into_raw();
            let pixels: Vec<egui::Color32> = rgba
                .chunks_exact(4)
                .map(|c| egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]))
                .collect();
            egui::ColorImage {
                size: [w, h],
                pixels,
            }
        }
        other => {
            let rgba = other.into_rgba8();
            let w = rgba.width() as usize;
            let h = rgba.height() as usize;
            let pixels = rgba.into_raw();
            egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels)
        }
    }
}

/// Downscale the image if either dimension exceeds [`MAX_TEXTURE_SIZE`],
/// preserving aspect ratio. Uses Lanczos3 for quality.
fn downscale_if_needed(img: DynamicImage) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w <= MAX_TEXTURE_SIZE && h <= MAX_TEXTURE_SIZE {
        return img;
    }
    let scale = (MAX_TEXTURE_SIZE as f64 / w as f64).min(MAX_TEXTURE_SIZE as f64 / h as f64);
    let new_w = (w as f64 * scale).round() as u32;
    let new_h = (h as f64 * scale).round() as u32;
    log::info!(
        "Downscaling {}x{} -> {}x{} (exceeds {}px GPU limit)",
        w, h, new_w, new_h, MAX_TEXTURE_SIZE,
    );
    img.resize_exact(new_w, new_h, FilterType::Lanczos3)
}

/// Convert [`image::DynamicImage`] to [`egui::ColorImage`] and downscale it for the thumbnail
pub fn image_to_thumbnail(img: DynamicImage) -> ColorImage {
    let img = img.resize(crate::app::THUMBNAIL_WIDTH as u32, crate::app::THUMBNAIL_HEIGHT as u32, FilterType::Triangle);
    convert_image(img)
}
