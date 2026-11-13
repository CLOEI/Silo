use egui::{Color32, CornerRadius, Margin, Response, RichText, Stroke, Ui};

use crate::icons::Icon;
use crate::theme::{self, Palette};

pub fn card_frame(palette: &Palette) -> egui::Frame {
    egui::Frame::NONE
        .fill(palette.container)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(16))
}

pub fn primary_button(ui: &mut Ui, palette: &Palette, text: impl Into<String>) -> Response {
    ui.add(
        egui::Button::new(
            RichText::new(text.into())
                .font(theme::semibold(12.5))
                .color(palette.selected_text),
        )
        .fill(palette.selected)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(6))
        .min_size(egui::vec2(0.0, 28.0)),
    )
}

pub fn secondary_button(ui: &mut Ui, palette: &Palette, text: impl Into<String>) -> Response {
    ui.add(
        egui::Button::new(
            RichText::new(text.into())
                .font(theme::medium(12.5))
                .color(palette.text),
        )
        .fill(palette.background)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(6))
        .min_size(egui::vec2(0.0, 28.0)),
    )
}

pub fn primary_icon_button(
    ui: &mut Ui,
    palette: &Palette,
    icon: Icon,
    text: impl Into<String>,
) -> Response {
    ui.add(
        egui::Button::image_and_text(
            icon.image(palette.selected_text, 14.0),
            RichText::new(text.into())
                .font(theme::semibold(12.5))
                .color(palette.selected_text),
        )
        .fill(palette.selected)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(6))
        .min_size(egui::vec2(0.0, 28.0)),
    )
}

pub fn secondary_icon_button(
    ui: &mut Ui,
    palette: &Palette,
    icon: Icon,
    text: impl Into<String>,
) -> Response {
    ui.add(
        egui::Button::image_and_text(
            icon.image(palette.text, 14.0),
            RichText::new(text.into())
                .font(theme::medium(12.5))
                .color(palette.text),
        )
        .fill(palette.background)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(6))
        .min_size(egui::vec2(0.0, 28.0)),
    )
}

pub fn small_button(ui: &mut Ui, palette: &Palette, text: impl Into<String>) -> Response {
    ui.add(
        egui::Button::new(
            RichText::new(text.into())
                .font(theme::medium(11.0))
                .color(palette.text),
        )
        .fill(palette.background)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(5))
        .min_size(egui::vec2(0.0, 22.0)),
    )
}

pub fn badge(ui: &mut Ui, text: impl Into<String>, color: Color32, bg: Color32) -> Response {
    egui::Frame::NONE
        .fill(bg)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text.into())
                    .font(theme::bold(10.0))
                    .color(color),
            );
        })
        .response
}

pub fn clickable_badge(
    ui: &mut Ui,
    palette: &Palette,
    text: impl Into<String>,
    color: Color32,
    bg: Color32,
) -> Response {
    ui.add(
        egui::Button::new(
            RichText::new(text.into())
                .font(theme::semibold(10.5))
                .color(color),
        )
        .fill(bg)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(4)),
    )
}

pub fn heading(ui: &mut Ui, palette: &Palette, text: impl Into<String>, size: f32) -> Response {
    ui.label(
        RichText::new(text.into())
            .font(theme::bold(size))
            .color(palette.text),
    )
}

pub fn muted(ui: &mut Ui, palette: &Palette, text: impl Into<String>) -> Response {
    ui.label(
        RichText::new(text.into())
            .font(theme::regular(11.5))
            .color(palette.disabled),
    )
}

pub fn separator(ui: &mut Ui, palette: &Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, palette.border),
    );
}

pub fn storage_row(ui: &mut Ui, palette: &Palette, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.add_space(4.0);
        ui.label(
            RichText::new(label)
                .font(theme::medium(10.5))
                .color(palette.disabled),
        );
        ui.add(
            egui::Label::new(
                RichText::new(value)
                    .font(egui::FontId::monospace(11.0))
                    .color(palette.text),
            )
            .truncate(),
        );
    });
}

pub fn tab_button(
    ui: &mut Ui,
    palette: &Palette,
    text: impl Into<String>,
    active: bool,
) -> Response {
    let (bg, fg, stroke) = if active {
        (palette.selected, palette.selected_text, Stroke::NONE)
    } else {
        (
            palette.background,
            palette.text,
            Stroke::new(1.0, palette.border),
        )
    };
    ui.add(
        egui::Button::new(
            RichText::new(text.into())
                .font(theme::semibold(11.5))
                .color(fg),
        )
        .fill(bg)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(5))
        .min_size(egui::vec2(0.0, 24.0)),
    )
}

pub fn toggle_pill(ui: &mut Ui, palette: &Palette, enabled: bool) -> Response {
    let color = if enabled {
        palette.green
    } else {
        palette.disabled
    };
    ui.add(
        egui::Button::new(
            RichText::new(if enabled { "ENABLED" } else { "DISABLED" })
                .font(theme::bold(10.0))
                .color(color),
        )
        .fill(palette.container)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(6))
        .min_size(egui::vec2(0.0, 26.0)),
    )
}

pub fn nav_button(ui: &mut Ui, palette: &Palette, text: &str, enabled: bool) -> Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(
            RichText::new(text)
                .font(theme::medium(11.0))
                .color(palette.text),
        )
        .fill(palette.background)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(5))
        .min_size(egui::vec2(0.0, 24.0)),
    )
}
