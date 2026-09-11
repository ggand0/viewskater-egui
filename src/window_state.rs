//! Persisted window geometry.
//!
//! eframe can persist the window on its own (`NativeOptions::persist_window`),
//! but it snapshots the window at save time. Quitting while maximized or
//! fullscreen then restores a maximized or fullscreen window on the next
//! launch, which flashes white on Windows (rust-windowing/winit#4575) and
//! loses the size the window had before it was maximized (emilk/egui#3494).
//!
//! Instead the app records the geometry every frame while the window is in
//! its normal state and writes that under eframe's own storage key. eframe
//! then restores a normal window at the last normal size and position,
//! using its usual monitor and scale-factor handling, and never a maximized
//! or fullscreen one.

use eframe::egui;

/// eframe's storage key for window settings. It is `STORAGE_WINDOW_KEY` in
/// eframe's `epi_integration`, private there. eframe reads this key at
/// startup regardless of `persist_window`; that flag only controls whether
/// eframe writes it.
pub const EFRAME_WINDOW_KEY: &str = "window";

/// Geometry of the window while it is neither maximized, fullscreen, nor
/// minimized. Units match `egui_winit::WindowSettings`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalWindowGeometry {
    /// Inner size in logical points.
    pub inner_size_points: egui::Vec2,
    /// Position of the window content in physical pixels. None where the
    /// platform does not expose window positions (Wayland).
    pub inner_position_pixels: Option<egui::Pos2>,
    /// Position of the window frame in physical pixels. None on Wayland.
    pub outer_position_pixels: Option<egui::Pos2>,
}

impl NormalWindowGeometry {
    /// Capture the current geometry, or None while the window is maximized,
    /// fullscreen, or minimized. On macOS egui-winit does not query the
    /// maximized state at runtime, so the window is asked directly; that
    /// also excludes the frames of a zoom animation in progress.
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    pub fn capture(ctx: &egui::Context, frame: &eframe::Frame) -> Option<Self> {
        #[cfg(target_os = "macos")]
        if crate::platform::macos::window_zoomed_or_resizing(frame) {
            return None;
        }
        ctx.input(|i| {
            let vp = i.viewport();
            if vp.maximized.unwrap_or(false)
                || vp.fullscreen.unwrap_or(false)
                || vp.minimized.unwrap_or(false)
            {
                return None;
            }
            // ViewportInfo rects are in points: physical pixels divided by
            // pixels_per_point (zoom factor times native scale), the same
            // convention WindowSettings::from_window uses for its size.
            let ppp = i.pixels_per_point;
            let inner_size_points = vp
                .inner_rect
                .map(|r| r.size())
                .unwrap_or_else(|| i.screen_rect().size());
            Some(Self {
                inner_size_points,
                inner_position_pixels: vp.inner_rect.map(|r| (r.min.to_vec2() * ppp).to_pos2()),
                outer_position_pixels: vp.outer_rect.map(|r| (r.min.to_vec2() * ppp).to_pos2()),
            })
        })
    }

    /// The value eframe deserializes on the next launch.
    pub fn to_window_settings(self) -> egui_winit::WindowSettings {
        egui_winit::WindowSettings::new(self.inner_size_points)
            .with_inner_position_pixels(self.inner_position_pixels)
            .with_outer_position_pixels(self.outer_position_pixels)
    }
}
