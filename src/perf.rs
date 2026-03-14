use std::collections::VecDeque;
use std::time::{Duration, Instant};

const WINDOW_DURATION: Duration = Duration::from_secs(2);

/// Tracks image rendering performance — how many unique images are
/// displayed per second, not UI event loop frame rate.
pub(crate) struct ImagePerfTracker {
    image_timestamps: VecDeque<Instant>,
}

impl ImagePerfTracker {
    pub(crate) fn new() -> Self {
        Self {
            image_timestamps: VecDeque::new(),
        }
    }

    /// Record that a new image was displayed.
    pub(crate) fn record_image_load(&mut self) {
        self.image_timestamps.push_back(Instant::now());
    }

    /// Calculate image rendering FPS from upload timestamps.
    /// Uses a 2-second rolling window, matching viewskater's ImageDisplayTracker.
    fn image_fps(&mut self) -> f64 {
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

    /// Build the FPS display string.
    pub(crate) fn fps_text(&mut self) -> String {
        format!("Img: {:.1} FPS", self.image_fps())
    }
}
