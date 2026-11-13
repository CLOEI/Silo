use egui::{Align, Layout, RichText};

use crate::app::{ActiveCenterTab, SiloApp};
use crate::icons::Icon;
use crate::theme;
use crate::ui::widgets;

pub(crate) fn show(
    app: &mut SiloApp,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    footprint: fastframe_titlebar::Footprint,
    header_height: f32,
) {
    let palette = app.palette;
    let rect = ui.max_rect();
    fastframe_titlebar::drag(ui, rect);

    ui.horizontal_centered(|ui| {
        ui.add_space(footprint.left + 16.0);

        egui::Frame::NONE
            .fill(palette.selected)
            .corner_radius(egui::CornerRadius::ZERO)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("SILO")
                        .font(theme::bold(13.0))
                        .color(palette.selected_text),
                );
            });

        ui.add_space(4.0);
        ui.label(
            RichText::new("Stardew Mod & Save Vault")
                .font(theme::semibold(17.0))
                .color(palette.text),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(20.0);
            if widgets::primary_icon_button(ui, &palette, Icon::Play, "Activate & Launch SMAPI")
                .clicked()
            {
                app.activate_selected_profile(true, ctx);
            }
            if widgets::secondary_icon_button(ui, &palette, Icon::Layers, "Mount Profile").clicked()
            {
                app.activate_selected_profile(false, ctx);
            }

            let settings_open = app.active_tab == ActiveCenterTab::Settings;
            let settings_clicked = if settings_open {
                widgets::primary_icon_button(ui, &palette, Icon::Settings, "Settings").clicked()
            } else {
                widgets::secondary_icon_button(ui, &palette, Icon::Settings, "Settings").clicked()
            };
            if settings_clicked {
                app.toggle_settings(ctx);
            }

            let (icon, label) = if app.dark_mode {
                (Icon::Sun, "Light")
            } else {
                (Icon::Moon, "Dark")
            };
            if widgets::secondary_icon_button(ui, &palette, icon, label).clicked() {
                app.toggle_dark_mode(ctx);
            }
        });
    });

    fastframe_titlebar::controls(
        ui,
        &fastframe_titlebar::Style {
            icon: palette.disabled,
            button: palette.separator,
            header_height,
        },
        &fastframe_titlebar::Labels::default(),
    );
}
