use eframe::egui;

/// Centralized color theme for all custom UI elements.
///
/// Built-in egui widgets (sliders, radio buttons, etc.) are themed via
/// `apply_to_visuals()` which sets the accent on egui's `Visuals`.
pub struct UiTheme {
    /// Primary accent color (toggles, links, highlights)
    pub accent: egui::Color32,
    /// Modal backdrop overlay
    pub backdrop: egui::Color32,
    /// Modal/card background fill
    pub card_bg: egui::Color32,
    /// Modal/card border stroke color
    pub card_stroke: egui::Color32,
    /// Subsection background (darker than card)
    pub section_bg: egui::Color32,
    /// Section heading text
    pub heading: egui::Color32,
    /// Secondary/muted text
    pub muted: egui::Color32,
    /// Toggle switch: off background
    pub toggle_off: egui::Color32,
    /// Toggle switch: knob
    pub toggle_knob: egui::Color32,
}

impl UiTheme {
    /// Teal dark theme matching the iced ViewSkater version.
    pub fn teal_dark() -> Self {
        Self {
            accent: egui::Color32::from_rgb(20, 148, 163),
            backdrop: egui::Color32::from_black_alpha(204),
            card_bg: egui::Color32::from_gray(40),
            card_stroke: egui::Color32::from_gray(80),
            section_bg: egui::Color32::from_gray(30),
            heading: egui::Color32::from_gray(180),
            muted: egui::Color32::from_gray(140),
            toggle_off: egui::Color32::from_gray(50),
            toggle_knob: egui::Color32::from_gray(240),
        }
    }

    /// Apply the theme to egui's built-in widget visuals.
    ///
    /// Sets accent colors, brightens text for VSCode-like contrast,
    /// and uses an accent-tinted background for hover/open states.
    pub fn apply_to_visuals(&self, ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();

        // Accent colors for selection, active widgets, hyperlinks
        visuals.selection.bg_fill = self.accent;
        visuals.hyperlink_color = self.accent;
        visuals.widgets.active.bg_fill = self.accent;

        // Bright widget text (default dark theme is too pale)
        visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_gray(210);
        visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_gray(220);
        visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_gray(255);
        visuals.widgets.active.fg_stroke.color = egui::Color32::from_gray(255);
        visuals.widgets.open.fg_stroke.color = egui::Color32::from_gray(255);

        // Accent-tinted hover/open backgrounds (35% accent mixed with dark base)
        let hover_bg = Self::blend_accent(self.accent, 35, 30);
        visuals.widgets.hovered.weak_bg_fill = hover_bg;
        visuals.widgets.open.weak_bg_fill = hover_bg;

        ctx.set_visuals(visuals);
    }

    /// Mix `accent` at `pct`% with gray(`base`) at `(100-pct)`%.
    fn blend_accent(accent: egui::Color32, pct: u16, base: u16) -> egui::Color32 {
        let [r, g, b, _] = accent.to_array();
        let rest = 100 - pct;
        egui::Color32::from_rgb(
            ((r as u16 * pct + base * rest) / 100) as u8,
            ((g as u16 * pct + base * rest) / 100) as u8,
            ((b as u16 * pct + base * rest) / 100) as u8,
        )
    }
}
