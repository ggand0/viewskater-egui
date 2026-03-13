use std::collections::VecDeque;
use std::time::{Duration, Instant};

use eframe::egui;

const WINDOW_DURATION: Duration = Duration::from_secs(2);

/// Tracks image rendering performance — how many unique images are
/// displayed per second, not UI event loop frame rate.
pub struct ImagePerfTracker {
    image_timestamps: VecDeque<Instant>,
    pub last_decode_ms: Option<f64>,
}

impl ImagePerfTracker {
    pub fn new() -> Self {
        Self {
            image_timestamps: VecDeque::new(),
            last_decode_ms: None,
        }
    }

    /// Record that a new image was decoded and uploaded to the GPU.
    pub fn record_image_load(&mut self, decode_ms: f64) {
        self.last_decode_ms = Some(decode_ms);
        self.image_timestamps.push_back(Instant::now());
    }

    /// Calculate image rendering FPS from upload timestamps.
    /// Uses a 2-second rolling window, matching viewskater's ImageDisplayTracker.
    pub fn image_fps(&mut self) -> f64 {
        let now = Instant::now();
        let cutoff = now - WINDOW_DURATION;
        while let Some(front) = self.image_timestamps.front() {
            if *front < cutoff {
                self.image_timestamps.pop_front();
            } else {
                break;
            }
        }

        if self.image_timestamps.len() < 2 {
            return 0.0;
        }
        // Use (now - oldest) rather than (newest - oldest) so the denominator
        // keeps growing after navigation stops, producing smooth decay instead
        // of spiking when only a tight cluster of final timestamps remains.
        let oldest = *self.image_timestamps.front().unwrap();
        let span = now.duration_since(oldest).as_secs_f64();
        if span > 0.0 {
            (self.image_timestamps.len() - 1) as f64 / span
        } else {
            0.0
        }
    }

    /// Draw the performance overlay in the top-right corner.
    pub fn show_overlay(&mut self, ctx: &egui::Context) {
        let fps = self.image_fps();
        let mut text = format!("Img: {:.1} FPS", fps);
        if let Some(ms) = self.last_decode_ms {
            text.push_str(&format!(" | Decode: {:.1}ms", ms));
        }

        egui::Window::new("fps")
            .title_bar(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_TOP, [-10.0, 36.0])
            .interactable(false)
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_black_alpha(180))
                    .corner_radius(4.0)
                    .inner_margin(6.0),
            )
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(text)
                        .monospace()
                        .color(egui::Color32::WHITE)
                        .size(14.0),
                );
            });
    }
}
