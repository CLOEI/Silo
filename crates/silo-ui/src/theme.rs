use egui::{
    Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, ThemePreference, Visuals,
};

use fastframe_fonts::Weight;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub background: Color32,
    pub container: Color32,
    pub separator: Color32,
    pub border: Color32,
    pub text: Color32,
    pub disabled: Color32,
    pub selected: Color32,
    pub selected_text: Color32,
    pub green: Color32,
    pub red: Color32,
    pub amber: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            dark: true,
            background: Color32::from_rgb(0x18, 0x18, 0x1B),
            container: Color32::from_rgb(0x20, 0x20, 0x24),
            separator: Color32::from_rgb(0x2A, 0x2A, 0x30),
            border: Color32::from_rgb(0x2E, 0x2E, 0x36),
            text: Color32::from_rgb(0xF4, 0xF4, 0xF5),
            disabled: Color32::from_rgb(0x94, 0x94, 0x9E),
            selected: Color32::from_rgb(0x4F, 0x8E, 0xF7),
            selected_text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            green: Color32::from_rgb(0x4A, 0xDE, 0x80),
            red: Color32::from_rgb(0xF8, 0x71, 0x71),
            amber: Color32::from_rgb(0xFB, 0xBF, 0x24),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            background: Color32::from_rgb(0xFA, 0xFA, 0xFA),
            container: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            separator: Color32::from_rgb(0xF1, 0xF1, 0xF3),
            border: Color32::from_rgb(0xE4, 0xE4, 0xE7),
            text: Color32::from_rgb(0x18, 0x18, 0x1B),
            disabled: Color32::from_rgb(0x71, 0x71, 0x7A),
            selected: Color32::from_rgb(0x25, 0x63, 0xEB),
            selected_text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            green: Color32::from_rgb(0x16, 0xA3, 0x4A),
            red: Color32::from_rgb(0xDC, 0x26, 0x26),
            amber: Color32::from_rgb(0xD9, 0x77, 0x06),
        }
    }
}

fn visuals(palette: Palette) -> Visuals {
    let mut v = if palette.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    v.dark_mode = palette.dark;
    v.panel_fill = palette.background;
    v.window_fill = palette.container;
    v.extreme_bg_color = palette.background;
    v.faint_bg_color = palette.container;
    v.code_bg_color = palette.container;
    v.override_text_color = Some(palette.text);
    v.hyperlink_color = palette.selected;
    v.warn_fg_color = palette.amber;
    v.error_fg_color = palette.red;
    v.window_stroke = Stroke::new(1.0, palette.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.selection.bg_fill = palette.selected;
    v.selection.stroke = Stroke::new(1.0, palette.selected_text);
    v.widgets.noninteractive.bg_fill = palette.container;
    v.widgets.noninteractive.weak_bg_fill = palette.container;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);

    v.widgets.inactive.bg_fill = palette.container;
    v.widgets.inactive.weak_bg_fill = palette.container;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
    v.widgets.inactive.corner_radius = CornerRadius::same(6);

    v.widgets.hovered.bg_fill = palette.separator;
    v.widgets.hovered.weak_bg_fill = palette.separator;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.selected);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, palette.text);
    v.widgets.hovered.corner_radius = CornerRadius::same(6);

    v.widgets.active.bg_fill = palette.selected;
    v.widgets.active.weak_bg_fill = palette.selected;
    v.widgets.active.bg_stroke = Stroke::new(1.0, palette.selected);
    v.widgets.active.fg_stroke = Stroke::new(1.0, palette.selected_text);
    v.widgets.active.corner_radius = CornerRadius::same(6);

    v.widgets.open.bg_fill = palette.separator;
    v.widgets.open.weak_bg_fill = palette.separator;
    v.widgets.open.bg_stroke = Stroke::new(1.0, palette.border);
    v.widgets.open.fg_stroke = Stroke::new(1.0, palette.text);
    v.widgets.open.corner_radius = CornerRadius::same(6);

    v
}

pub fn install_fonts(ctx: &egui::Context, rendering: fastframe_text::TextRendering) {
    let mut fonts = fastframe_fonts::FontSetup::default().definitions();
    rendering.apply_to(&mut fonts);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(18.0, FontFamily::Proportional),
        );
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        rendering.apply_to_visuals(&mut style.visuals);
    });
}

/// Applies the app palette to both themes and makes `dark` the active one.
pub fn apply(ctx: &egui::Context, palette: Palette, rendering: fastframe_text::TextRendering) {
    ctx.set_theme(if palette.dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    });
    let dark = visuals(Palette::dark());
    let light = visuals(Palette::light());
    ctx.set_visuals_of(Theme::Dark, dark);
    ctx.set_visuals_of(Theme::Light, light);
    ctx.all_styles_mut(|style| rendering.apply_to_visuals(&mut style.visuals));
}

pub fn bold(size: f32) -> FontId {
    Weight::Bold.font_id(size)
}

pub fn semibold(size: f32) -> FontId {
    Weight::SemiBold.font_id(size)
}

pub fn medium(size: f32) -> FontId {
    Weight::Medium.font_id(size)
}

pub fn regular(size: f32) -> FontId {
    Weight::Regular.font_id(size)
}
