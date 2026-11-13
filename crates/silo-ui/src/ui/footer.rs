use egui::{RichText, Ui};

use crate::app::SiloApp;
use crate::theme;

pub(crate) fn show(app: &SiloApp, ui: &mut Ui) {
    let palette = app.palette;
    let color = if app.is_busy {
        palette.selected
    } else {
        palette.text
    };

    ui.horizontal_centered(|ui| {
        ui.add(
            egui::Label::new(
                RichText::new(app.snapshot.status_message.clone())
                    .font(theme::regular(11.0))
                    .color(color),
            )
            .truncate(),
        );
    });
}
