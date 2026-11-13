use egui::{Color32, RichText, Sense, Stroke, Ui};

use crate::app::{ActiveCenterTab, SiloApp};
use crate::icons::{Icon, icon_button};
use crate::theme;
use crate::ui::widgets;

const SAVE_CARD_HEIGHT: f32 = 86.0;
const PROFILE_CARD_HEIGHT: f32 = 44.0;

enum Action {
    Rescan,
    Select(i64),
    Clone(i64),
    Delete(i64),
    NewProfile,
}

fn card_shell<R>(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    height: f32,
    background: Color32,
    hover_background: Color32,
    border: Color32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> (egui::Response, R) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click());
    let fill = if response.hovered() {
        hover_background
    } else {
        background
    };
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(8),
        fill,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let inner = ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(id_salt)
            .max_rect(rect.shrink2(egui::vec2(12.0, 8.0)))
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
        add_contents,
    );
    (response, inner.inner)
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
                for (index, (save, bound_profile_id)) in saves.iter().enumerate() {
                    let is_selected = !in_settings
                        && bound_profile_id.is_some()
                        && *bound_profile_id == selected_profile_id;
                    let is_mounted =
                        bound_profile_id.is_some() && *bound_profile_id == active_profile_id;
                    let background = if is_selected {
                        palette.separator
                    } else {
                        palette.background
                    };
                    let border = if is_selected {
                        palette.selected
                    } else {
                        palette.border
                    };
                    let title = format!("{} · {} Farm", save.farmer_name, save.farm_name);

                    let (card_response, child_action) = card_shell(
                        ui,
                        ("save-card", index),
                        SAVE_CARD_HEIGHT,
                        background,
                        palette.separator,
                        border,
                        |ui| {
                            let mut child_action = false;
                            ui.horizontal(|ui| {
                                let title_width = (ui.available_width() - 110.0).max(60.0);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(title_width, 20.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(title.clone())
                                                    .font(theme::semibold(12.0))
                                                    .color(palette.text),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if let Some(profile_id) = bound_profile_id
                                            && icon_button(ui, Icon::Copy, palette.text)
                                                .on_hover_text("Clone profile")
                                                .clicked()
                                        {
                                            action = Some(Action::Clone(*profile_id));
                                            child_action = true;
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
                            child_action
                        },
                    );

                    if card_response.clicked()
                        && !child_action
                        && let Some(profile_id) = bound_profile_id
                    {
                        action = Some(Action::Select(*profile_id));
                    }
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

            for (index, (profile_id, name)) in profiles.iter().enumerate() {
                let is_selected = !in_settings && Some(*profile_id) == selected_profile_id;
                let is_mounted = Some(*profile_id) == active_profile_id;
                let background = if is_selected {
                    palette.separator
                } else {
                    palette.background
                };
                let border = if is_selected {
                    palette.selected
                } else {
                    palette.border
                };

                let (card_response, child_action) = card_shell(
                    ui,
                    ("profile-card", index),
                    PROFILE_CARD_HEIGHT,
                    background,
                    palette.separator,
                    border,
                    |ui| {
                        let mut child_action = false;
                        ui.horizontal(|ui| {
                            let title_width = (ui.available_width() - 140.0).max(60.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(title_width, 20.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(name.clone())
                                                .font(theme::semibold(12.0))
                                                .color(palette.text),
                                        )
                                        .truncate(),
                                    );
                                },
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icon_button(ui, Icon::X, palette.red)
                                        .on_hover_text("Delete profile")
                                        .clicked()
                                    {
                                        action = Some(Action::Delete(*profile_id));
                                        child_action = true;
                                    }
                                    if icon_button(ui, Icon::Copy, palette.text)
                                        .on_hover_text("Clone profile")
                                        .clicked()
                                    {
                                        action = Some(Action::Clone(*profile_id));
                                        child_action = true;
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
                        child_action
                    },
                );

                if card_response.clicked() && !child_action {
                    action = Some(Action::Select(*profile_id));
                }
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
