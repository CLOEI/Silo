use egui::{RichText, Sense, Stroke, Ui};

use crate::app::{ActiveCenterTab, SiloApp};
use crate::icons::{Icon, icon_button};
use crate::theme;
use crate::ui::widgets;

enum Action {
    Rescan,
    Select(i64),
    Clone(i64),
    Delete(i64),
    NewProfile,
}

pub(crate) fn show(app: &mut SiloApp, ui: &mut Ui, ctx: &egui::Context) {
    let palette = app.palette;
    let in_settings = app.active_tab == ActiveCenterTab::Settings;
    let selected_profile_id = app.snapshot.selected_profile_id;
    let active_profile_id = app.snapshot.active_profile_id;

    let saves: Vec<(silo_core::SaveMetadata, Option<i64>)> = app
        .snapshot
        .saves
        .iter()
        .map(|save| {
            (
                save.clone(),
                app.snapshot
                    .profile_for_save(&save.folder_name)
                    .map(|profile| profile.id),
            )
        })
        .collect();

    let profiles: Vec<(i64, String)> = app
        .snapshot
        .profiles
        .iter()
        .filter(|profile| profile.save_folder_name.is_none())
        .map(|profile| (profile.id, profile.name.clone()))
        .collect();

    let save_count = saves.len();
    let profile_count = profiles.len();
    let mut action = None;

    egui::ScrollArea::vertical()
        .id_salt("left-rail-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::heading(ui, &palette, format!("SAVES ({save_count})"), 11.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::small_button(ui, &palette, "Rescan").clicked() {
                        action = Some(Action::Rescan);
                    }
                    ui.add(Icon::Refresh.image(palette.text, 11.0));
                });
            });
            ui.add_space(6.0);

            if saves.is_empty() {
                egui::Frame::NONE
                    .fill(palette.background)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        widgets::muted(
                            ui,
                            &palette,
                            "No Stardew Valley saves found in %APPDATA%/StardewValley/Saves.",
                        );
                    });
            } else {
                for (save, bound_profile_id) in &saves {
                    let is_selected = !in_settings
                        && bound_profile_id.is_some()
                        && *bound_profile_id == selected_profile_id;
                    let is_mounted =
                        bound_profile_id.is_some() && *bound_profile_id == active_profile_id;
                    let bg = if is_selected {
                        palette.separator
                    } else {
                        palette.background
                    };
                    let border = if is_selected {
                        palette.selected
                    } else {
                        palette.border
                    };

                    egui::Frame::NONE
                        .fill(bg)
                        .stroke(Stroke::new(1.0, border))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let title =
                                    format!("{} · {} Farm", save.farmer_name, save.farm_name);
                                let response = ui.add(
                                    egui::Label::new(
                                        RichText::new(title)
                                            .font(theme::semibold(12.0))
                                            .color(palette.text),
                                    )
                                    .truncate()
                                    .sense(Sense::click()),
                                );
                                if response.clicked()
                                    && let Some(profile_id) = bound_profile_id
                                {
                                    action = Some(Action::Select(*profile_id));
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if let Some(profile_id) = bound_profile_id
                                            && icon_button(ui, Icon::Copy, palette.text).clicked()
                                        {
                                            action = Some(Action::Clone(*profile_id));
                                        }
                                        if is_mounted {
                                            widgets::badge(
                                                ui,
                                                "MOUNTED",
                                                palette.green,
                                                palette.separator,
                                            );
                                        } else {
                                            widgets::badge(
                                                ui,
                                                "SAVE",
                                                palette.disabled,
                                                palette.separator,
                                            );
                                        }
                                    },
                                );
                            });

                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!(
                                        "Year {} · {} {} ({} days)",
                                        save.year, save.season, save.day_of_month, save.days_played
                                    ))
                                    .font(theme::regular(10.5))
                                    .color(palette.text),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(save.folder_name.clone())
                                        .font(theme::regular(10.5))
                                        .color(palette.disabled),
                                )
                                .truncate(),
                            );
                        });
                    ui.add_space(6.0);
                }
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                widgets::heading(ui, &palette, format!("PROFILES ({profile_count})"), 11.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary_button(ui, &palette, "+ New Profile").clicked() {
                        action = Some(Action::NewProfile);
                    }
                });
            });
            ui.add_space(6.0);

            for (profile_id, name) in &profiles {
                let is_selected = !in_settings && Some(*profile_id) == selected_profile_id;
                let is_mounted = Some(*profile_id) == active_profile_id;
                let bg = if is_selected {
                    palette.separator
                } else {
                    palette.background
                };
                let border = if is_selected {
                    palette.selected
                } else {
                    palette.border
                };

                egui::Frame::NONE
                    .fill(bg)
                    .stroke(Stroke::new(1.0, border))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                egui::Label::new(
                                    RichText::new(name.clone())
                                        .font(theme::semibold(12.0))
                                        .color(palette.text),
                                )
                                .truncate()
                                .sense(Sense::click()),
                            );
                            if response.clicked() {
                                action = Some(Action::Select(*profile_id));
                            }

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icon_button(ui, Icon::X, palette.red)
                                        .on_hover_text("Delete profile")
                                        .clicked()
                                    {
                                        action = Some(Action::Delete(*profile_id));
                                    }
                                    if icon_button(ui, Icon::Copy, palette.text)
                                        .on_hover_text("Clone profile")
                                        .clicked()
                                    {
                                        action = Some(Action::Clone(*profile_id));
                                    }
                                    if is_mounted {
                                        widgets::badge(
                                            ui,
                                            "MOUNTED",
                                            palette.green,
                                            palette.separator,
                                        );
                                    } else {
                                        widgets::badge(
                                            ui,
                                            "PROFILE",
                                            palette.disabled,
                                            palette.separator,
                                        );
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(6.0);
            }
        });

    match action {
        Some(Action::Rescan) => {
            app.refresh_state(None, Some("Rescanned Saves & Vault".to_string()), ctx)
        }
        Some(Action::Select(profile_id)) => app.select_profile(profile_id, ctx),
        Some(Action::Clone(profile_id)) => app.clone_profile(profile_id, ctx),
        Some(Action::Delete(profile_id)) => app.delete_profile(profile_id, ctx),
        Some(Action::NewProfile) => app.create_new_profile(ctx),
        None => {}
    }
}
