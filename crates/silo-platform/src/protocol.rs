#[cfg(not(target_os = "windows"))]
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use tracing::info;

/// Registers the `nxm://` URI scheme handler for the current user so clicking
/// "Mod Manager Download" on Nexus Mods invokes `silo "<nxm_uri>"`.
///
/// - **Windows**: Writes `HKCU\Software\Classes\nxm` and `HKCU\Software\Classes\nxm\shell\open\command`
///   (no Administrator privileges required).
/// - **Linux**: Writes `~/.local/share/applications/silo-nxm.desktop` and registers it via
///   `xdg-mime default silo-nxm.desktop x-scheme-handler/nxm`.
/// - **macOS**: Writes a companion `Info.plist` next to the binary if running inside a bundle
///   (and works alongside GPUI's `Application::on_open_urls`).
pub fn register_nxm_protocol_handler(exe_path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        register_nxm_windows(exe_path)
    }

    #[cfg(target_os = "linux")]
    {
        register_nxm_linux(exe_path)
    }

    #[cfg(target_os = "macos")]
    {
        register_nxm_macos(exe_path)
    }
}

#[cfg(target_os = "windows")]
fn register_nxm_windows(exe_path: &Path) -> io::Result<()> {
    let exe_str = exe_path.display().to_string();
    let open_command = format!("\"{exe_str}\" \"%1\"");

    // 1. Set HKCU\Software\Classes\nxm (Default) = "URL:NXM Protocol"
    let status1 = Command::new("reg")
        .args([
            "add",
            r"HKCU\Software\Classes\nxm",
            "/ve",
            "/t",
            "REG_SZ",
            "/d",
            "URL:NXM Protocol",
            "/f",
        ])
        .status()?;

    if !status1.success() {
        return Err(io::Error::other(
            "reg.exe failed to set HKCU\\Software\\Classes\\nxm default value",
        ));
    }

    // 2. Set HKCU\Software\Classes\nxm "URL Protocol" = ""
    let status2 = Command::new("reg")
        .args([
            "add",
            r"HKCU\Software\Classes\nxm",
            "/v",
            "URL Protocol",
            "/t",
            "REG_SZ",
            "/d",
            "",
            "/f",
        ])
        .status()?;

    if !status2.success() {
        return Err(io::Error::other(
            "reg.exe failed to set URL Protocol value on HKCU\\Software\\Classes\\nxm",
        ));
    }

    // 3. Set HKCU\Software\Classes\nxm\shell\open\command (Default) = "\"<exe>\" \"%1\""
    let status3 = Command::new("reg")
        .args([
            "add",
            r"HKCU\Software\Classes\nxm\shell\open\command",
            "/ve",
            "/t",
            "REG_SZ",
            "/d",
            &open_command,
            "/f",
        ])
        .status()?;

    if !status3.success() {
        return Err(io::Error::other(
            "reg.exe failed to set HKCU\\Software\\Classes\\nxm\\shell\\open\\command",
        ));
    }

    info!(exe = %exe_str, "Registered nxm:// protocol handler in HKCU\\Software\\Classes\\nxm");
    Ok(())
}

#[cfg(target_os = "linux")]
fn register_nxm_linux(exe_path: &Path) -> io::Result<()> {
    let home = dirs::home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Home directory not found"))?;
    let apps_dir = home.join(".local").join("share").join("applications");
    fs::create_dir_all(&apps_dir)?;

    let desktop_path = apps_dir.join("silo-nxm.desktop");
    let desktop_contents = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Silo Stardew Mod Manager\n\
         Comment=Handle Nexus Mods nxm:// links with Silo\n\
         Exec=\"{}\" %u\n\
         Terminal=false\n\
         NoDisplay=true\n\
         MimeType=x-scheme-handler/nxm;\n\
         Categories=Game;Utility;\n",
        exe_path.display()
    );
    fs::write(&desktop_path, desktop_contents)?;

    let _ = Command::new("xdg-mime")
        .args(["default", "silo-nxm.desktop", "x-scheme-handler/nxm"])
        .status();

    let _ = Command::new("update-desktop-database")
        .arg(&apps_dir)
        .status();

    info!(desktop_file = %desktop_path.display(), "Registered nxm:// protocol handler via xdg-mime");
    Ok(())
}

#[cfg(target_os = "macos")]
fn register_nxm_macos(exe_path: &Path) -> io::Result<()> {
    let _ = fs:: metadata(exe_path)?;
    let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
        .args(["-f", &exe_path.display().to_string()])
        .status();
    info!("Registered macOS LaunchServices URL handler for nxm://");
    Ok(())
}

/// Generates the macOS `Info.plist` `CFBundleURLTypes` XML snippet for `nxm://` support.
pub fn macos_info_plist_url_types_snippet() -> &'static str {
    r#"<key>CFBundleURLTypes</key>
<array>
    <dict>
        <key>CFBundleURLName</key>
        <dict>com.cendy.silo.nxm</dict>
        <key>CFBundleURLSchemes</key>
        <array>
            <string>nxm</string>
        </array>
    </dict>
</array>"#
}
