mod app;
mod icons;
mod state;
mod theme;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use fastframe_instance::{Claim, Slot};
use silo_platform::{extract_nxm_arg_from_env, register_nxm_protocol_handler};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Auto-bind the OS `nxm://` protocol handler to this executable on every startup.
    if let Ok(current_exe) = std::env::current_exe() {
        let _ = register_nxm_protocol_handler(&current_exe);
    }

    let initial_nxm = extract_nxm_arg_from_env();
    let request = initial_nxm.clone().unwrap_or_else(|| "show".to_string());

    let (nxm_tx, nxm_rx) = std::sync::mpsc::channel::<String>();
    let waker: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());

    let claim = Slot::new("com.cendy.silo").claim(&request, {
        let nxm_tx = nxm_tx.clone();
        let waker = waker.clone();
        move |request: &str| {
            let is_nxm = request.starts_with("nxm://") || request.starts_with("NXM://");
            if is_nxm {
                let _ = nxm_tx.send(request.to_string());
            } else if request != "show" {
                return None;
            }
            if let Some(ctx) = waker.get() {
                ctx.request_repaint();
            }
            Some("ok".to_string())
        }
    });

    let instance_guard = match claim {
        Claim::First(guard) => guard,
        Claim::Running(_) | Claim::Declined | Claim::Unanswered => return,
    };

    fastframe_titlebar::init();
    fastframe_titlebar::set_enabled(false);
    let rendering = fastframe_text::detect();

    let nxm_rx_cell = Rc::new(RefCell::new(Some(nxm_rx)));

    let native_options = || eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Silo — Stardew Valley Mod & Save Manager")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([960.0, 600.0])
            .with_fullsize_content_view(cfg!(target_os = "macos"))
            .with_titlebar_shown(!cfg!(target_os = "macos"))
            .with_title_shown(!cfg!(target_os = "macos"))
            .with_decorations(!fastframe_titlebar::enabled())
            .with_app_id("com.cendy.silo"),
        ..Default::default()
    };

    let app_creator = || -> eframe::AppCreator<'static> {
        let nxm_rx = nxm_rx_cell.clone();
        let initial_nxm = initial_nxm.clone();
        let waker = waker.clone();
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx, rendering);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            fastframe_icons::install::<icons::Icon>(&cc.egui_ctx);
            fastframe_titlebar::install(&cc.egui_ctx);
            let _ = waker.set(cc.egui_ctx.clone());

            let rx = nxm_rx.borrow_mut().take().unwrap_or_else(|| {
                let (_tx, rx) = std::sync::mpsc::channel();
                rx
            });

            Ok(
                Box::new(app::SiloApp::new(cc, rx, initial_nxm.clone(), rendering))
                    as Box<dyn eframe::App>,
            )
        })
    };

    let result = fastframe_graphics::Graphics::new("Silo").run_native(native_options, app_creator);
    drop(instance_guard);

    if let Err(err) = result {
        eprintln!("Silo failed to open a window: {err}");
        std::process::exit(1);
    }
}
