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
    /// Menu item hover background
    pub menu_hover: egui::Color32,
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
            menu_hover: egui::Color32::from_gray(60),
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

        // Light gray hover/open backgrounds (matches iced's background.weak)
        visuals.widgets.hovered.weak_bg_fill = self.menu_hover;
        visuals.widgets.open.weak_bg_fill = self.menu_hover;

        ctx.set_visuals(visuals);
    }
}
