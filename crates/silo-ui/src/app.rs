use std::sync::Arc;

use egui::{Id, Key};
use silo_core::{DependencyIssueKind, paginate_discoverable_mods};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::state::{
    SiloSnapshot, activate_profile_blocking, cache_curated_covers_blocking,
    check_nexus_updates_blocking, clone_profile_blocking, create_profile_blocking,
    delete_profile_blocking, download_nxm_blocking, fetch_nexus_graphql_page_blocking,
    import_archives_blocking, import_folder_blocking, install_discovered_mod_blocking,
    load_snapshot_blocking, refresh_discovery_feed_blocking, rename_profile_blocking,
    save_dark_mode_blocking, save_nexus_api_key_blocking, toggle_mod_blocking,
    toggle_strict_lock_blocking,
};
use crate::theme::{self, Palette};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActiveCenterTab {
    ProfileMods,
    DiscoverMods,
    Settings,
}

pub(crate) enum UiMessage {
    Progress(String),
    Done(Box<anyhow::Result<SiloSnapshot>>),
}

pub struct SiloApp {
    pub(crate) snapshot: SiloSnapshot,
    pub(crate) active_tab: ActiveCenterTab,
    pub(crate) discovery_page: usize,
    pub(crate) discovery_page_size: usize,
    pub(crate) discovery_category: String,
    pub(crate) discovery_search: String,
    pub(crate) is_renaming_profile: bool,
    pub(crate) profile_rename_input: String,
    pub(crate) api_key_input: String,
    pub(crate) api_key_paste_pending: bool,
    pub(crate) dark_mode: bool,
    pub(crate) palette: Palette,
    pub(crate) rendering: fastframe_text::TextRendering,
    pub(crate) is_busy: bool,
    pub(crate) pending_jobs: usize,
    pub(crate) scrolling: fastframe_scroll::Scrolling,
    pub(crate) rt: Arc<Runtime>,
    pub(crate) jobs_tx: UnboundedSender<UiMessage>,
    pub(crate) jobs_rx: UnboundedReceiver<UiMessage>,
    pub(crate) nxm_rx: std::sync::mpsc::Receiver<String>,
    pub(crate) cover_uris: Vec<String>,
}

impl SiloApp {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        nxm_rx: std::sync::mpsc::Receiver<String>,
        initial_nxm: Option<String>,
        rendering: fastframe_text::TextRendering,
    ) -> Self {
        let snapshot =
            load_snapshot_blocking(None, None, None).unwrap_or_else(|err| SiloSnapshot {
                status_message: format!("Initialization warning: {err}"),
                ..Default::default()
            });

        let api_key_input = snapshot.nexus_api_key.clone();
        let dark_mode = snapshot
            .dark_mode
            .unwrap_or_else(|| matches!(cc.egui_ctx.system_theme(), Some(egui::Theme::Dark)));
        let palette = if dark_mode {
            Palette::dark()
        } else {
            Palette::light()
        };
        theme::apply(&cc.egui_ctx, palette, rendering);

        let rt = Arc::new(
            RuntimeBuilder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .thread_name("silo-tokio-worker")
                .build()
                .expect("failed to initialize Tokio runtime"),
        );
        let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();

        let mut app = Self {
            snapshot,
            active_tab: ActiveCenterTab::ProfileMods,
            discovery_page: 1,
            discovery_page_size: 8,
            discovery_category: "All".to_string(),
            discovery_search: String::new(),
            is_renaming_profile: false,
            profile_rename_input: String::new(),
            api_key_input,
            api_key_paste_pending: false,
            dark_mode,
            palette,
            rendering,
            is_busy: false,
            pending_jobs: 0,
            scrolling: fastframe_scroll::Scrolling::default(),
            rt,
            jobs_tx,
            jobs_rx,
            nxm_rx,
            cover_uris: Vec::new(),
        };

        if let Some(uri) = initial_nxm {
            app.handle_incoming_nxm(uri, &cc.egui_ctx);
        } else {
            app.refresh_state(None, None, &cc.egui_ctx);
        }

        app
    }

    fn spawn_job<F>(&mut self, label: impl Into<String>, job: F, ctx: &egui::Context)
    where
        F: FnOnce() -> anyhow::Result<SiloSnapshot> + Send + 'static,
    {
        self.snapshot.status_message = label.into();
        self.pending_jobs += 1;
        self.is_busy = true;

        let tx = self.jobs_tx.clone();
        let repaint = ctx.clone();
        self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(job)
                .await
                .unwrap_or_else(|err| Err(anyhow::anyhow!("Worker task panicked: {err}")));
            let _ = tx.send(UiMessage::Done(Box::new(result)));
            repaint.request_repaint();
        });
    }

    fn spawn_progress_job<F>(&mut self, label: impl Into<String>, job: F, ctx: &egui::Context)
    where
        F: FnOnce(UnboundedSender<String>) -> anyhow::Result<SiloSnapshot> + Send + 'static,
    {
        self.snapshot.status_message = label.into();
        self.pending_jobs += 1;
        self.is_busy = true;

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<String>();
        let tx = self.jobs_tx.clone();
        let repaint = ctx.clone();
        self.rt.spawn(async move {
            while let Some(message) = progress_rx.recv().await {
                let _ = tx.send(UiMessage::Progress(message));
                repaint.request_repaint();
            }
        });

        let tx = self.jobs_tx.clone();
        let repaint = ctx.clone();
        self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || job(progress_tx))
                .await
                .unwrap_or_else(|err| Err(anyhow::anyhow!("Worker task panicked: {err}")));
            let _ = tx.send(UiMessage::Done(Box::new(result)));
            repaint.request_repaint();
        });
    }

    fn drain_messages(&mut self, ctx: &egui::Context) {
        while let Ok(uri) = self.nxm_rx.try_recv() {
            self.handle_incoming_nxm(uri, ctx);
        }

        let mut finished = 0usize;
        while let Ok(message) = self.jobs_rx.try_recv() {
            match message {
                UiMessage::Progress(status) => {
                    self.snapshot.status_message = status;
                }
                UiMessage::Done(result) => {
                    match *result {
                        Ok(snapshot) => self.snapshot = snapshot,
                        Err(err) => self.snapshot.status_message = format!("Error: {err}"),
                    }
                    finished += 1;
                }
            }
        }

        if finished > 0 {
            self.pending_jobs = self.pending_jobs.saturating_sub(finished);
            self.is_busy = self.pending_jobs > 0;
            if self.active_tab == ActiveCenterTab::DiscoverMods {
                self.sync_cover_textures(ctx);
            }
        }
    }

    pub(crate) fn refresh_state(
        &mut self,
        preferred_profile_id: Option<i64>,
        status_override: Option<String>,
        ctx: &egui::Context,
    ) {
        let rt_handle = self.rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let target_profile = preferred_profile_id.or(self.snapshot.selected_profile_id);
        let label = status_override.clone().unwrap_or_default();
        self.spawn_job(
            label,
            move || {
                check_nexus_updates_blocking(rt_handle, game_path, target_profile, status_override)
            },
            ctx,
        );
    }

    pub(crate) fn select_profile(&mut self, profile_id: i64, ctx: &egui::Context) {
        if self.active_tab == ActiveCenterTab::Settings {
            self.active_tab = ActiveCenterTab::ProfileMods;
        }
        self.snapshot.selected_profile_id = Some(profile_id);
        self.refresh_state(Some(profile_id), None, ctx);
    }

    pub(crate) fn toggle_mod(&mut self, mod_id: i64, currently_enabled: bool, ctx: &egui::Context) {
        let Some(profile_id) = self.snapshot.selected_profile_id else {
            return;
        };
        let game_path = self.snapshot.game_install_path.clone();
        let next_enabled = !currently_enabled;
        self.spawn_job(
            "Updating profile mod state...",
            move || toggle_mod_blocking(game_path, profile_id, mod_id, next_enabled),
            ctx,
        );
    }

    pub(crate) fn set_strict_lock(&mut self, strict: bool, ctx: &egui::Context) {
        if self.snapshot.strict_lock == strict {
            return;
        }
        let selected_profile_id = self.snapshot.selected_profile_id;
        let game_path = self.snapshot.game_install_path.clone();
        self.spawn_job(
            "Updating global SiloGuard save lock...",
            move || toggle_strict_lock_blocking(game_path, selected_profile_id, strict),
            ctx,
        );
    }

    pub(crate) fn prompt_import_archive(&mut self, ctx: &egui::Context) {
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.spawn_job(
            "Select mod archive(s) (.zip / .7z) to import into vault...",
            move || {
                let picked = rfd::FileDialog::new()
                    .set_title("Import SMAPI Mod Archive (.zip / .7z)")
                    .add_filter("Mod Archives (.zip, .7z)", &["zip", "7z"])
                    .pick_files();
                match picked {
                    Some(files) if !files.is_empty() => {
                        import_archives_blocking(game_path, selected_profile_id, files)
                    }
                    _ => load_snapshot_blocking(
                        game_path,
                        selected_profile_id,
                        Some("Archive import cancelled".to_string()),
                    ),
                }
            },
            ctx,
        );
    }

    pub(crate) fn prompt_import_folder(&mut self, ctx: &egui::Context) {
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.spawn_job(
            "Select unpacked SMAPI mod folder to import into vault...",
            move || {
                let picked = rfd::FileDialog::new()
                    .set_title("Import Unpacked SMAPI Mod Folder")
                    .pick_folder();
                match picked {
                    Some(folder) => import_folder_blocking(game_path, selected_profile_id, folder),
                    None => load_snapshot_blocking(
                        game_path,
                        selected_profile_id,
                        Some("Folder import cancelled".to_string()),
                    ),
                }
            },
            ctx,
        );
    }

    pub(crate) fn clone_profile(&mut self, profile_id: i64, ctx: &egui::Context) {
        let game_path = self.snapshot.game_install_path.clone();
        self.spawn_job(
            "Cloning profile...",
            move || clone_profile_blocking(game_path, profile_id),
            ctx,
        );
    }

    pub(crate) fn create_new_profile(&mut self, ctx: &egui::Context) {
        let game_path = self.snapshot.game_install_path.clone();
        self.spawn_job(
            "Creating new standalone profile...",
            move || create_profile_blocking(game_path, None),
            ctx,
        );
    }

    pub(crate) fn start_renaming_profile(&mut self, ctx: &egui::Context) {
        let Some(profile) = self.snapshot.selected_profile() else {
            return;
        };
        self.profile_rename_input = profile.name.clone();
        self.is_renaming_profile = true;
        ctx.memory_mut(|memory| {
            memory.request_focus(Id::new("profile-rename-input"));
        });
    }

    pub(crate) fn commit_profile_rename(&mut self, ctx: &egui::Context) {
        if !self.is_renaming_profile {
            return;
        }
        self.is_renaming_profile = false;
        let Some(profile_id) = self.snapshot.selected_profile_id else {
            return;
        };
        let new_name = self.profile_rename_input.trim().to_string();
        if new_name.is_empty() {
            return;
        }
        let game_path = self.snapshot.game_install_path.clone();
        self.spawn_job(
            "Renaming profile...",
            move || rename_profile_blocking(game_path, profile_id, new_name),
            ctx,
        );
    }

    pub(crate) fn delete_profile(&mut self, profile_id: i64, ctx: &egui::Context) {
        let game_path = self.snapshot.game_install_path.clone();
        self.spawn_job(
            "Deleting profile...",
            move || delete_profile_blocking(game_path, profile_id),
            ctx,
        );
    }

    pub(crate) fn save_nexus_api_key_from_input(&mut self, ctx: &egui::Context) {
        let key_to_save = self.api_key_input.trim().to_string();
        self.api_key_input = key_to_save.clone();
        self.api_key_paste_pending = false;
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.spawn_job(
            "Saving Nexus Mods API key...",
            move || save_nexus_api_key_blocking(game_path, selected_profile_id, key_to_save),
            ctx,
        );
    }

    pub(crate) fn request_paste_api_key(&mut self, ctx: &egui::Context) {
        self.api_key_paste_pending = true;
        ctx.memory_mut(|memory| {
            memory.request_focus(Id::new("settings-api-key-input"));
        });
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    }

    pub(crate) fn toggle_dark_mode(&mut self, ctx: &egui::Context) {
        self.dark_mode = !self.dark_mode;
        self.palette = if self.dark_mode {
            Palette::dark()
        } else {
            Palette::light()
        };
        theme::apply(ctx, self.palette, self.rendering);

        let next_dark = self.dark_mode;
        self.rt.spawn(async move {
            let _ = tokio::task::spawn_blocking(move || save_dark_mode_blocking(next_dark)).await;
        });
        ctx.request_repaint();
    }

    pub(crate) fn set_center_tab(&mut self, tab: ActiveCenterTab, ctx: &egui::Context) {
        if self.active_tab == tab {
            return;
        }
        let previous = self.active_tab;
        self.active_tab = tab;

        if previous == ActiveCenterTab::DiscoverMods && tab != ActiveCenterTab::DiscoverMods {
            self.clear_cover_textures(ctx);
        }

        if tab == ActiveCenterTab::Settings && self.api_key_input.is_empty() {
            self.api_key_input = self.snapshot.nexus_api_key.clone();
        }

        if tab == ActiveCenterTab::DiscoverMods {
            self.sync_cover_textures(ctx);
            if self
                .snapshot
                .discoverable_mods
                .iter()
                .any(|m| m.cached_cover_path.is_none())
            {
                self.run_cover_cache_job(ctx);
            }
        }
        ctx.request_repaint();
    }

    pub(crate) fn toggle_settings(&mut self, ctx: &egui::Context) {
        if self.active_tab == ActiveCenterTab::Settings {
            self.set_center_tab(ActiveCenterTab::ProfileMods, ctx);
        } else {
            self.set_center_tab(ActiveCenterTab::Settings, ctx);
        }
    }

    fn run_cover_cache_job(&mut self, ctx: &egui::Context) {
        let rt_handle = self.rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.spawn_job(
            "Loading mod cover images from Nexus Mods...",
            move || cache_curated_covers_blocking(rt_handle, game_path, selected_profile_id),
            ctx,
        );
    }

    pub(crate) fn refresh_discovery_feed(&mut self, ctx: &egui::Context) {
        self.clear_cover_textures(ctx);
        self.discovery_page = 1;
        let rt_handle = self.rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.spawn_job(
            "Fetching live Trending & Updated Stardew Valley mods & covers from Nexus Mods...",
            move || refresh_discovery_feed_blocking(rt_handle, game_path, selected_profile_id),
            ctx,
        );
    }

    pub(crate) fn fetch_live_nexus_page(&mut self, page: usize, ctx: &egui::Context) {
        self.clear_cover_textures(ctx);
        self.snapshot.live_nexus_page = None;
        let target_page = page.max(1);
        self.discovery_page = target_page;
        self.discovery_category = "Live Nexus".to_string();

        let rt_handle = self.rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        let page_size = self.discovery_page_size;
        let search = if self.discovery_search.trim().is_empty() {
            None
        } else {
            Some(self.discovery_search.clone())
        };

        self.spawn_job(
            format!("Loading Nexus Mods Catalog & Covers — Page {target_page}..."),
            move || {
                fetch_nexus_graphql_page_blocking(
                    rt_handle,
                    game_path,
                    selected_profile_id,
                    target_page,
                    page_size,
                    search,
                )
            },
            ctx,
        );
    }

    pub(crate) fn set_discovery_page(&mut self, page: usize, ctx: &egui::Context) {
        let next_page = page.max(1);
        if self.discovery_category == "Live Nexus" {
            self.fetch_live_nexus_page(next_page, ctx);
        } else {
            self.discovery_page = next_page;
            self.sync_cover_textures(ctx);
            ctx.request_repaint();
        }
    }

    pub(crate) fn set_discovery_category(&mut self, category: String, ctx: &egui::Context) {
        if category == "Live Nexus" {
            self.fetch_live_nexus_page(1, ctx);
        } else {
            self.discovery_category = category;
            self.discovery_page = 1;
            self.sync_cover_textures(ctx);
            ctx.request_repaint();
        }
    }

    pub(crate) fn clear_discovery_search(&mut self, ctx: &egui::Context) {
        self.discovery_search.clear();
        self.discovery_page = 1;
        if self.discovery_category == "Live Nexus" {
            self.fetch_live_nexus_page(1, ctx);
        } else {
            self.sync_cover_textures(ctx);
            ctx.request_repaint();
        }
    }

    pub(crate) fn install_discovered_mod(
        &mut self,
        nexus_mod_id: i64,
        mod_display_name: String,
        nexus_files_url: String,
        ctx: &egui::Context,
    ) {
        if !self.snapshot.has_nexus_api_key {
            ctx.open_url(egui::OpenUrl::new_tab(nexus_files_url));
            self.snapshot.status_message = format!(
                "Opened {mod_display_name} on Nexus Mods — click 'Mod Manager Download' (nxm:// is auto-bound) or set your API Key for 1-click in-app install"
            );
            ctx.request_repaint();
            return;
        }

        let rt_handle = self.rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.spawn_progress_job(
            format!("Starting direct Nexus install for {mod_display_name} (#{nexus_mod_id})..."),
            move |progress_tx| {
                install_discovered_mod_blocking(
                    rt_handle,
                    game_path,
                    selected_profile_id,
                    nexus_mod_id,
                    mod_display_name,
                    progress_tx,
                )
            },
            ctx,
        );
    }

    pub(crate) fn handle_incoming_nxm(&mut self, nxm_uri: String, ctx: &egui::Context) {
        let rt_handle = self.rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.spawn_progress_job(
            format!("Resolving Nexus Mods link: {nxm_uri}..."),
            move |progress_tx| {
                download_nxm_blocking(
                    rt_handle,
                    game_path,
                    selected_profile_id,
                    nxm_uri,
                    progress_tx,
                )
            },
            ctx,
        );
    }

    pub(crate) fn activate_selected_profile(
        &mut self,
        launch_smapi_after: bool,
        ctx: &egui::Context,
    ) {
        let Some(profile_id) = self.snapshot.selected_profile_id else {
            return;
        };
        let game_path = self.snapshot.game_install_path.clone();
        let label = if launch_smapi_after {
            "Mounting profile in Mods/, writing active_profile.json & launching SMAPI..."
        } else {
            "Mounting profile in Mods/ & writing active_profile.json..."
        };
        self.spawn_job(
            label,
            move || activate_profile_blocking(game_path, profile_id, launch_smapi_after),
            ctx,
        );
    }

    pub(crate) fn current_discovery_page(&self) -> silo_core::DiscoveryPageResult {
        if self.discovery_category == "Live Nexus" {
            self.snapshot.live_nexus_page.clone().unwrap_or_else(|| {
                paginate_discoverable_mods(
                    &self.snapshot.discoverable_mods,
                    self.discovery_page,
                    self.discovery_page_size,
                    None,
                    Some(&self.discovery_search),
                    "Loading Live Nexus Catalog...",
                )
            })
        } else {
            paginate_discoverable_mods(
                &self.snapshot.discoverable_mods,
                self.discovery_page,
                self.discovery_page_size,
                Some(&self.discovery_category),
                Some(&self.discovery_search),
                "Curated & Trending Catalog",
            )
        }
    }

    pub(crate) fn sync_cover_textures(&mut self, ctx: &egui::Context) {
        let page = self.current_discovery_page();
        let next: Vec<String> = page
            .mods
            .iter()
            .filter_map(|item| item.cached_cover_path.as_deref().map(file_uri))
            .collect();
        for uri in &self.cover_uris {
            if !next.contains(uri) {
                ctx.forget_image(uri);
            }
        }
        self.cover_uris = next;
    }

    pub(crate) fn clear_cover_textures(&mut self, ctx: &egui::Context) {
        for uri in self.cover_uris.drain(..) {
            ctx.forget_image(&uri);
        }
    }

    fn center_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.active_tab == ActiveCenterTab::Settings {
            crate::ui::settings::show(self, ui, ctx);
            return;
        }

        let palette = self.palette;
        let profile_name = self
            .snapshot
            .selected_profile()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "No Profile Selected".to_string());
        let strict_lock_on = self.snapshot.strict_lock;
        let save_binding_label = match self.snapshot.selected_profile() {
            Some(p) => match (&p.save_folder_name, strict_lock_on) {
                (Some(folder), true) => {
                    format!("Locked to Save Folder: {folder} · SiloGuard: Strict Lock ON")
                }
                (Some(folder), false) => {
                    format!("Save Folder: {folder} · SiloGuard: Warn Only (All Saves Allowed)")
                }
                (None, true) => "Standalone Profile · SiloGuard: Strict Lock ON".to_string(),
                (None, false) => {
                    "Standalone Profile · SiloGuard: Warn Only (All Saves Allowed)".to_string()
                }
            },
            None => "No Profile Selected".to_string(),
        };
        let enabled_count = self
            .snapshot
            .profile_mods
            .iter()
            .filter(|m| m.enabled)
            .count();
        let total_count = self.snapshot.profile_mods.len();
        let discover_count = self.snapshot.discoverable_mods.len();
        let issue_count = self.snapshot.dependency_issues.len();
        let active_tab = self.active_tab;
        let standalone_selected = self
            .snapshot
            .selected_profile()
            .is_some_and(|p| p.save_folder_name.is_none());
        let selected_profile_id = self.snapshot.selected_profile_id;

        crate::ui::widgets::card_frame(&palette).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if self.is_renaming_profile {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.profile_rename_input)
                                    .id(Id::new("profile-rename-input"))
                                    .desired_width(220.0)
                                    .hint_text("Enter profile name...")
                                    .frame(
                                        egui::Frame::NONE
                                            .fill(palette.background)
                                            .stroke(egui::Stroke::new(1.0, palette.border))
                                            .corner_radius(egui::CornerRadius::same(6))
                                            .inner_margin(egui::Margin::symmetric(10, 5)),
                                    ),
                            );
                            let enter = response.lost_focus()
                                && ui.input(|input| input.key_pressed(Key::Enter));
                            if enter {
                                self.commit_profile_rename(ctx);
                            }
                            let escape = response.has_focus()
                                && ui.input(|input| input.key_pressed(Key::Escape));
                            if escape {
                                self.is_renaming_profile = false;
                            }
                            if crate::ui::widgets::primary_button(ui, &palette, "Save").clicked() {
                                self.commit_profile_rename(ctx);
                            }
                            if crate::ui::widgets::secondary_button(ui, &palette, "Cancel")
                                .clicked()
                            {
                                self.is_renaming_profile = false;
                            }
                        } else {
                            crate::ui::widgets::heading(
                                ui,
                                &palette,
                                format!("Profile: {profile_name}"),
                                16.0,
                            );
                            if crate::ui::widgets::small_button(ui, &palette, "Rename").clicked() {
                                self.start_renaming_profile(ctx);
                            }
                            if standalone_selected
                                && crate::ui::widgets::small_button(ui, &palette, "Delete Profile")
                                    .clicked()
                                && let Some(pid) = selected_profile_id
                            {
                                self.delete_profile(pid, ctx);
                            }
                        }
                    });
                    crate::ui::widgets::muted(ui, &palette, save_binding_label.clone());
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let issue_color = if issue_count > 0 {
                        palette.red
                    } else {
                        palette.green
                    };
                    let issue_text = if issue_count > 0 {
                        format!("{issue_count} Dependency Issue(s)")
                    } else {
                        "All Dependencies Satisfied".to_string()
                    };
                    crate::ui::widgets::badge(ui, issue_text, issue_color, palette.background);

                    if crate::ui::widgets::tab_button(
                        ui,
                        &palette,
                        format!("Discover & Add Mods ({discover_count})"),
                        active_tab == ActiveCenterTab::DiscoverMods,
                    )
                    .clicked()
                    {
                        self.set_center_tab(ActiveCenterTab::DiscoverMods, ctx);
                    }
                    if crate::ui::widgets::tab_button(
                        ui,
                        &palette,
                        format!("My Profile Mods ({enabled_count}/{total_count})"),
                        active_tab == ActiveCenterTab::ProfileMods,
                    )
                    .clicked()
                    {
                        self.set_center_tab(ActiveCenterTab::ProfileMods, ctx);
                    }
                });
            });
        });

        ui.add_space(12.0);
        let remaining = ui.available_height();
        egui::Frame::NONE
            .fill(palette.background)
            .stroke(egui::Stroke::new(1.0, palette.border))
            .corner_radius(egui::CornerRadius::same(8))
            .show(ui, |ui| {
                ui.set_min_height(remaining);
                ui.set_width(ui.available_width());
                match active_tab {
                    ActiveCenterTab::ProfileMods => {
                        crate::ui::profile_mods::show(self, ui, ctx);
                    }
                    ActiveCenterTab::DiscoverMods => {
                        crate::ui::discover::show(self, ui, ctx);
                    }
                    ActiveCenterTab::Settings => {}
                }
            });
    }
}

impl eframe::App for SiloApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.scrolling.apply(&ctx);
        self.drain_messages(&ctx);

        let header_height = 64.0;
        let footprint = fastframe_titlebar::footprint(&ctx, header_height);

        egui::Panel::top("silo-toolbar")
            .exact_size(header_height)
            .frame(
                egui::Frame::NONE
                    .fill(self.palette.container)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                crate::ui::toolbar::show(self, ui, &ctx, footprint, header_height);
            });

        if !self.snapshot.status_message.trim().is_empty() {
            egui::Panel::bottom("silo-footer")
                .exact_size(28.0)
                .frame(
                    egui::Frame::NONE
                        .fill(self.palette.container)
                        .inner_margin(egui::Margin::symmetric(20, 0)),
                )
                .show(ui, |ui| {
                    crate::ui::footer::show(self, ui);
                });
        }

        egui::Panel::left("silo-left-rail")
            .exact_size(330.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(self.palette.container)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ui, |ui| {
                crate::ui::left_rail::show(self, ui, &ctx);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(self.palette.background)
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ui, |ui| {
                self.center_ui(ui, &ctx);
            });

        fastframe_titlebar::align_traffic_lights(frame, &ctx, header_height);
    }
}

pub(crate) fn file_uri(path: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        format!("file://{}", path.to_string_lossy())
    }
}

pub(crate) fn dependency_badge_label(issue: &silo_core::DependencyIssue) -> String {
    match &issue.kind {
        DependencyIssueKind::Missing => match &issue.minimum_version {
            Some(min) => format!("Missing: {} (>={min}) ↗", issue.required_unique_id),
            None => format!("Missing: {} ↗", issue.required_unique_id),
        },
        DependencyIssueKind::Disabled => {
            format!("Disabled Dep: {} ↗", issue.required_unique_id)
        }
        DependencyIssueKind::IncompatibleVersion {
            installed_version,
            minimum_version,
        } => format!(
            "Update {}: v{installed_version} < v{minimum_version} ↗",
            issue.required_unique_id
        ),
    }
}
