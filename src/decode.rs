use image::DynamicImage;
use eframe::egui;
use eframe::egui::ColorImage;
use image::imageops::FilterType;
use image::metadata::Orientation;

/// Maximum texture dimension supported by most GPUs. Images exceeding this
/// in either dimension are downscaled to fit before uploading to the GPU.
const MAX_TEXTURE_SIZE: u32 = 8192;

const THUMBNAIL_MAX_SIZE: u32 = 400;

/// Convert a DynamicImage directly to egui's ColorImage, bypassing both
/// image crate v0.25's slow CICP color space conversion and egui's
/// per-pixel `from_rgba_unmultiplied` conversion. Goes straight from
/// decoded pixel data to `Vec<Color32>`.
///
/// Images larger than `MAX_TEXTURE_SIZE` in either dimension are
/// automatically downscaled to prevent GPU texture allocation failures.
///
/// `orientation` is the turn the file's EXIF orientation tag asks for,
/// `LoadedImage::orientation`. It is applied after the downscale.
pub fn image_to_color_image(img: DynamicImage, orientation: Orientation) -> ColorImage {
    let mut img = downscale_if_needed(img);
    img.apply_orientation(orientation);
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

/// Convert [`image::DynamicImage`] to [`egui::ColorImage`], downscaling if either dimension exceeds [`THUMBNAIL_MAX_SIZE`].
/// `orientation` is applied after the downscale, so a photo is turned at thumbnail size.
pub fn image_to_thumbnail(img: DynamicImage, orientation: Orientation) -> ColorImage {
    let mut img = if img.width() <= THUMBNAIL_MAX_SIZE && img.height() <= THUMBNAIL_MAX_SIZE {
        img
    } else {
        img.resize(THUMBNAIL_MAX_SIZE, THUMBNAIL_MAX_SIZE, FilterType::Triangle)
    };
    img.apply_orientation(orientation);
    convert_image(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Red left of blue, turned a quarter clockwise, is red above blue.
    #[test]
    fn conversion_applies_the_orientation() {
        let mut stored = image::RgbImage::new(2, 1);
        stored.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        stored.put_pixel(1, 0, image::Rgb([0, 0, 255]));
        let stored = DynamicImage::ImageRgb8(stored);

        let shown = image_to_color_image(stored.clone(), Orientation::Rotate90);
        assert_eq!(shown.size, [1, 2]);
        assert_eq!(shown.pixels, [egui::Color32::RED, egui::Color32::BLUE]);

        let shown = image_to_color_image(stored, Orientation::NoTransforms);
        assert_eq!(shown.size, [2, 1]);
        assert_eq!(shown.pixels, [egui::Color32::RED, egui::Color32::BLUE]);
    }

    #[test]
    fn thumbnail_is_turned_after_the_downscale() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(1000, 500));
        assert_eq!(image_to_thumbnail(img.clone(), Orientation::NoTransforms).size, [400, 200]);
        assert_eq!(image_to_thumbnail(img, Orientation::Rotate90).size, [200, 400]);
    }
}
