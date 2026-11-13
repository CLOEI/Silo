mod icons;
mod state;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, FontWeight, KeyDownEvent, ObjectFit, QuitMode,
    RetainAllImageCache, SharedString, Task, TitlebarOptions, UniformListScrollHandle, Window,
    WindowAppearance, WindowBounds, WindowOptions, colors::Colors, div, green, hsla_to_rgba, img,
    prelude::*, px, red, rgb, size, uniform_list,
};
use rfd::FileDialog;
use silo_core::{DependencyIssueKind, paginate_discoverable_mods};
use silo_platform::{
    extract_nxm_arg_from_env, register_nxm_protocol_handler, spawn_nxm_ipc_listener,
    try_forward_nxm_to_running_instance,
};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use tokio::sync::mpsc;

use crate::icons::{
    LUCIDE_CHECK, LUCIDE_COPY, LUCIDE_FOLDER_PLUS, LUCIDE_HARD_DRIVE, LUCIDE_KEY, LUCIDE_LAYERS,
    LUCIDE_MOON, LUCIDE_PLAY, LUCIDE_REFRESH_CW, LUCIDE_SEARCH, LUCIDE_SETTINGS, LUCIDE_SHIELD,
    LUCIDE_SUN, LUCIDE_X, lucide_icon,
};
use crate::state::{
    SiloSnapshot, activate_profile_blocking, cache_curated_covers_blocking,
    check_nexus_updates_blocking, clone_profile_blocking, create_profile_blocking,
    delete_profile_blocking, download_nxm_blocking, fetch_nexus_graphql_page_blocking,
    import_archives_blocking, import_folder_blocking, install_discovered_mod_blocking,
    load_snapshot_blocking, refresh_discovery_feed_blocking, rename_profile_blocking,
    save_dark_mode_blocking, save_nexus_api_key_blocking, toggle_mod_blocking,
    toggle_strict_lock_blocking,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveCenterTab {
    ProfileMods,
    DiscoverMods,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextInputKind {
    Search,
    ProfileRename,
    ApiKey,
}

struct SiloWorkspace {
    tokio_rt: Arc<Runtime>,
    snapshot: SiloSnapshot,
    active_tab: ActiveCenterTab,
    discovery_page: usize,
    discovery_page_size: usize,
    discovery_category: String,
    discovery_search: String,
    search_cursor: usize,
    search_focus: FocusHandle,
    is_renaming_profile: bool,
    profile_rename_input: String,
    profile_rename_cursor: usize,
    profile_rename_focus: FocusHandle,
    api_key_input: String,
    api_key_cursor: usize,
    api_key_focus: FocusHandle,
    dark_mode: bool,
    cursor_blink_visible: bool,
    cursor_blink_ticks: u8,
    mod_scroll: UniformListScrollHandle,
    discover_scroll: UniformListScrollHandle,
    discover_image_cache: Entity<RetainAllImageCache>,
    discover_cache_needs_clear: bool,
    is_busy: bool,
    pending_task: Option<Task<()>>,
    _ipc_poller_task: Option<Task<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextKeyOutcome {
    Changed,
    Moved,
    Submit,
    Cancel,
    Ignored,
}

fn apply_text_edit_keystroke(
    text: &mut String,
    cursor: &mut usize,
    ks: & gpui::Keystroke,
    clipboard_text: Option<String>,
    max_chars: usize,
) -> TextKeyOutcome {
    let mut chars: Vec<char> = text.chars().collect();
    *cursor = (*cursor).min(chars.len());

    if ks.key == "enter" {
        return TextKeyOutcome::Submit;
    }
    if ks.key == "escape" {
        return TextKeyOutcome::Cancel;
    }
    if ks.key == "left" {
        *cursor = cursor.saturating_sub(1);
        return TextKeyOutcome::Moved;
    }
    if ks.key == "right" {
        *cursor = (*cursor + 1).min(chars.len());
        return TextKeyOutcome::Moved;
    }
    if ks.key == "home" {
        *cursor = 0;
        return TextKeyOutcome::Moved;
    }
    if ks.key == "end" {
        *cursor = chars.len();
        return TextKeyOutcome::Moved;
    }
    if ks.key == "backspace" {
        if (ks.modifiers.control || ks.modifiers.platform) && *cursor > 0 {
            chars.drain(0..*cursor);
            *cursor = 0;
            *text = chars.into_iter().collect();
            return TextKeyOutcome::Changed;
        } else if *cursor > 0 {
            chars.remove(*cursor - 1);
            *cursor -= 1;
            *text = chars.into_iter().collect();
            return TextKeyOutcome::Changed;
        }
        return TextKeyOutcome::Moved;
    }
    if ks.key == "delete" {
        if *cursor < chars.len() {
            chars.remove(*cursor);
            *text = chars.into_iter().collect();
            return TextKeyOutcome::Changed;
        }
        return TextKeyOutcome::Moved;
    }
    if (ks.modifiers.control || ks.modifiers.platform) && ks.key.eq_ignore_ascii_case("a") {
        *cursor = chars.len();
        return TextKeyOutcome::Moved;
    }
    if (ks.modifiers.control || ks.modifiers.platform) && ks.key.eq_ignore_ascii_case("v") {
        if let Some(pasted) = clipboard_text {
            let mut changed = false;
            for ch in pasted.chars().filter(|c| !c.is_control()) {
                if chars.len() < max_chars {
                    chars.insert(*cursor, ch);
                    *cursor += 1;
                    changed = true;
                }
            }
            if changed {
                *text = chars.into_iter().collect();
                return TextKeyOutcome::Changed;
            }
        }
        return TextKeyOutcome::Ignored;
    }
    if !ks.modifiers.control && !ks.modifiers.alt && !ks.modifiers.platform {
        if let Some(ref kc) = ks.key_char {
            let mut changed = false;
            for ch in kc.chars().filter(|c| !c.is_control()) {
                if chars.len() < max_chars {
                    chars.insert(*cursor, ch);
                    *cursor += 1;
                    changed = true;
                }
            }
            if changed {
                *text = chars.into_iter().collect();
                return TextKeyOutcome::Changed;
            }
        } else if ks.key == "space" && chars.len() < max_chars {
            chars.insert(*cursor, ' ');
            *cursor += 1;
            *text = chars.into_iter().collect();
            return TextKeyOutcome::Changed;
        }
    }
    TextKeyOutcome::Ignored
}

impl SiloWorkspace {
    fn new(
        initial_nxm_uris: Vec<String>,
        ipc_rx: mpsc::UnboundedReceiver<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tokio_rt = Arc::new(
            RuntimeBuilder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .thread_name("silo-tokio-worker")
                .build()
                .expect("failed to initialize Tokio runtime"),
        );

        let initial_snapshot = load_snapshot_blocking(None, None, None).unwrap_or_else(|err| {
            SiloSnapshot {
                status_message: format!("Initialization warning: {err}"),
                ..Default::default()
            }
        });

        let initial_api_key = initial_snapshot.nexus_api_key.clone();
        let initial_api_key_cursor = initial_api_key.chars().count();
        let initial_dark_mode = initial_snapshot.dark_mode.unwrap_or_else(|| {
            matches!(
                window.appearance(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            )
        });

        let mut ws = Self {
            tokio_rt,
            snapshot: initial_snapshot,
            active_tab: ActiveCenterTab::ProfileMods,
            discovery_page: 1,
            discovery_page_size: 8,
            discovery_category: "All".to_string(),
            discovery_search: String::new(),
            search_cursor: 0,
            search_focus: cx.focus_handle(),
            is_renaming_profile: false,
            profile_rename_input: String::new(),
            profile_rename_cursor: 0,
            profile_rename_focus: cx.focus_handle(),
            api_key_input: initial_api_key,
            api_key_cursor: initial_api_key_cursor,
            api_key_focus: cx.focus_handle(),
            dark_mode: initial_dark_mode,
            cursor_blink_visible: true,
            cursor_blink_ticks: 0,
            mod_scroll: UniformListScrollHandle::new(),
            discover_scroll: UniformListScrollHandle::new(),
            discover_image_cache: RetainAllImageCache::new(cx),
            discover_cache_needs_clear: false,
            is_busy: false,
            pending_task: None,
            _ipc_poller_task: None,
        };

        ws.start_nxm_ipc_bridge(ipc_rx, cx);

        if let Some(first_nxm) = initial_nxm_uris.into_iter().next() {
            ws.handle_incoming_nxm_uri(first_nxm, cx);
        } else {
            ws.refresh_state(None, None, cx);
        }

        ws
    }

    fn start_nxm_ipc_bridge(
        &mut self,
        mut ipc_rx: mpsc::UnboundedReceiver<String>,
        cx: &mut Context<Self>,
    ) {
        self._ipc_poller_task = Some(cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;

                let mut received = Vec::new();
                while let Ok(uri) = ipc_rx.try_recv() {
                    received.push(uri);
                }

                let ok = view.update(cx, |ws, cx| {
                    for uri in received {
                        ws.handle_incoming_nxm_uri(uri, cx);
                    }
                    ws.cursor_blink_ticks = ws.cursor_blink_ticks.wrapping_add(1);
                    if ws.cursor_blink_ticks >= 4 {
                        ws.cursor_blink_ticks = 0;
                        ws.cursor_blink_visible = !ws.cursor_blink_visible;
                        if ws.is_renaming_profile
                            || ws.active_tab == ActiveCenterTab::DiscoverMods
                            || ws.active_tab == ActiveCenterTab::Settings
                        {
                            cx.notify();
                        }
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        }));
    }

    fn run_tokio_job<F>(&mut self, busy_label: impl Into<String>, job: F, cx: &mut Context<Self>)
    where
        F: FnOnce() -> anyhow::Result<SiloSnapshot> + Send + 'static,
    {
        self.is_busy = true;
        self.snapshot.status_message = busy_label.into();
        cx.notify();

        let rt = self.tokio_rt.clone();
        self.pending_task = Some(cx.spawn(async move |view, cx| {
            let join_res = cx
                .background_spawn(async move { rt.spawn_blocking(job).await })
                .await;

            let _ = view.update(cx, |ws, cx| {
                ws.is_busy = false;
                match join_res {
                    Ok(Ok(new_snapshot)) => {
                        let page_changed = ws.snapshot.live_nexus_page.as_ref().map(|p| {
                            (
                                p.page,
                                p.mods
                                    .iter()
                                    .map(|m| m.nexus_mod_id)
                                    .collect::<Vec<_>>(),
                            )
                        }) != new_snapshot.live_nexus_page.as_ref().map(|p| {
                            (
                                p.page,
                                p.mods
                                    .iter()
                                    .map(|m| m.nexus_mod_id)
                                    .collect::<Vec<_>>(),
                            )
                        });
                        if page_changed || ws.active_tab != ActiveCenterTab::DiscoverMods {
                            ws.discover_cache_needs_clear = true;
                        }
                        ws.snapshot = new_snapshot;
                    }
                    Ok(Err(err)) => {
                        ws.snapshot.status_message = format!("Error: {err}");
                    }
                    Err(join_err) => {
                        ws.snapshot.status_message = format!("Worker task panicked: {join_err}");
                    }
                }
                cx.notify();
            });
        }));
    }

    fn handle_incoming_nxm_uri(&mut self, nxm_uri: String, cx: &mut Context<Self>) {
        self.is_busy = true;
        self.snapshot.status_message = format!("Resolving Nexus Mods link: {nxm_uri}...");
        cx.notify();

        let rt = self.tokio_rt.clone();
        let rt_handle = rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<String>();

        self.pending_task = Some(cx.spawn(async move |view, cx| {
            let mut download_fut = cx.background_spawn(async move {
                rt.spawn_blocking(move || {
                    download_nxm_blocking(
                        rt_handle,
                        game_path,
                        selected_profile_id,
                        nxm_uri,
                        progress_tx,
                    )
                })
                .await
            });

            loop {
                while let Ok(progress_msg) = progress_rx.try_recv() {
                    let _ = view.update(cx, |ws, cx| {
                        ws.snapshot.status_message = progress_msg;
                        cx.notify();
                    });
                }

                match futures_poll_once(&mut download_fut) {
                    Some(join_res) => {
                        let _ = view.update(cx, |ws, cx| {
                            ws.is_busy = false;
                            match join_res {
                                Ok(Ok(new_snapshot)) => {
                                    ws.snapshot = new_snapshot;
                                }
                                Ok(Err(err)) => {
                                    ws.snapshot.status_message =
                                        format!("NXM Download Error: {err}");
                                }
                                Err(join_err) => {
                                    ws.snapshot.status_message =
                                        format!("NXM Worker panicked: {join_err}");
                                }
                            }
                            cx.notify();
                        });
                        break;
                    }
                    None => {
                        cx.background_executor()
                            .timer(Duration::from_millis(80))
                            .await;
                    }
                }
            }
        }));
    }

    fn refresh_state(
        &mut self,
        preferred_profile_id: Option<i64>,
        status_override: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let rt_handle = self.tokio_rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let target_profile = preferred_profile_id.or(self.snapshot.selected_profile_id);
        let busy_label = status_override.clone().unwrap_or_default();
        self.run_tokio_job(
            busy_label,
            move || {
                check_nexus_updates_blocking(
                    rt_handle,
                    game_path,
                    target_profile,
                    status_override,
                )
            },
            cx,
        );
    }

    fn select_profile(&mut self, profile_id: i64, cx: &mut Context<Self>) {
        if self.active_tab == ActiveCenterTab::Settings {
            self.active_tab = ActiveCenterTab::ProfileMods;
        }
        self.snapshot.selected_profile_id = Some(profile_id);
        cx.notify();
        self.refresh_state(Some(profile_id), None, cx);
    }

    fn toggle_mod(&mut self, mod_id: i64, currently_enabled: bool, cx: &mut Context<Self>) {
        let Some(profile_id) = self.snapshot.selected_profile_id else {
            return;
        };
        let game_path = self.snapshot.game_install_path.clone();
        let next_enabled = !currently_enabled;
        self.run_tokio_job(
            "Updating profile mod state...",
            move || toggle_mod_blocking(game_path, profile_id, mod_id, next_enabled),
            cx,
        );
    }

    fn set_strict_lock(&mut self, strict: bool, cx: &mut Context<Self>) {
        if self.snapshot.strict_lock == strict {
            return;
        }
        let selected_profile_id = self.snapshot.selected_profile_id;
        let game_path = self.snapshot.game_install_path.clone();

        self.run_tokio_job(
            "Updating global SiloGuard save lock...",
            move || toggle_strict_lock_blocking(game_path, selected_profile_id, strict),
            cx,
        );
    }

    fn prompt_import_archive(&mut self, cx: &mut Context<Self>) {
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;

        self.run_tokio_job(
            "Select mod archive(s) (.zip / .7z) to import into vault...",
            move || {
                let picked = FileDialog::new()
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
            cx,
        );
    }

    fn prompt_import_folder(&mut self, cx: &mut Context<Self>) {
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;

        self.run_tokio_job(
            "Select unpacked SMAPI mod folder to import into vault...",
            move || {
                let picked = FileDialog::new()
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
            cx,
        );
    }

    fn clone_profile(&mut self, profile_id: i64, cx: &mut Context<Self>) {
        let game_path = self.snapshot.game_install_path.clone();
        self.run_tokio_job(
            "Cloning profile...",
            move || clone_profile_blocking(game_path, profile_id),
            cx,
        );
    }

    fn create_new_profile(&mut self, cx: &mut Context<Self>) {
        let game_path = self.snapshot.game_install_path.clone();
        self.run_tokio_job(
            "Creating new standalone profile...",
            move || create_profile_blocking(game_path, None),
            cx,
        );
    }

    fn start_renaming_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.snapshot.selected_profile() else {
            return;
        };
        self.profile_rename_input = profile.name.clone();
        self.profile_rename_cursor = self.profile_rename_input.chars().count();
        self.is_renaming_profile = true;
        self.cursor_blink_visible = true;
        self.cursor_blink_ticks = 0;
        window.focus(&self.profile_rename_focus, cx);
        cx.notify();
    }

    fn commit_profile_rename(&mut self, cx: &mut Context<Self>) {
        if !self.is_renaming_profile {
            return;
        }
        self.is_renaming_profile = false;
        let Some(profile_id) = self.snapshot.selected_profile_id else {
            cx.notify();
            return;
        };
        let new_name = self.profile_rename_input.trim().to_string();
        if new_name.is_empty() {
            cx.notify();
            return;
        }
        let game_path = self.snapshot.game_install_path.clone();
        self.run_tokio_job(
            "Renaming profile...",
            move || rename_profile_blocking(game_path, profile_id, new_name),
            cx,
        );
    }

    fn handle_profile_rename_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cursor_blink_visible = true;
        self.cursor_blink_ticks = 0;
        let ks = &event.keystroke;
        let clipboard = if (ks.modifiers.control || ks.modifiers.platform)
            && ks.key.eq_ignore_ascii_case("v")
        {
            cx.read_from_clipboard().and_then(|i| i.text())
        } else {
            None
        };

        match apply_text_edit_keystroke(
            &mut self.profile_rename_input,
            &mut self.profile_rename_cursor,
            ks,
            clipboard,
            64,
        ) {
            TextKeyOutcome::Submit => {
                window.blur(cx);
                self.commit_profile_rename(cx);
            }
            TextKeyOutcome::Cancel => {
                self.is_renaming_profile = false;
                window.blur(cx);
                cx.notify();
            }
            TextKeyOutcome::Changed | TextKeyOutcome::Moved => {
                cx.notify();
            }
            TextKeyOutcome::Ignored => {}
        }
    }

    fn delete_profile(&mut self, profile_id: i64, cx: &mut Context<Self>) {
        let game_path = self.snapshot.game_install_path.clone();
        self.run_tokio_job(
            "Deleting profile...",
            move || delete_profile_blocking(game_path, profile_id),
            cx,
        );
    }

    fn save_nexus_api_key_from_input(&mut self, cx: &mut Context<Self>) {
        let key_to_save = self.api_key_input.trim().to_string();
        self.api_key_input = key_to_save.clone();
        self.api_key_cursor = self.api_key_input.chars().count();

        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.run_tokio_job(
            "Saving Nexus Mods API key...",
            move || save_nexus_api_key_blocking(game_path, selected_profile_id, key_to_save),
            cx,
        );
    }

    fn handle_api_key_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cursor_blink_visible = true;
        self.cursor_blink_ticks = 0;
        let ks = &event.keystroke;
        let clipboard = if (ks.modifiers.control || ks.modifiers.platform)
            && ks.key.eq_ignore_ascii_case("v")
        {
            cx.read_from_clipboard().and_then(|i| i.text())
        } else {
            None
        };

        match apply_text_edit_keystroke(
            &mut self.api_key_input,
            &mut self.api_key_cursor,
            ks,
            clipboard,
            160,
        ) {
            TextKeyOutcome::Submit => {
                window.blur(cx);
                self.save_nexus_api_key_from_input(cx);
            }
            TextKeyOutcome::Cancel => {
                self.api_key_input = self.snapshot.nexus_api_key.clone();
                self.api_key_cursor = self.api_key_input.chars().count();
                window.blur(cx);
                cx.notify();
            }
            TextKeyOutcome::Changed | TextKeyOutcome::Moved => {
                cx.notify();
            }
            TextKeyOutcome::Ignored => {}
        }
    }

    fn paste_nexus_api_key_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let clipboard_text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
            .trim()
            .to_string();

        if clipboard_text.is_empty() {
            self.snapshot.status_message =
                "Clipboard is empty — copy your Personal API Key from Nexus Mods first"
                    .to_string();
            cx.notify();
            return;
        }

        self.api_key_input = clipboard_text.clone();
        self.api_key_cursor = self.api_key_input.chars().count();

        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.run_tokio_job(
            "Saving Nexus Mods API key from clipboard...",
            move || save_nexus_api_key_blocking(game_path, selected_profile_id, clipboard_text),
            cx,
        );
    }

    /// Evicts all decoded Discovery cover images and GPU textures from memory.
    fn clear_discover_image_cache(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.discover_cache_needs_clear = false;
        self.discover_image_cache.update(cx, |cache, cx| {
            cache.clear(window, cx);
        });
    }

    fn set_center_tab(&mut self, tab: ActiveCenterTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab == tab {
            return;
        }
        if self.active_tab == ActiveCenterTab::DiscoverMods && tab != ActiveCenterTab::DiscoverMods {
            self.clear_discover_image_cache(window, cx);
        }
        if tab == ActiveCenterTab::Settings && self.api_key_input.is_empty() {
            self.api_key_input = self.snapshot.nexus_api_key.clone();
            self.api_key_cursor = self.api_key_input.chars().count();
        }
        self.active_tab = tab;
        cx.notify();

        if tab == ActiveCenterTab::DiscoverMods
            && self
                .snapshot
                .discoverable_mods
                .iter()
                .any(|m| m.cached_cover_path.is_none())
        {
            let rt_handle = self.tokio_rt.handle().clone();
            let game_path = self.snapshot.game_install_path.clone();
            let selected_profile_id = self.snapshot.selected_profile_id;
            self.run_tokio_job(
                "Loading mod cover images from Nexus Mods...",
                move || cache_curated_covers_blocking(rt_handle, game_path, selected_profile_id),
                cx,
            );
        }
    }

    fn refresh_discovery_feed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_discover_image_cache(window, cx);
        self.discovery_page = 1;
        let rt_handle = self.tokio_rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        self.run_tokio_job(
            "Fetching live Trending & Updated Stardew Valley mods & covers from Nexus Mods...",
            move || refresh_discovery_feed_blocking(rt_handle, game_path, selected_profile_id),
            cx,
        );
    }

    fn fetch_live_nexus_page(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_discover_image_cache(window, cx);
        self.snapshot.live_nexus_page = None;
        let target_page = page.max(1);
        self.discovery_page = target_page;
        self.discovery_category = "Live Nexus".to_string();

        let rt_handle = self.tokio_rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;
        let page_size = self.discovery_page_size;
        let search = if self.discovery_search.trim().is_empty() {
            None
        } else {
            Some(self.discovery_search.clone())
        };

        self.run_tokio_job(
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
            cx,
        );
    }

    fn set_discovery_page(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        let next_page = page.max(1);
        if self.discovery_category == "Live Nexus" {
            self.fetch_live_nexus_page(next_page, window, cx);
        } else {
            self.clear_discover_image_cache(window, cx);
            self.discovery_page = next_page;
            cx.notify();
        }
    }

    fn set_discovery_category(
        &mut self,
        category: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cat = category.into();
        if cat == "Live Nexus" {
            self.fetch_live_nexus_page(1, window, cx);
        } else {
            self.clear_discover_image_cache(window, cx);
            self.discovery_category = cat;
            self.discovery_page = 1;
            cx.notify();
        }
    }

    fn handle_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cursor_blink_visible = true;
        self.cursor_blink_ticks = 0;
        let ks = &event.keystroke;
        let clipboard = if (ks.modifiers.control || ks.modifiers.platform)
            && ks.key.eq_ignore_ascii_case("v")
        {
            cx.read_from_clipboard().and_then(|i| i.text())
        } else {
            None
        };

        match apply_text_edit_keystroke(
            &mut self.discovery_search,
            &mut self.search_cursor,
            ks,
            clipboard,
            64,
        ) {
            TextKeyOutcome::Submit => {
                self.discovery_page = 1;
                if self.discovery_category == "Live Nexus" {
                    self.fetch_live_nexus_page(1, window, cx);
                } else {
                    self.clear_discover_image_cache(window, cx);
                    cx.notify();
                }
            }
            TextKeyOutcome::Cancel => {
                window.blur(cx);
                self.clear_discovery_search(window, cx);
            }
            TextKeyOutcome::Changed => {
                self.discovery_page = 1;
                if (ks.modifiers.control || ks.modifiers.platform)
                    && ks.key.eq_ignore_ascii_case("v")
                    && self.discovery_category == "Live Nexus"
                {
                    self.fetch_live_nexus_page(1, window, cx);
                } else {
                    if self.discovery_category != "Live Nexus" {
                        self.clear_discover_image_cache(window, cx);
                    }
                    cx.notify();
                }
            }
            TextKeyOutcome::Moved => {
                cx.notify();
            }
            TextKeyOutcome::Ignored => {}
        }
    }

    fn clear_discovery_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.discovery_search.clear();
        self.search_cursor = 0;
        self.discovery_page = 1;
        if self.discovery_category == "Live Nexus" {
            self.fetch_live_nexus_page(1, window, cx);
        } else {
            self.clear_discover_image_cache(window, cx);
            cx.notify();
        }
    }

    fn install_discovered_mod(
        &mut self,
        nexus_mod_id: i64,
        mod_display_name: String,
        nexus_files_url: String,
        cx: &mut Context<Self>,
    ) {
        if !self.snapshot.has_nexus_api_key {
            cx.open_url(&nexus_files_url);
            self.snapshot.status_message = format!(
                "Opened {mod_display_name} on Nexus Mods — click 'Mod Manager Download' (nxm:// is auto-bound) or set your API Key for 1-click in-app install"
            );
            cx.notify();
            return;
        }

        self.is_busy = true;
        self.snapshot.status_message =
            format!("Starting direct Nexus install for {mod_display_name} (#{nexus_mod_id})...");
        cx.notify();

        let rt = self.tokio_rt.clone();
        let rt_handle = rt.handle().clone();
        let game_path = self.snapshot.game_install_path.clone();
        let selected_profile_id = self.snapshot.selected_profile_id;

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<String>();

        self.pending_task = Some(cx.spawn(async move |view, cx| {
            let mut download_fut = cx.background_spawn(async move {
                rt.spawn_blocking(move || {
                    install_discovered_mod_blocking(
                        rt_handle,
                        game_path,
                        selected_profile_id,
                        nexus_mod_id,
                        mod_display_name,
                        progress_tx,
                    )
                })
                .await
            });

            loop {
                while let Ok(progress_msg) = progress_rx.try_recv() {
                    let _ = view.update(cx, |ws, cx| {
                        ws.snapshot.status_message = progress_msg;
                        cx.notify();
                    });
                }

                match futures_poll_once(&mut download_fut) {
                    Some(join_res) => {
                        let _ = view.update(cx, |ws, cx| {
                            ws.is_busy = false;
                            match join_res {
                                Ok(Ok(new_snapshot)) => {
                                    ws.snapshot = new_snapshot;
                                }
                                Ok(Err(err)) => {
                                    ws.snapshot.status_message = format!(
                                        "Direct Install Note: {err} (Opening Nexus files page for nxm:// download)"
                                    );
                                    cx.open_url(&nexus_files_url);
                                }
                                Err(join_err) => {
                                    ws.snapshot.status_message =
                                        format!("Install worker panicked: {join_err}");
                                }
                            }
                            cx.notify();
                        });
                        break;
                    }
                    None => {
                        cx.background_executor()
                            .timer(Duration::from_millis(80))
                            .await;
                    }
                }
            }
        }));
    }

    fn activate_selected_profile(&mut self, launch_smapi_after: bool, cx: &mut Context<Self>) {
        let Some(profile_id) = self.snapshot.selected_profile_id else {
            return;
        };
        let game_path = self.snapshot.game_install_path.clone();
        let label = if launch_smapi_after {
            "Mounting profile in Mods/, writing active_profile.json & launching SMAPI..."
        } else {
            "Mounting profile in Mods/ & writing active_profile.json..."
        };

        self.run_tokio_job(
            label,
            move || activate_profile_blocking(game_path, profile_id, launch_smapi_after),
            cx,
        );
    }

    fn current_colors(&self) -> Colors {
        if self.dark_mode {
            let mut c = Colors::dark();
            c.background = rgb(0x18181b);
            c.container = rgb(0x202024);
            c.separator = rgb(0x2a2a30);
            c.border = rgb(0x2e2e36);
            c.text = rgb(0xf4f4f5);
            c.disabled = rgb(0x94949e);
            c
        } else {
            Colors::light()
        }
    }

    fn toggle_dark_mode(&mut self, cx: &mut Context<Self>) {
        self.dark_mode = !self.dark_mode;
        let next_dark = self.dark_mode;
        let rt = self.tokio_rt.clone();
        cx.background_spawn(async move {
            let _ = rt.spawn_blocking(move || save_dark_mode_blocking(next_dark)).await;
        })
        .detach();
        cx.notify();
    }

    fn render_top_toolbar(&self, colors: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let is_settings_open = self.active_tab == ActiveCenterTab::Settings;

        div()
            .w_full()
            .h(px(64.))
            .px_5()
            .flex()
            .items_center()
            .justify_between()
            .bg(colors.container)
            .border_b_1()
            .border_color(colors.border)
            .child(
                // Left: Brand
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(colors.selected)
                            .text_color(colors.selected_text)
                            .font_weight(FontWeight::BOLD)
                            .text_sm()
                            .child("SILO"),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child("Stardew Mod & Save Vault"),
                    ),
            )
            .child(
                // Right: Dark Mode Toggle, Settings, Mount & Launch
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("toggle-dark-mode-btn")
                            .px_2p5()
                            .py_1p5()
                            .rounded_md()
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_sm()
                            .text_color(colors.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.separator))
                            .child(lucide_icon(
                                if self.dark_mode {
                                    LUCIDE_SUN
                                } else {
                                    LUCIDE_MOON
                                },
                                14.,
                                colors.text,
                            ))
                            .child(if self.dark_mode { "Light" } else { "Dark" })
                            .on_click(cx.listener(|ws, _event, _window, cx| {
                                ws.toggle_dark_mode(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("open-settings-btn")
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .bg(if is_settings_open {
                                colors.selected
                            } else {
                                colors.background
                            })
                            .border_1()
                            .border_color(if is_settings_open {
                                colors.selected
                            } else {
                                colors.border
                            })
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_sm()
                            .font_weight(if is_settings_open {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if is_settings_open {
                                colors.selected_text
                            } else {
                                colors.text
                            })
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .child(lucide_icon(
                                LUCIDE_SETTINGS,
                                14.,
                                if is_settings_open {
                                    colors.selected_text
                                } else {
                                    colors.text
                                },
                            ))
                            .child("Settings")
                            .on_click(cx.listener(|ws, _event, window, cx| {
                                if ws.active_tab == ActiveCenterTab::Settings {
                                    ws.set_center_tab(ActiveCenterTab::ProfileMods, window, cx);
                                } else {
                                    ws.set_center_tab(ActiveCenterTab::Settings, window, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("mount-only-btn")
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_sm()
                            .text_color(colors.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.separator))
                            .child(lucide_icon(LUCIDE_LAYERS, 14., colors.text))
                            .child("Mount Profile")
                            .on_click(cx.listener(|ws, _event, _window, cx| {
                                ws.activate_selected_profile(false, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("activate-launch-smapi-btn")
                            .px_4()
                            .py_1p5()
                            .rounded_md()
                            .bg(colors.selected)
                            .text_color(colors.selected_text)
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_sm()
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.92))
                            .child(lucide_icon(LUCIDE_PLAY, 14., colors.selected_text))
                            .child("Activate & Launch SMAPI")
                            .on_click(cx.listener(|ws, _event, _window, cx| {
                                ws.activate_selected_profile(true, cx);
                            })),
                    ),
            )
    }

    fn render_left_rail(&self, colors: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_profile_id = self.snapshot.selected_profile_id;
        let active_profile_id = self.snapshot.active_profile_id;
        let in_settings = self.active_tab == ActiveCenterTab::Settings;
        let green_rgba = hsla_to_rgba(green());
        let red_rgba = hsla_to_rgba(red());

        let mut save_cards = Vec::with_capacity(self.snapshot.saves.len());
        for (idx, save) in self.snapshot.saves.iter().enumerate() {
            let bound_profile = self.snapshot.profile_for_save(&save.folder_name);
            let bound_profile_id = bound_profile.map(|p| p.id);
            let is_selected =
                !in_settings && bound_profile_id.is_some() && bound_profile_id == selected_profile_id;
            let is_mounted = bound_profile_id.is_some() && bound_profile_id == active_profile_id;

            let card_border = if is_selected {
                colors.selected
            } else {
                colors.border
            };
            let card_bg = if is_selected {
                colors.separator
            } else {
                colors.background
            };

            let farmer_title = format!("{} · {} Farm", save.farmer_name, save.farm_name);
            let date_subtitle = format!(
                "Year {} · {} {} ({} days)",
                save.year, save.season, save.day_of_month, save.days_played
            );
            let folder_label = save.folder_name.clone();

            let mut card = div()
                .id(SharedString::from(format!("save-card-{idx}")))
                .w_full()
                .p_3()
                .rounded_lg()
                .bg(card_bg)
                .border_1()
                .border_color(card_border)
                .overflow_hidden()
                .flex()
                .flex_col()
                .gap_1()
                .cursor_pointer()
                .hover(|s| s.bg(colors.separator))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text)
                                .child(farmer_title),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .child(if is_mounted {
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(colors.separator)
                                        .text_color(green_rgba)
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .child("MOUNTED")
                                } else {
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(colors.separator)
                                        .text_color(colors.disabled)
                                        .text_xs()
                                        .child("SAVE")
                                })
                                .children(bound_profile_id.map(|pid| {
                                    div()
                                        .id(SharedString::from(format!("clone-save-profile-btn-{idx}")))
                                        .p_1()
                                        .rounded_sm()
                                        .bg(colors.container)
                                        .border_1()
                                        .border_color(colors.border)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(colors.separator))
                                        .child(lucide_icon(LUCIDE_COPY, 12., colors.text))
                                        .on_click(cx.listener(move |ws, _event, _window, cx| {
                                            cx.stop_propagation();
                                            ws.clone_profile(pid, cx);
                                        }))
                                })),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_xs()
                        .text_color(colors.text)
                        .child(date_subtitle),
                )
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_xs()
                        .text_color(colors.disabled)
                        .child(folder_label),
                );

            if let Some(pid) = bound_profile_id {
                card = card.on_click(cx.listener(move |ws, _event, _window, cx| {
                    ws.select_profile(pid, cx);
                }));
            }

            save_cards.push(card);
        }

        let mut standalone_cards = Vec::new();
        for (idx, profile) in self
            .snapshot
            .profiles
            .iter()
            .filter(|p| p.save_folder_name.is_none())
            .enumerate()
        {
            let pid = profile.id;
            let is_selected = !in_settings && Some(pid) == selected_profile_id;
            let is_mounted = Some(pid) == active_profile_id;

            let card = div()
                .id(SharedString::from(format!("standalone-profile-{idx}")))
                .w_full()
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(if is_selected {
                    colors.separator
                } else {
                    colors.background
                })
                .border_1()
                .border_color(if is_selected {
                    colors.selected
                } else {
                    colors.border
                })
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .hover(|s| s.bg(colors.separator))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text)
                        .child(profile.name.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(if is_mounted {
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .bg(colors.separator)
                                .text_color(green_rgba)
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .child("MOUNTED")
                        } else {
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .bg(colors.separator)
                                .text_color(colors.disabled)
                                .text_xs()
                                .child("PROFILE")
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("clone-profile-btn-{idx}")))
                                .p_1()
                                .rounded_sm()
                                .bg(colors.container)
                                .border_1()
                                .border_color(colors.border)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.separator))
                                .child(lucide_icon(LUCIDE_COPY, 12., colors.text))
                                .on_click(cx.listener(move |ws, _event, _window, cx| {
                                    cx.stop_propagation();
                                    ws.clone_profile(pid, cx);
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("delete-profile-btn-{idx}")))
                                .p_1()
                                .rounded_sm()
                                .bg(colors.separator)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.85))
                                .child(lucide_icon(LUCIDE_X, 12., red_rgba))
                                .on_click(cx.listener(move |ws, _event, _window, cx| {
                                    cx.stop_propagation();
                                    ws.delete_profile(pid, cx);
                                })),
                        ),
                )
                .on_click(cx.listener(move |ws, _event, _window, cx| {
                    ws.select_profile(pid, cx);
                }));

            standalone_cards.push(card);
        }

        div()
            .w(px(330.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(colors.container)
            .border_r_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.text)
                            .child(format!("SAVES ({})", self.snapshot.saves.len())),
                    )
                    .child(
                        div()
                            .id("rescan-saves-btn")
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(colors.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.separator))
                            .child(lucide_icon(LUCIDE_REFRESH_CW, 11., colors.text))
                            .child("Rescan")
                            .on_click(cx.listener(|ws, _event, _window, cx| {
                                ws.refresh_state(
                                    None,
                                    Some("Rescanned Saves & Vault".to_string()),
                                    cx,
                                );
                            })),
                    ),
            )
            .child(if save_cards.is_empty() {
                div()
                    .p_3()
                    .rounded_lg()
                    .bg(colors.background)
                    .border_1()
                    .border_color(colors.border)
                    .text_xs()
                    .text_color(colors.disabled)
                    .child("No Stardew Valley saves found in %APPDATA%/StardewValley/Saves.")
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(save_cards)
                    .into_any_element()
            })
            .child(
                div()
                    .mt_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.text)
                            .child(format!("PROFILES ({})", standalone_cards.len())),
                    )
                    .child(
                        div()
                            .id("new-standalone-profile-btn")
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .bg(colors.selected)
                            .text_color(colors.selected_text)
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .child("+ New Profile")
                            .on_click(cx.listener(|ws, _event, _window, cx| {
                                ws.create_new_profile(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(standalone_cards),
            )
    }

    fn render_text_input_content(
        &self,
        id_prefix: &str,
        text: &str,
        placeholder: &str,
        cursor: usize,
        is_focused: bool,
        input_kind: TextInputKind,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let chars: Vec<char> = text.chars().collect();
        let char_len = chars.len();
        let clamped_cursor = cursor.min(char_len);

        let cursor_bar = || {
            div()
                .w(px(2.))
                .h(px(14.))
                .flex_none()
                .bg(colors.selected)
                .opacity(if self.cursor_blink_visible { 1.0 } else { 0.0 })
                .into_any_element()
        };

        if chars.is_empty() {
            let mut row = div().w_full().flex().items_center();
            if is_focused {
                row = row.child(cursor_bar());
            }
            row = row.child(
                div()
                    .id(SharedString::from(format!("{id_prefix}-empty-hit")))
                    .flex_1()
                    .text_color(colors.disabled)
                    .child(placeholder.to_string())
                    .on_click(cx.listener(move |ws, _event, window, cx| {
                        ws.cursor_blink_visible = true;
                        ws.cursor_blink_ticks = 0;
                        match input_kind {
                            TextInputKind::ProfileRename => {
                                ws.profile_rename_cursor = 0;
                                window.focus(&ws.profile_rename_focus, cx);
                            }
                            TextInputKind::Search => {
                                ws.is_renaming_profile = false;
                                ws.search_cursor = 0;
                                window.focus(&ws.search_focus, cx);
                            }
                            TextInputKind::ApiKey => {
                                ws.is_renaming_profile = false;
                                ws.api_key_cursor = 0;
                                window.focus(&ws.api_key_focus, cx);
                            }
                        }
                        cx.notify();
                    })),
            );
            return row.into_any_element();
        }

        let mut children: Vec<gpui::AnyElement> = Vec::with_capacity(char_len + 2);
        for (idx, ch) in chars.into_iter().enumerate() {
            if is_focused && idx == clamped_cursor {
                children.push(cursor_bar());
            }
            let display_ch = if ch == ' ' {
                "\u{00A0}".to_string()
            } else {
                ch.to_string()
            };
            children.push(
                div()
                    .id(SharedString::from(format!("{id_prefix}-ch-{idx}")))
                    .flex_none()
                    .text_color(colors.text)
                    .child(display_ch)
                    .on_click(cx.listener(move |ws, _event, window, cx| {
                        ws.cursor_blink_visible = true;
                        ws.cursor_blink_ticks = 0;
                        match input_kind {
                            TextInputKind::ProfileRename => {
                                ws.profile_rename_cursor = idx;
                                window.focus(&ws.profile_rename_focus, cx);
                            }
                            TextInputKind::Search => {
                                ws.is_renaming_profile = false;
                                ws.search_cursor = idx;
                                window.focus(&ws.search_focus, cx);
                            }
                            TextInputKind::ApiKey => {
                                ws.is_renaming_profile = false;
                                ws.api_key_cursor = idx;
                                window.focus(&ws.api_key_focus, cx);
                            }
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }

        if is_focused && clamped_cursor == char_len {
            children.push(cursor_bar());
        }

        children.push(
            div()
                .id(SharedString::from(format!("{id_prefix}-tail-hit")))
                .flex_1()
                .min_w(px(12.))
                .h(px(16.))
                .on_click(cx.listener(move |ws, _event, window, cx| {
                    ws.cursor_blink_visible = true;
                    ws.cursor_blink_ticks = 0;
                    match input_kind {
                        TextInputKind::ProfileRename => {
                            ws.profile_rename_cursor = ws.profile_rename_input.chars().count();
                            window.focus(&ws.profile_rename_focus, cx);
                        }
                        TextInputKind::Search => {
                            ws.is_renaming_profile = false;
                            ws.search_cursor = ws.discovery_search.chars().count();
                            window.focus(&ws.search_focus, cx);
                        }
                        TextInputKind::ApiKey => {
                            ws.is_renaming_profile = false;
                            ws.api_key_cursor = ws.api_key_input.chars().count();
                            window.focus(&ws.api_key_focus, cx);
                        }
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );

        div()
            .w_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .children(children)
            .into_any_element()
    }

    fn render_center_mod_table(
        &self,
        colors: &Colors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.active_tab == ActiveCenterTab::Settings {
            return div()
                .flex_1()
                .h_full()
                .min_w_0()
                .flex()
                .flex_col()
                .bg(colors.background)
                .child(self.render_settings_body(colors, window, cx))
                .into_any_element();
        }

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
                (None, true) => {
                    "Standalone Profile · SiloGuard: Strict Lock ON".to_string()
                }
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
        let is_rename_focused =
            window.focused(cx).as_ref() == Some(&self.profile_rename_focus);
        let green_rgba = hsla_to_rgba(green());
        let red_rgba = hsla_to_rgba(red());

        let active_body = match active_tab {
            ActiveCenterTab::ProfileMods => self.render_profile_mods_body(colors, cx),
            ActiveCenterTab::DiscoverMods => self.render_discover_mods_body(colors, window, cx),
            ActiveCenterTab::Settings => unreachable!(),
        };

        div()
            .flex_1()
            .h_full()
            .min_w_0()
            .flex()
            .flex_col()
            .p_5()
            .gap_4()
            .bg(colors.background)
            .child(
                div()
                    .w_full()
                    .p_4()
                    .rounded_lg()
                    .bg(colors.container)
                    .border_1()
                    .border_color(colors.border)
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(if self.is_renaming_profile {
                                        let rename_inner = self.render_text_input_content(
                                            "profile-rename",
                                            &self.profile_rename_input,
                                            "Enter profile name...",
                                            self.profile_rename_cursor,
                                            is_rename_focused,
                                            TextInputKind::ProfileRename,
                                            colors,
                                            cx,
                                        );
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .child(
                                                div()
                                                    .id("profile-rename-input")
                                                    .w(px(220.))
                                                    .px_2p5()
                                                    .py_1()
                                                    .rounded_sm()
                                                    .bg(colors.background)
                                                    .border_1()
                                                    .border_color(if is_rename_focused {
                                                        colors.selected
                                                    } else {
                                                        colors.border
                                                    })
                                                    .text_sm()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(colors.text)
                                                    .cursor_text()
                                                    .track_focus(&self.profile_rename_focus)
                                                    .on_key_down(cx.listener(
                                                        Self::handle_profile_rename_key_down,
                                                    ))
                                                    .child(rename_inner),
                                            )
                                            .child(
                                                div()
                                                    .id("save-rename-profile-btn")
                                                    .flex_none()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(colors.selected)
                                                    .text_color(colors.selected_text)
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .cursor_pointer()
                                                    .child("Save")
                                                    .on_click(cx.listener(
                                                        |ws, _event, _window, cx| {
                                                            ws.commit_profile_rename(cx);
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id("cancel-rename-profile-btn")
                                                    .flex_none()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(colors.background)
                                                    .border_1()
                                                    .border_color(colors.border)
                                                    .text_xs()
                                                    .text_color(colors.text)
                                                    .cursor_pointer()
                                                    .child("Cancel")
                                                    .on_click(cx.listener(
                                                        |ws, _event, _window, cx| {
                                                            ws.is_renaming_profile = false;
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .into_any_element()
                                    } else {
                                        div()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_lg()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(colors.text)
                                                    .child(format!("Profile: {profile_name}")),
                                            )
                                            .child(
                                                div()
                                                    .id("rename-profile-btn")
                                                    .flex_none()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(colors.background)
                                                    .border_1()
                                                    .border_color(colors.border)
                                                    .text_xs()
                                                    .text_color(colors.text)
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(colors.separator))
                                                    .child("Rename")
                                                    .on_click(cx.listener(
                                                        |ws, _event, window, cx| {
                                                            ws.start_renaming_profile(window, cx);
                                                        },
                                                    )),
                                            )
                                            .into_any_element()
                                    })
                                    .children(
                                        self.snapshot
                                            .selected_profile()
                                            .filter(|p| p.save_folder_name.is_none())
                                            .map(|p| {
                                                let pid = p.id;
                                                div()
                                                    .id("delete-selected-profile-btn")
                                                    .flex_none()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(colors.background)
                                                    .border_1()
                                                    .border_color(colors.border)
                                                    .text_color(red_rgba)
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(colors.separator))
                                                    .child("Delete Profile")
                                                    .on_click(cx.listener(
                                                        move |ws, _event, _window, cx| {
                                                            ws.delete_profile(pid, cx);
                                                        },
                                                    ))
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.disabled)
                                    .child(save_binding_label),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2p5()
                            .child(
                                // Tab switcher: My Profile Mods vs Discover & Add Mods
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .p_1()
                                    .rounded_md()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(
                                        div()
                                            .id("tab-profile-mods")
                                            .px_3()
                                            .py_1()
                                            .rounded_sm()
                                            .bg(if active_tab == ActiveCenterTab::ProfileMods {
                                                colors.selected
                                            } else {
                                                colors.background
                                            })
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(
                                                if active_tab == ActiveCenterTab::ProfileMods {
                                                    colors.selected_text
                                                } else {
                                                    colors.text
                                                },
                                            )
                                            .cursor_pointer()
                                            .child(format!(
                                                "My Profile Mods ({enabled_count}/{total_count})"
                                            ))
                                            .on_click(cx.listener(|ws, _event, window, cx| {
                                                ws.set_center_tab(ActiveCenterTab::ProfileMods, window, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("tab-discover-mods")
                                            .px_3()
                                            .py_1()
                                            .rounded_sm()
                                            .bg(if active_tab == ActiveCenterTab::DiscoverMods {
                                                colors.selected
                                            } else {
                                                colors.background
                                            })
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(
                                                if active_tab == ActiveCenterTab::DiscoverMods {
                                                    colors.selected_text
                                                } else {
                                                    colors.text
                                                },
                                            )
                                            .cursor_pointer()
                                            .child(format!(
                                                "Discover & Add Mods ({discover_count})"
                                            ))
                                            .on_click(cx.listener(|ws, _event, window, cx| {
                                                ws.set_center_tab(
                                                    ActiveCenterTab::DiscoverMods,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    ),
                            )
                            .child(if issue_count > 0 {
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(red_rgba)
                                    .child(format!("{issue_count} Dependency Issue(s)"))
                            } else {
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(green_rgba)
                                    .child("All Dependencies Satisfied")
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .rounded_lg()
                    .bg(colors.background)
                    .border_1()
                    .border_color(colors.border)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(active_body),
            )
            .into_any_element()
    }

    fn render_profile_mods_body(
        &self,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let total_count = self.snapshot.profile_mods.len();
        let entries = self.snapshot.profile_mods.clone();
        let snapshot_for_list = self.snapshot.clone();
        let weak_ws = cx.weak_entity();

        if entries.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .p_8()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text)
                        .child("Your Silo Mod Vault is Empty"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(colors.disabled)
                        .child("Browse & install mods directly in \"Discover & Add Mods\", import a .zip/.7z archive or folder, or click \"Mod Manager Download\" on Nexus Mods (nxm:// is auto-bound)."),
                )
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .id("empty-open-discover-btn")
                                .px_4()
                                .py_2()
                                .rounded_md()
                                .bg(colors.selected)
                                .text_color(colors.selected_text)
                                .font_weight(FontWeight::BOLD)
                                .text_sm()
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.92))
                                .child("Browse & Add Mods")
                                .on_click(cx.listener(|ws, _event, window, cx| {
                                    ws.set_center_tab(ActiveCenterTab::DiscoverMods, window, cx);
                                })),
                        )
                        .child(
                            div()
                                .id("empty-import-archive-btn")
                                .px_4()
                                .py_2()
                                .rounded_md()
                                .bg(colors.container)
                                .border_1()
                                .border_color(colors.border)
                                .text_color(colors.text)
                                .text_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.separator))
                                .child("Import Archive (.zip / .7z)")
                                .on_click(cx.listener(|ws, _event, _window, cx| {
                                    ws.prompt_import_archive(cx);
                                })),
                        ),
                )
                .into_any_element()
        } else {
            let list_colors = colors.clone();
            uniform_list(
                "vault-mods-uniform-list",
                total_count,
                move |range, _window, _cx| {
                    let row_colors = list_colors.clone();
                    let green_rgba = hsla_to_rgba(green());
                    let red_rgba = hsla_to_rgba(red());
                    range
                        .map(|ix| {
                            let entry = &entries[ix];
                            let mod_id = entry.mod_record.id;
                            let enabled = entry.enabled;
                            let mod_name = entry.mod_record.name.clone();
                            let unique_id = entry.mod_record.unique_id.clone();
                            let version = format!("v{}", entry.mod_record.version);
                            let mod_issues = snapshot_for_list.issues_for_mod(&unique_id);
                            let mod_update = snapshot_for_list.update_for_mod(&unique_id);
                            let ws_handle = weak_ws.clone();

                            let mut badges = Vec::with_capacity(mod_issues.len() + 1);

                            if let Some(update_info) = mod_update {
                                let update_url = update_info.nexus_url();
                                let update_label = format!(
                                    "Update Available: v{} ↗",
                                    update_info.latest_version
                                );
                                badges.push(
                                    div()
                                        .id(SharedString::from(format!("update-badge-{ix}")))
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(row_colors.selected)
                                        .text_color(row_colors.selected_text)
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .cursor_pointer()
                                        .hover(|s| s.opacity(0.9))
                                        .child(update_label)
                                        .on_click(move |_event, _window, cx| {
                                            cx.open_url(&update_url);
                                        }),
                                );
                            }

                            for (b_idx, issue) in mod_issues.into_iter().enumerate() {
                                let label = match &issue.kind {
                                    DependencyIssueKind::Missing => match &issue.minimum_version {
                                        Some(min) => format!(
                                            "Missing: {} (>={min}) ↗",
                                            issue.required_unique_id
                                        ),
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
                                };
                                let nexus_url = issue.nexus_url();

                                badges.push(
                                    div()
                                        .id(SharedString::from(format!("dep-badge-{ix}-{b_idx}")))
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(row_colors.container)
                                        .border_1()
                                        .border_color(row_colors.border)
                                        .text_color(red_rgba)
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(row_colors.separator))
                                        .child(label)
                                        .on_click(move |_event, _window, cx| {
                                            cx.open_url(&nexus_url);
                                        }),
                                );
                            }

                            div()
                                .id(SharedString::from(format!("mod-row-{ix}")))
                                .w_full()
                                .px_4()
                                .py_2p5()
                                .border_b_1()
                                .border_color(row_colors.border)
                                .flex()
                                .items_center()
                                .justify_between()
                                .hover(|s| s.bg(row_colors.container))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .id(SharedString::from(format!("mod-toggle-{ix}")))
                                                .px_2p5()
                                                .py_1()
                                                .rounded_md()
                                                .bg(row_colors.container)
                                                .border_1()
                                                .border_color(row_colors.border)
                                                .text_color(if enabled {
                                                    green_rgba
                                                } else {
                                                    row_colors.disabled
                                                })
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .cursor_pointer()
                                                .child(if enabled { "ENABLED" } else { "DISABLED" })
                                                .on_click(move |_event, _window, cx| {
                                                    let _ = ws_handle.update(cx, |ws, cx| {
                                                        ws.toggle_mod(mod_id, enabled, cx);
                                                    });
                                                }),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(row_colors.text)
                                                        .child(mod_name),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(row_colors.disabled)
                                                        .child(unique_id),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .children(badges)
                                        .child(
                                            div()
                                                .px_2()
                                                .py_0p5()
                                                .rounded_sm()
                                                .bg(row_colors.container)
                                                .border_1()
                                                .border_color(row_colors.border)
                                                .text_xs()
                                                .text_color(row_colors.text)
                                                .child(version),
                                        ),
                                )
                        })
                        .collect()
                },
            )
            .track_scroll(&self.mod_scroll)
            .w_full()
            .flex_1()
            .min_h_0()
            .into_any_element()
        }
    }

    fn render_discover_mods_body(
        &self,
        colors: &Colors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_search_focused = window.focused(cx).as_ref() == Some(&self.search_focus);
        let red_rgba = hsla_to_rgba(red());
        let page_result = if self.discovery_category == "Live Nexus" {
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
        };

        let items = page_result.mods;
        let item_count = items.len();
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
            (start_idx + item_count - 1).min(total_count)
        };

        let snapshot_for_list = self.snapshot.clone();
        let weak_ws = cx.weak_entity();
        let has_api_key = self.snapshot.has_nexus_api_key;
        let list_colors = colors.clone();
        let discover_image_cache = self.discover_image_cache.clone();

        let list_el = uniform_list(
            "discover-mods-uniform-list",
            item_count,
            move |range, _window, _cx| {
                let row_colors = list_colors.clone();
                let green_rgba = hsla_to_rgba(green());
                range
                    .map(|ix| {
                        let item = &items[ix];
                        let nexus_mod_id = item.nexus_mod_id;
                        let name = item.name.clone();
                        let name_for_install = item.name.clone();
                        let summary = item.summary.clone();
                        let author = format!("by {}", item.author);
                        let version = format!("v{}", item.version);
                        let category = item.category.clone();
                        let category_short = item.category.clone();
                        let cover_path = item.cached_cover_path.clone();
                        let nexus_files_url = item.nexus_files_url();
                        let nexus_page_url = item.nexus_page_url();
                        let installed = snapshot_for_list
                            .installed_vault_mod(item)
                            .map(|m| m.version.clone());
                        let ws_handle = weak_ws.clone();

                        let install_btn_label = if installed.is_some() {
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

                        let cover_element = if let Some(path) = cover_path {
                            div()
                                .w(px(108.))
                                .h(px(60.))
                                .flex_none()
                                .rounded_md()
                                .overflow_hidden()
                                .bg(row_colors.container)
                                .border_1()
                                .border_color(row_colors.border)
                                .child(
                                    img(path)
                                        .image_cache(&discover_image_cache)
                                        .w(px(108.))
                                        .h(px(60.))
                                        .object_fit(ObjectFit::Cover),
                                )
                                .into_any_element()
                        } else {
                            div()
                                .w(px(108.))
                                .h(px(60.))
                                .flex_none()
                                .rounded_md()
                                .overflow_hidden()
                                .bg(row_colors.container)
                                .border_1()
                                .border_color(row_colors.border)
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(row_colors.selected)
                                        .child(category_short),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(row_colors.disabled)
                                        .child(format!("#{nexus_mod_id}")),
                                )
                                .into_any_element()
                        };

                        div()
                            .id(SharedString::from(format!("discover-row-{ix}")))
                            .w_full()
                            .px_4()
                            .py_2p5()
                            .border_b_1()
                            .border_color(row_colors.border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3p5()
                            .hover(|s| s.bg(row_colors.container))
                            .child(cover_element)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(row_colors.container)
                                                    .border_1()
                                                    .border_color(row_colors.border)
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(row_colors.selected)
                                                    .child(category),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(row_colors.text)
                                                    .child(name),
                                            )
                                            .child(
                                                div()
                                                    .px_1p5()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(row_colors.container)
                                                    .text_xs()
                                                    .text_color(row_colors.text)
                                                    .child(version),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(row_colors.disabled)
                                                    .child(author),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(row_colors.text)
                                            .child(summary),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .children(installed.map(|ver| {
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .bg(row_colors.container)
                                            .border_1()
                                            .border_color(row_colors.border)
                                            .text_color(green_rgba)
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .child(format!("IN VAULT (v{ver})"))
                                    }))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "discover-install-btn-{ix}"
                                            )))
                                            .px_3()
                                            .py_1p5()
                                            .rounded_md()
                                            .bg(row_colors.selected)
                                            .text_color(row_colors.selected_text)
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .cursor_pointer()
                                            .hover(|s| s.opacity(0.9))
                                            .child(install_btn_label)
                                            .on_click(move |_event, _window, cx| {
                                                let mod_title = name_for_install.clone();
                                                let files_url = nexus_files_url.clone();
                                                let _ = ws_handle.update(cx, |ws, cx| {
                                                    ws.install_discovered_mod(
                                                        nexus_mod_id,
                                                        mod_title,
                                                        files_url,
                                                        cx,
                                                    );
                                                });
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "discover-page-btn-{ix}"
                                            )))
                                            .px_2p5()
                                            .py_1p5()
                                            .rounded_md()
                                            .bg(row_colors.background)
                                            .border_1()
                                            .border_color(row_colors.border)
                                            .text_xs()
                                            .text_color(row_colors.text)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(row_colors.separator))
                                            .child("Nexus Page ↗")
                                            .on_click(move |_event, _window, cx| {
                                                cx.open_url(&nexus_page_url);
                                            }),
                                    ),
                            )
                    })
                    .collect()
            },
        )
        .track_scroll(&self.discover_scroll)
        .w_full()
        .flex_1()
        .min_h_0();

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

        let mut category_pills = Vec::with_capacity(categories.len());
        for (cat_idx, (cat_key, cat_label)) in categories.into_iter().enumerate() {
            let is_active = self.discovery_category.eq_ignore_ascii_case(cat_key);
            let cat_key_owned = cat_key.to_string();

            category_pills.push(
                div()
                    .id(SharedString::from(format!("discover-cat-pill-{cat_idx}")))
                    .px_2p5()
                    .py_1()
                    .rounded_sm()
                    .bg(if is_active {
                        colors.selected
                    } else {
                        colors.background
                    })
                    .border_1()
                    .border_color(colors.border)
                    .text_xs()
                    .font_weight(if is_active {
                        FontWeight::BOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if is_active {
                        colors.selected_text
                    } else {
                        colors.text
                    })
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.9))
                    .child(cat_label)
                    .on_click(cx.listener(move |ws, _event, window, cx| {
                        ws.set_discovery_category(cat_key_owned.clone(), window, cx);
                    })),
            );
        }

        let has_prev = current_page > 1;
        let has_next = current_page < total_pages;
        let prev_page = current_page.saturating_sub(1).max(1);
        let next_page = (current_page + 1).min(total_pages);

        let search_inner = self.render_text_input_content(
            "discover-search",
            &self.discovery_search,
            "Search mods...",
            self.search_cursor,
            is_search_focused,
            TextInputKind::Search,
            colors,
            cx,
        );

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(
                // Top filter & search bar
                div()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .bg(colors.container)
                    .border_b_1()
                    .border_color(colors.border)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .children(category_pills),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("discover-search-input")
                                    .w(px(210.))
                                    .px_2p5()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(if is_search_focused {
                                        colors.selected
                                    } else {
                                        colors.border
                                    })
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .text_xs()
                                    .text_color(colors.text)
                                    .cursor_text()
                                    .hover(|s| s.border_color(colors.selected))
                                    .track_focus(&self.search_focus)
                                    .on_key_down(cx.listener(Self::handle_search_key_down))
                                    .on_click(cx.listener(|ws, _event, window, cx| {
                                        ws.is_renaming_profile = false;
                                        ws.cursor_blink_visible = true;
                                        ws.cursor_blink_ticks = 0;
                                        window.focus(&ws.search_focus, cx);
                                        cx.notify();
                                    }))
                                    .child(lucide_icon(LUCIDE_SEARCH, 12., colors.disabled))
                                    .child(div().flex_1().min_w_0().child(search_inner)),
                            )
                            .children((!self.discovery_search.is_empty()).then(|| {
                                div()
                                    .id("discover-search-clear-btn")
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .text_color(red_rgba)
                                    .text_xs()
                                    .cursor_pointer()
                                    .child("Clear")
                                    .on_click(cx.listener(|ws, _event, window, cx| {
                                        ws.clear_discovery_search(window, cx);
                                    }))
                            }))
                            .child(
                                div()
                                    .id("refresh-nexus-feed-btn")
                                    .px_2p5()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.separator))
                                    .child(lucide_icon(LUCIDE_REFRESH_CW, 12., colors.text))
                                    .child("Sync Trending")
                                    .on_click(cx.listener(|ws, _event, window, cx| {
                                        ws.refresh_discovery_feed(window, cx);
                                    })),
                            ),
                    ),
            )
            .child(list_el)
            .child(
                // Bottom Pagination Controls Bar
                div()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .bg(colors.container)
                    .border_t_1()
                    .border_color(colors.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.text)
                            .child(format!(
                                "{} · Showing {start_idx}–{end_idx} of {total_count} mods",
                                page_result.source_label
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("discover-page-first-btn")
                                    .px_2p5()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .text_xs()
                                    .text_color(if has_prev {
                                        colors.text
                                    } else {
                                        colors.disabled
                                    })
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.separator))
                                    .child("« First")
                                    .on_click(cx.listener(move |ws, _event, window, cx| {
                                        if has_prev {
                                            ws.set_discovery_page(1, window, cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id("discover-page-prev-btn")
                                    .px_3()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if has_prev {
                                        colors.text
                                    } else {
                                        colors.disabled
                                    })
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.separator))
                                    .child("‹ Prev Page")
                                    .on_click(cx.listener(move |ws, _event, window, cx| {
                                        if has_prev {
                                            ws.set_discovery_page(prev_page, window, cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.selected)
                                    .child(format!("Page {current_page} / {total_pages}")),
                            )
                            .child(
                                div()
                                    .id("discover-page-next-btn")
                                    .px_3()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if has_next {
                                        colors.text
                                    } else {
                                        colors.disabled
                                    })
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.separator))
                                    .child("Next Page ›")
                                    .on_click(cx.listener(move |ws, _event, window, cx| {
                                        if has_next {
                                            ws.set_discovery_page(next_page, window, cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id("discover-page-last-btn")
                                    .px_2p5()
                                    .py_1()
                                    .rounded_sm()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .text_xs()
                                    .text_color(if has_next {
                                        colors.text
                                    } else {
                                        colors.disabled
                                    })
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.separator))
                                    .child("Last »")
                                    .on_click(cx.listener(move |ws, _event, window, cx| {
                                        if has_next {
                                            ws.set_discovery_page(total_pages, window, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_settings_body(
        &self,
        colors: &Colors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let strict_lock_on = self.snapshot.strict_lock;
        let has_api_key = self.snapshot.has_nexus_api_key;
        let is_api_key_focused =
            window.focused(cx).as_ref() == Some(&self.api_key_focus);
        let green_rgba = hsla_to_rgba(green());

        let saves_dir_display = self
            .snapshot
            .saves_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Not found".to_string());

        let api_key_inner = self.render_text_input_content(
            "settings-api-key",
            &self.api_key_input,
            "Enter or Ctrl+V paste your Nexus Mods Personal API Key...",
            self.api_key_cursor,
            is_api_key_focused,
            TextInputKind::ApiKey,
            colors,
            cx,
        );

        let storage_row = |label: &str, value: String| {
            div()
                .w_full()
                .py_2()
                .border_t_1()
                .border_color(colors.border)
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.disabled)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .px_2p5()
                        .py_1()
                        .rounded_sm()
                        .bg(colors.background)
                        .border_1()
                        .border_color(colors.border)
                        .text_xs()
                        .text_color(colors.text)
                        .child(value),
                )
        };

        div()
            .id("settings-scroll-container")
            .flex_1()
            .min_h_0()
            .w_full()
            .p_6()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    // Header
                    .child(
                        div()
                            .pb_3()
                            .border_b_1()
                            .border_color(colors.border)
                            .flex()
                            .items_center()
                            .gap_2p5()
                            .child(lucide_icon(LUCIDE_SETTINGS, 18., colors.text))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(colors.text)
                                            .child("Settings"),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .text_color(colors.disabled)
                                            .child(
                                                "Manage Nexus Mods API access, SiloGuard save lock, and local mod imports.",
                                            ),
                                    ),
                            ),
                    )
                    // 1. Nexus Mods Personal API Key (Interactive Input + Paste + Save)
                    .child(
                        div()
                            .w_full()
                            .p_4()
                            .rounded_lg()
                            .bg(colors.container)
                            .border_1()
                            .border_color(colors.border)
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(lucide_icon(LUCIDE_KEY, 15., colors.text))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(colors.text)
                                                    .child("Nexus Mods Personal API Key"),
                                            )
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(colors.background)
                                                    .border_1()
                                                    .border_color(colors.border)
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(if has_api_key {
                                                        green_rgba
                                                    } else {
                                                        colors.disabled
                                                    })
                                                    .child(if has_api_key {
                                                        "CONNECTED"
                                                    } else {
                                                        "NOT SET"
                                                    }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("settings-get-api-key-btn")
                                            .flex_none()
                                            .px_2p5()
                                            .py_1()
                                            .rounded_md()
                                            .bg(colors.background)
                                            .border_1()
                                            .border_color(colors.border)
                                            .text_xs()
                                            .text_color(colors.text)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(colors.separator))
                                            .child("Get API Key on Nexus ↗")
                                            .on_click(|_event, _window, cx| {
                                                cx.open_url(
                                                    "https://next.nexusmods.com/settings/api-keys",
                                                );
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.disabled)
                                    .child(
                                        "Type or press Ctrl+V inside the box below to set your Personal API Key for 1-click nxm:// downloads and update checks.",
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("settings-api-key-input")
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .px_3()
                                            .py_1p5()
                                            .rounded_md()
                                            .bg(colors.background)
                                            .border_1()
                                            .border_color(if is_api_key_focused {
                                                colors.selected
                                            } else {
                                                colors.border
                                            })
                                            .text_xs()
                                            .text_color(colors.text)
                                            .cursor_text()
                                            .hover(|s| s.border_color(colors.selected))
                                            .track_focus(&self.api_key_focus)
                                            .on_key_down(
                                                cx.listener(Self::handle_api_key_key_down),
                                            )
                                            .on_click(cx.listener(|ws, _event, window, cx| {
                                                ws.is_renaming_profile = false;
                                                ws.cursor_blink_visible = true;
                                                ws.cursor_blink_ticks = 0;
                                                window.focus(&ws.api_key_focus, cx);
                                                cx.notify();
                                            }))
                                            .child(api_key_inner),
                                    )
                                    .child(
                                        div()
                                            .id("settings-paste-api-key-btn")
                                            .flex_none()
                                            .px_3()
                                            .py_1p5()
                                            .rounded_md()
                                            .bg(colors.background)
                                            .border_1()
                                            .border_color(colors.border)
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.text)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(colors.separator))
                                            .child("Paste")
                                            .on_click(cx.listener(|ws, _event, _window, cx| {
                                                ws.paste_nexus_api_key_from_clipboard(cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("settings-save-api-key-btn")
                                            .flex_none()
                                            .px_3()
                                            .py_1p5()
                                            .rounded_md()
                                            .bg(colors.selected)
                                            .text_color(colors.selected_text)
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .cursor_pointer()
                                            .hover(|s| s.opacity(0.9))
                                            .child(lucide_icon(
                                                LUCIDE_CHECK,
                                                12.,
                                                colors.selected_text,
                                            ))
                                            .child("Save Key")
                                            .on_click(cx.listener(|ws, _event, _window, cx| {
                                                ws.save_nexus_api_key_from_input(cx);
                                            })),
                                    ),
                            ),
                    )
                    // 2. SiloGuard Strictness Section
                    .child(
                        div()
                            .w_full()
                            .p_4()
                            .rounded_lg()
                            .bg(colors.container)
                            .border_1()
                            .border_color(colors.border)
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(lucide_icon(LUCIDE_SHIELD, 15., colors.text))
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_sm()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(colors.text)
                                                    .child("SiloGuard Save Protection (Global)"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .p_1()
                                            .rounded_md()
                                            .bg(colors.background)
                                            .border_1()
                                            .border_color(colors.border)
                                            .child(
                                                div()
                                                    .id("settings-strict-lock-on-btn")
                                                    .px_3()
                                                    .py_1()
                                                    .rounded_sm()
                                                    .bg(if strict_lock_on {
                                                        colors.selected
                                                    } else {
                                                        colors.background
                                                    })
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(if strict_lock_on {
                                                        colors.selected_text
                                                    } else {
                                                        colors.text
                                                    })
                                                    .cursor_pointer()
                                                    .child("Strict Lock ON")
                                                    .on_click(cx.listener(|ws, _event, _window, cx| {
                                                        ws.set_strict_lock(true, cx);
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("settings-strict-lock-warn-btn")
                                                    .px_3()
                                                    .py_1()
                                                    .rounded_sm()
                                                    .bg(if !strict_lock_on {
                                                        colors.selected
                                                    } else {
                                                        colors.background
                                                    })
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(if !strict_lock_on {
                                                        colors.selected_text
                                                    } else {
                                                        colors.text
                                                    })
                                                    .cursor_pointer()
                                                    .child("Warn Only")
                                                    .on_click(cx.listener(|ws, _event, _window, cx| {
                                                        ws.set_strict_lock(false, cx);
                                                    })),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.disabled)
                                    .child(
                                        "Blocks loading a mismatched save in Stardew Valley when Strict Lock is active.",
                                    ),
                            ),
                    )
                    // 3. Import Local Mods into Vault
                    .child(
                        div()
                            .w_full()
                            .p_4()
                            .rounded_lg()
                            .bg(colors.container)
                            .border_1()
                            .border_color(colors.border)
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(lucide_icon(
                                                LUCIDE_FOLDER_PLUS,
                                                15.,
                                                colors.text,
                                            ))
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_sm()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(colors.text)
                                                    .child("Import Local Mods into Vault"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .id("settings-import-archive-btn")
                                                    .px_3()
                                                    .py_1p5()
                                                    .rounded_md()
                                                    .bg(colors.selected)
                                                    .text_color(colors.selected_text)
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .cursor_pointer()
                                                    .hover(|s| s.opacity(0.9))
                                                    .child("Import Archive (.zip / .7z)")
                                                    .on_click(cx.listener(|ws, _event, _window, cx| {
                                                        ws.prompt_import_archive(cx);
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("settings-import-folder-btn")
                                                    .px_3()
                                                    .py_1p5()
                                                    .rounded_md()
                                                    .bg(colors.background)
                                                    .border_1()
                                                    .border_color(colors.border)
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(colors.text)
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(colors.separator))
                                                    .child("Import Folder")
                                                    .on_click(cx.listener(|ws, _event, _window, cx| {
                                                        ws.prompt_import_folder(cx);
                                                    })),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.disabled)
                                    .child(
                                        "Add downloaded .zip/.7z archives or unpacked SMAPI mod folders into your central vault.",
                                    ),
                            ),
                    )
                    // 4. Storage Paths Summary
                    .child(
                        div()
                            .w_full()
                            .p_4()
                            .rounded_lg()
                            .bg(colors.container)
                            .border_1()
                            .border_color(colors.border)
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(lucide_icon(LUCIDE_HARD_DRIVE, 15., colors.text))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(colors.text)
                                            .child("Storage Locations"),
                                    ),
                            )
                            .child(storage_row(
                                "Central Mod Vault",
                                self.snapshot.vault_dir.display().to_string(),
                            ))
                            .child(storage_row(
                                "SQLite Database",
                                self.snapshot.db_path.display().to_string(),
                            ))
                            .child(storage_row("Stardew Saves Folder", saves_dir_display)),
                    ),
            )
            .into_any_element()
    }

    fn render_footer(&self, colors: &Colors) -> impl IntoElement {
        div()
            .w_full()
            .h(px(28.))
            .px_5()
            .flex()
            .items_center()
            .bg(colors.container)
            .border_t_1()
            .border_color(colors.border)
            .child(
                div()
                    .w_full()
                    .truncate()
                    .text_xs()
                    .text_color(if self.is_busy {
                        colors.selected
                    } else {
                        colors.text
                    })
                    .child(self.snapshot.status_message.clone()),
            )
    }
}

impl Render for SiloWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.discover_cache_needs_clear {
            self.clear_discover_image_cache(window, cx);
        }
        let colors = self.current_colors();
        let show_footer = !self.snapshot.status_message.trim().is_empty();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.text)
            .child(self.render_top_toolbar(&colors, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_row()
                    .child(self.render_left_rail(&colors, cx))
                    .child(self.render_center_mod_table(&colors, window, cx)),
            )
            .children(show_footer.then(|| self.render_footer(&colors)))
    }
}

/// Polls a pinned or unpinned future once without blocking.
fn futures_poll_once<F: std::future::Future + Unpin>(fut: &mut F) -> Option<F::Output> {
    use std::pin::Pin;
    use std::task::{Context as TaskContext, Poll, RawWaker, RawWakerVTable, Waker};

    fn noop_clone(_: *const ()) -> RawWaker {
        noop_raw_waker()
    }
    fn noop(_: *const ()) {}

    const VTABLE: RawWakerVTable = RawWakerVTable::new(noop_clone, noop, noop, noop);
    fn noop_raw_waker() -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }

    let waker = unsafe { Waker::from_raw(noop_raw_waker()) };
    let mut cx = TaskContext::from_waker(&waker);
    match Pin::new(fut).poll(&mut cx) {
        Poll::Ready(val) => Some(val),
        Poll::Pending => None,
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Auto-bind the OS `nxm://` protocol handler to this executable on every startup
    if let Ok(current_exe) = std::env::current_exe() {
        let _ = register_nxm_protocol_handler(&current_exe);
    }

    // Single-Instance IPC Check:
    // If launched via an `nxm://...` protocol link and an existing Silo window is already running,
    // forward the URI over `interprocess::local_socket` and exit immediately without opening a duplicate window.
    let cli_nxm_arg = extract_nxm_arg_from_env();
    if let Some(ref nxm_uri) = cli_nxm_arg {
        if let Ok(true) = try_forward_nxm_to_running_instance(nxm_uri) {
            return;
        }
    }

    let (ipc_tx, ipc_rx) = mpsc::unbounded_channel::<String>();
    let _ = spawn_nxm_ipc_listener(ipc_tx.clone());

    let ipc_rx_holder = Arc::new(Mutex::new(Some(ipc_rx)));
    let initial_nxm: Vec<String> = cli_nxm_arg.into_iter().collect();

    let app = gpui_platform::application().with_quit_mode(QuitMode::LastWindowClosed);

    // Handle macOS / OS URL scheme open events (`Application::on_open_urls`)
    let url_tx = ipc_tx.clone();
    app.on_open_urls(move |urls| {
        for url in urls {
            if url.starts_with("nxm://") || url.starts_with("NXM://") {
                let _ = url_tx.send(url);
            }
        }
    });

    app.run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Silo — Stardew Valley Mod & Save Manager")),
                ..Default::default()
            }),
            ..Default::default()
        };

        let rx = ipc_rx_holder
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())
            .unwrap_or_else(|| mpsc::unbounded_channel().1);
        let startup_nxm = initial_nxm.clone();

        cx.open_window(options, move |window, cx| {
            cx.new(|cx| SiloWorkspace::new(startup_nxm, rx, window, cx))
        })
        .expect("failed to open Silo GPUI window");

        cx.activate(true);
    });
}
