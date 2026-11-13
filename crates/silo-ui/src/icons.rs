use egui::{Color32, Response, Ui};

fastframe_icons::icons! {
    /// Every icon the interface draws.
    pub enum Icon {
        prefix: "silo-icon-",
        directory: "../assets/icons/",

        Check => lucide "check",
        Copy => lucide "copy",
        FolderPlus => "folder-plus",
        HardDrive => "hard-drive",
        Key => "key",
        Layers => "layers",
        Moon => lucide "moon",
        Play => lucide "play",
        Refresh => lucide "refresh-cw",
        Search => lucide "search",
        Settings => lucide "settings",
        Shield => "shield",
        Sun => lucide "sun",
        X => lucide "x",
    }
}

/// A frameless icon button in the app's own size and tint.
pub fn icon_button(ui: &mut Ui, icon: Icon, tint: Color32) -> Response {
    ui.add(
        egui::Button::image(icon.image(tint, 14.0))
            .frame(false)
            .min_size(egui::vec2(22.0, 22.0)),
    )
}
