use egui::{Id, Key, RichText, Ui};

use crate::app::SiloApp;
use crate::icons::Icon;
use crate::theme;
use crate::ui::widgets;

pub(crate) fn show(app: &mut SiloApp, ui: &mut Ui, ctx: &egui::Context) {
    let palette = app.palette;
    let strict_lock_on = app.snapshot.strict_lock;
    let has_api_key = app.snapshot.has_nexus_api_key;
    let vault_dir = app.snapshot.vault_dir.display().to_string();
    let db_path = app.snapshot.db_path.display().to_string();
    let saves_dir = app
        .snapshot
        .saves_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "Not found".to_string());

    egui::ScrollArea::vertical()
        .id_salt("settings-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width().min(760.0);
            ui.horizontal(|ui| {
                let padding = ((ui.available_width() - width) / 2.0).max(0.0);
                ui.add_space(padding);
                ui.vertical(|ui| {
                    ui.set_width(width);

                    ui.horizontal(|ui| {
                        ui.add(Icon::Settings.image(palette.text, 18.0));
                        ui.vertical(|ui| {
                            widgets::heading(ui, &palette, "Settings", 17.0);
                            widgets::muted(
                                ui,
                                &palette,
                                "Manage Nexus Mods API access, SiloGuard save lock, and local mod imports.",
                            );
                        });
                    });
                    ui.add_space(4.0);
                    widgets::separator(ui, &palette);
                    ui.add_space(12.0);

                    widgets::card_frame(&palette).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add(Icon::Key.image(palette.text, 15.0));
                            ui.label(
                                RichText::new("Nexus Mods Personal API Key")
                                    .font(theme::bold(13.0))
                                    .color(palette.text),
                            );
                            widgets::badge(
                                ui,
                                if has_api_key { "CONNECTED" } else { "NOT SET" },
                                if has_api_key {
                                    palette.green
                                } else {
                                    palette.disabled
                                },
                                palette.background,
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::secondary_button(
                                        ui,
                                        &palette,
                                        "Get API Key on Nexus ↗",
                                    )
                                    .clicked()
                                    {
                                        ctx.open_url(egui::OpenUrl::new_tab(
                                            "https://next.nexusmods.com/settings/api-keys",
                                        ));
                                    }
                                },
                            );
                        });

                        ui.add_space(2.0);
                        widgets::muted(
                            ui,
                            &palette,
                            "Type or press Ctrl+V inside the box below to set your Personal API Key for 1-click nxm:// downloads and update checks.",
                        );
                        ui.add_space(4.0);

                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if widgets::primary_icon_button(
                                    ui,
                                    &palette,
                                    Icon::Check,
                                    "Save Key",
                                )
                                .clicked()
                                {
                                    app.save_nexus_api_key_from_input(ctx);
                                }
                                if widgets::secondary_button(ui, &palette, "Paste").clicked() {
                                    app.request_paste_api_key(ctx);
                                }

                                let width = ui.available_width().max(160.0);
                                let response = ui.add(
                                    egui::TextEdit::singleline(&mut app.api_key_input)
                                        .id(Id::new("settings-api-key-input"))
                                        .password(true)
                                        .hint_text(
                                            "Enter or Ctrl+V paste your Nexus Mods Personal API Key...",
                                        )
                                        .desired_width(width)
                                        .frame(
                                            egui::Frame::NONE
                                                .fill(palette.background)
                                                .stroke(egui::Stroke::new(1.0, palette.border))
                                                .corner_radius(egui::CornerRadius::same(6))
                                                .inner_margin(egui::Margin::symmetric(10, 6)),
                                        ),
                                );
                                let enter = response.lost_focus()
                                    && ui.input(|input| input.key_pressed(Key::Enter));
                                if enter {
                                    app.save_nexus_api_key_from_input(ctx);
                                }
                                let paste_arrived = ui.input(|input| {
                                    input
                                        .events
                                        .iter()
                                        .any(|event| matches!(event, egui::Event::Paste(_)))
                                });
                                if paste_arrived && app.api_key_paste_pending {
                                    app.api_key_paste_pending = false;
                                    app.save_nexus_api_key_from_input(ctx);
                                }
                            },
                        );
                    });
                    ui.add_space(12.0);

                    widgets::card_frame(&palette).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add(Icon::Shield.image(palette.text, 15.0));
                            ui.label(
                                RichText::new("SiloGuard Save Protection (Global)")
                                    .font(theme::bold(13.0))
                                    .color(palette.text),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::tab_button(
                                        ui,
                                        &palette,
                                        "Warn Only",
                                        !strict_lock_on,
                                    )
                                    .clicked()
                                    {
                                        app.set_strict_lock(false, ctx);
                                    }
                                    if widgets::tab_button(
                                        ui,
                                        &palette,
                                        "Strict Lock ON",
                                        strict_lock_on,
                                    )
                                    .clicked()
                                    {
                                        app.set_strict_lock(true, ctx);
                                    }
                                },
                            );
                        });
                        ui.add_space(4.0);
                        widgets::muted(
                            ui,
                            &palette,
                            "Blocks loading a mismatched save in Stardew Valley when Strict Lock is active.",
                        );
                    });
                    ui.add_space(12.0);

                    widgets::card_frame(&palette).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add(Icon::FolderPlus.image(palette.text, 15.0));
                            ui.label(
                                RichText::new("Import Local Mods into Vault")
                                    .font(theme::bold(13.0))
                                    .color(palette.text),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::secondary_button(ui, &palette, "Import Folder")
                                        .clicked()
                                    {
                                        app.prompt_import_folder(ctx);
                                    }
                                    if widgets::primary_button(
                                        ui,
                                        &palette,
                                        "Import Archive (.zip / .7z)",
                                    )
                                    .clicked()
                                    {
                                        app.prompt_import_archive(ctx);
                                    }
                                },
                            );
                        });
                        ui.add_space(4.0);
                        widgets::muted(
                            ui,
                            &palette,
                            "Add downloaded .zip/.7z archives or unpacked SMAPI mod folders into your central vault.",
                        );
                    });
                    ui.add_space(12.0);

                    widgets::card_frame(&palette).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add(Icon::HardDrive.image(palette.text, 15.0));
                            ui.label(
                                RichText::new("Storage Locations")
                                    .font(theme::bold(13.0))
                                    .color(palette.text),
                            );
                        });
                        ui.add_space(4.0);
                        widgets::separator(ui, &palette);
                        widgets::storage_row(ui, &palette, "Central Mod Vault", &vault_dir);
                        widgets::separator(ui, &palette);
                        widgets::storage_row(ui, &palette, "SQLite Database", &db_path);
                        widgets::separator(ui, &palette);
                        widgets::storage_row(ui, &palette, "Stardew Saves Folder", &saves_dir);
                    });
                    ui.add_space(20.0);
                });
            });
        });
}
