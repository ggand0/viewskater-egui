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
            accent: egui::Color32::from_rgb(26, 189, 208),
            backdrop: egui::Color32::from_black_alpha(140),
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
        let mut style = (*ctx.style()).clone();
        style.visuals = egui::Visuals::dark();

        // Accent colors for selection, active widgets, hyperlinks
        style.visuals.selection.bg_fill = self.accent;
        style.visuals.hyperlink_color = self.accent;
        style.visuals.widgets.active.bg_fill = self.accent;

        // Bright widget text (default dark theme is too pale)
        style.visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_gray(210);
        style.visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_gray(220);
        style.visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_gray(255);
        style.visuals.widgets.active.fg_stroke.color = egui::Color32::from_gray(255);
        style.visuals.widgets.open.fg_stroke.color = egui::Color32::from_gray(255);

        // Remove panel separator lines for a cleaner look
        style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;

        // Light gray hover/open backgrounds (matches iced's background.weak)
        style.visuals.widgets.hovered.weak_bg_fill = self.menu_hover;
        style.visuals.widgets.open.weak_bg_fill = self.menu_hover;

        ctx.set_style(style);
    }
}
