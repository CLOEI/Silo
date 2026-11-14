use egui::{RichText, Sense, Stroke, Ui};

use crate::app::{SiloApp, dependency_badge_label};
use crate::theme;
use crate::ui::widgets;

const ROW_HEIGHT: f32 = 64.0;

enum Action {
    Toggle(i64, bool),
    OpenUrl(String),
}

pub(crate) fn show(app: &mut SiloApp, ui: &mut Ui, ctx: &egui::Context) {
    let palette = app.palette;

    if app.snapshot.profile_mods.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            widgets::heading(ui, &palette, "Your Silo Mod Vault is Empty", 17.0);
            ui.add_space(4.0);
            widgets::muted(
                ui,
                &palette,
                "Browse & install mods directly in \"Discover & Add Mods\", import a .zip/.7z archive or folder, or click \"Mod Manager Download\" on Nexus Mods (nxm:// is auto-bound).",
            );
            ui.add_space(12.0);
            if widgets::primary_button(ui, &palette, "Browse & Add Mods").clicked() {
                app.set_center_tab(crate::app::ActiveCenterTab::DiscoverMods, ctx);
            }
            ui.add_space(4.0);
            if widgets::secondary_button(ui, &palette, "Import Archive (.zip / .7z)").clicked() {
                app.prompt_import_archive(ctx);
            }
        });
        return;
    }

    let entries = app.snapshot.profile_mods.clone();
    let snapshot = app.snapshot.clone();
    let mut action = None;

    egui::ScrollArea::vertical()
        .id_salt("profile-mods-scroll")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, entries.len(), |ui, range| {
            for index in range {
                let entry = &entries[index];
                let mod_id = entry.mod_record.id;
                let enabled = entry.enabled;
                let mod_name = entry.mod_record.name.clone();
                let unique_id = entry.mod_record.unique_id.clone();
                let version = entry.mod_record.version.clone();
                let issues = snapshot.issues_for_mod(&unique_id);
                let update = snapshot.update_for_mod(&unique_id);

                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ROW_HEIGHT),
                    Sense::hover(),
                );
                if response.hovered() {
                    ui.painter().rect_filled(rect, 0.0, palette.container);
                }

                let content_rect = rect.shrink2(egui::vec2(16.0, 8.0));
                let mut child_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("profile-mod-row", mod_id))
                        .max_rect(content_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                child_ui.shrink_clip_rect(rect);
                {
                    let ui = &mut child_ui;
                    if widgets::toggle_pill(ui, &palette, enabled).clicked() {
                        action = Some(Action::Toggle(mod_id, enabled));
                    }
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(mod_name.clone())
                                    .font(theme::semibold(12.5))
                                    .color(palette.text),
                            )
                            .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(unique_id.clone())
                                    .font(theme::regular(10.5))
                                    .color(palette.disabled),
                            )
                            .truncate(),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::badge(
                            ui,
                            format!("v{version}"),
                            palette.text,
                            palette.container,
                        );

                        for issue in issues.iter().rev() {
                            if widgets::clickable_badge(
                                ui,
                                &palette,
                                dependency_badge_label(issue),
                                palette.red,
                                palette.container,
                            )
                            .clicked()
                            {
                                action = Some(Action::OpenUrl(issue.nexus_url()));
                            }
                        }

                        if let Some(update) = update
                            && widgets::clickable_badge(
                                ui,
                                &palette,
                                format!("Update Available: v{} ↗", update.latest_version),
                                palette.selected_text,
                                palette.selected,
                            )
                            .clicked()
                        {
                            action = Some(Action::OpenUrl(update.nexus_url()));
                        }
                    });
                }

                ui.painter()
                    .hline(rect.x_range(), rect.max.y, Stroke::new(1.0, palette.border));
            }
        });

    match action {
        Some(Action::Toggle(mod_id, enabled)) => app.toggle_mod(mod_id, enabled, ctx),
        Some(Action::OpenUrl(url)) => crate::app::open_external(ctx, url),
        None => {}
    }
}
