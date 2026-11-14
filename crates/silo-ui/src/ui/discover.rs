use egui::{Align2, Id, Key, RichText, Sense, Stroke, Ui};

use crate::app::{SiloApp, file_uri};
use crate::icons::Icon;
use crate::theme;
use crate::ui::widgets;

pub(crate) fn show(app: &mut SiloApp, ui: &mut Ui, ctx: &egui::Context) {
    let palette = app.palette;

    ui.vertical(|ui| {
        egui::Frame::NONE
            .fill(palette.container)
            .corner_radius(egui::CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            })
            .inner_margin(egui::Margin::symmetric(16, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    let categories = [
                        ("All", "All"),
                        ("Live Nexus", "Live Nexus"),
                        ("Core Framework", "Frameworks"),
                        ("Expansion", "Expansions"),
                        ("Quality of Life", "QoL"),
                        ("Gameplay", "Gameplay"),
                        ("Visuals", "Visuals"),
                        ("Utility", "Utility"),
                    ];
                    for (key, label) in categories {
                        let active = app.discovery_category.eq_ignore_ascii_case(key);
                        if widgets::tab_button(ui, &palette, label, active).clicked() {
                            app.set_discovery_category(key.to_string(), ctx);
                        }
                    }
                });

                ui.add_space(6.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(Icon::Search.image(palette.disabled, 13.0));
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut app.discovery_search)
                                .id(Id::new("discover-search-input"))
                                .hint_text("Search mods...")
                                .desired_width(220.0)
                                .frame(
                                    egui::Frame::NONE
                                        .fill(palette.background)
                                        .stroke(egui::Stroke::new(1.0, palette.border))
                                        .corner_radius(egui::CornerRadius::same(6))
                                        .inner_margin(egui::Margin::symmetric(10, 5)),
                                ),
                        );
                        if response.changed() {
                            app.discovery_page = 1;
                            app.sync_cover_textures(ctx);
                        }
                        if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter))
                        {
                            app.discovery_page = 1;
                            if app.discovery_category == "Live Nexus" {
                                app.fetch_live_nexus_page(1, ctx);
                            }
                        }
                        if (response.has_focus() || response.lost_focus())
                            && ui.input(|input| input.key_pressed(Key::Escape))
                        {
                            app.clear_discovery_search(ctx);
                        }

                        if !app.discovery_search.is_empty()
                            && widgets::small_button(ui, &palette, "Clear").clicked()
                        {
                            app.clear_discovery_search(ctx);
                        }
                        if widgets::secondary_icon_button(
                            ui,
                            &palette,
                            Icon::Refresh,
                            "Sync Trending",
                        )
                        .clicked()
                        {
                            app.refresh_discovery_feed(ctx);
                        }
                    },
                );
            });

        let page_result = app.current_discovery_page();
        let items = page_result.mods.clone();
        let current_page = page_result.page;
        let total_pages = page_result.total_pages.max(1);
        let total_count = page_result.total_count;
        let start_idx = if total_count == 0 {
            0
        } else {
            (current_page - 1) * page_result.page_size + 1
        };
        let end_idx = if total_count == 0 {
            0
        } else {
            (start_idx + items.len().saturating_sub(1)).min(total_count)
        };

        let pagination_height = 44.0;
        let scroll_height = (ui.available_height() - pagination_height).max(120.0);

        egui::ScrollArea::vertical()
            .id_salt("discover-scroll")
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if items.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        widgets::muted(ui, &palette, "No mods match this catalog or search.");
                    });
                    return;
                }

                for item in &items {
                    let nexus_mod_id = item.nexus_mod_id;
                    let nexus_files_url = item.nexus_files_url();
                    let nexus_page_url = item.nexus_page_url();
                    let installed = app
                        .snapshot
                        .installed_vault_mod(item)
                        .map(|model| model.version.clone());
                    let has_api_key = app.snapshot.has_nexus_api_key;
                    let cover_uri = item.cached_cover_path.as_deref().map(file_uri);

                    let install_label = if installed.is_some() {
                        if has_api_key {
                            "Reinstall / Update"
                        } else {
                            "Get Update (nxm://) ↗"
                        }
                    } else if has_api_key {
                        "Install to Vault"
                    } else {
                        "Add via Nexus (nxm://) ↗"
                    };

                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(108.0, 60.0), Sense::hover());
                        if let Some(uri) = &cover_uri {
                            ui.put(
                                rect,
                                egui::Image::new(uri.as_str())
                                    .fit_to_exact_size(rect.size())
                                    .corner_radius(egui::CornerRadius::same(6)),
                            );
                        } else {
                            ui.painter().rect(
                                rect,
                                egui::CornerRadius::same(6),
                                palette.container,
                                Stroke::new(1.0, palette.border),
                                egui::StrokeKind::Inside,
                            );
                            ui.painter().text(
                                rect.center() - egui::vec2(0.0, 8.0),
                                Align2::CENTER_CENTER,
                                item.category.clone(),
                                theme::bold(10.0),
                                palette.selected,
                            );
                            ui.painter().text(
                                rect.center() + egui::vec2(0.0, 8.0),
                                Align2::CENTER_CENTER,
                                format!("#{nexus_mod_id}"),
                                theme::regular(10.0),
                                palette.disabled,
                            );
                        }

                        ui.add_space(12.0);
                        let actions_width = 330.0;
                        let details_width = (ui.available_width() - actions_width - 8.0).max(180.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(details_width, 60.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_max_width(details_width);
                                ui.horizontal(|ui| {
                                    widgets::badge(
                                        ui,
                                        item.category.clone(),
                                        palette.selected,
                                        palette.container,
                                    );
                                    let name_width = (details_width - 190.0).max(90.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(name_width, 18.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(item.name.clone())
                                                        .font(theme::bold(12.5))
                                                        .color(palette.text),
                                                )
                                                .truncate(),
                                            );
                                        },
                                    );
                                    widgets::badge(
                                        ui,
                                        format!("v{}", item.version),
                                        palette.text,
                                        palette.container,
                                    );
                                });
                                ui.horizontal(|ui| {
                                    widgets::muted(ui, &palette, format!("by {}", item.author));
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(item.summary.clone())
                                                .font(theme::regular(11.0))
                                                .color(palette.text),
                                        )
                                        .truncate(),
                                    );
                                });
                            },
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::secondary_button(ui, &palette, "Nexus Page ↗").clicked() {
                                crate::app::open_external(ctx, nexus_page_url.clone());
                            }
                            if widgets::primary_button(ui, &palette, install_label).clicked() {
                                app.install_discovered_mod(
                                    nexus_mod_id,
                                    item.name.clone(),
                                    nexus_files_url.clone(),
                                    ctx,
                                );
                            }
                            if let Some(version) = &installed {
                                widgets::badge(
                                    ui,
                                    format!("IN VAULT (v{version})"),
                                    palette.green,
                                    palette.container,
                                );
                            }
                        });
                    });

                    ui.add_space(8.0);
                    widgets::separator(ui, &palette);
                    ui.add_space(8.0);
                }
            });

        egui::Frame::NONE
            .fill(palette.container)
            .corner_radius(egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: 8,
                se: 8,
            })
            .inner_margin(egui::Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    widgets::muted(
                        ui,
                        &palette,
                        format!(
                            "{} · Showing {start_idx}–{end_idx} of {total_count} mods",
                            page_result.source_label
                        ),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let has_prev = current_page > 1;
                        let has_next = current_page < total_pages;
                        let prev_page = current_page.saturating_sub(1).max(1);
                        let next_page = (current_page + 1).min(total_pages);

                        if widgets::nav_button(ui, &palette, "Last »", has_next).clicked() {
                            app.set_discovery_page(total_pages, ctx);
                        }
                        if widgets::nav_button(ui, &palette, "Next Page ›", has_next).clicked() {
                            app.set_discovery_page(next_page, ctx);
                        }
                        widgets::badge(
                            ui,
                            format!("Page {current_page} / {total_pages}"),
                            palette.selected,
                            palette.background,
                        );
                        if widgets::nav_button(ui, &palette, "‹ Prev Page", has_prev).clicked() {
                            app.set_discovery_page(prev_page, ctx);
                        }
                        if widgets::nav_button(ui, &palette, "« First", has_prev).clicked() {
                            app.set_discovery_page(1, ctx);
                        }
                    });
                });
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_filter_container_spans_full_discover_width() {
        let ctx = egui::Context::default();
        let palette = theme::Palette::dark();
        let rendering = fastframe_text::TextRendering::default();
        theme::install_fonts(&ctx, rendering);
        theme::apply(&ctx, palette, rendering);

        let mut search = String::new();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };

        let mut container_width = 0.0f32;
        let mut outer_width = 0.0f32;
        let mut output = ctx.run_ui(raw_input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                outer_width = ui.available_width();
                let resp = egui::Frame::NONE
                    .fill(palette.container)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            for label in ["All", "Live Nexus", "Frameworks", "QoL"] {
                                let _ = widgets::tab_button(ui, &palette, label, label == "All");
                            }
                        });
                        ui.add_space(6.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), 32.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut search)
                                        .id(Id::new("discover-search-test"))
                                        .desired_width(220.0),
                                );
                            },
                        );
                    });
                container_width = resp.response.rect.width();
            });
        });
        output.textures_delta.clear();

        assert!(
            (container_width - outer_width).abs() <= 1.0,
            "expected filter container width ({container_width}) to match outer width ({outer_width})"
        );
    }
}
