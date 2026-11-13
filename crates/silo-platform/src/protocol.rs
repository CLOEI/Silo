#[cfg(not(target_os = "windows"))]
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
#[cfg(target_os = "macos")]
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};

use tracing::info;

/// Registers the `nxm://` URI scheme handler for the current user so clicking
/// "Mod Manager Download" on Nexus Mods invokes `silo "<nxm_uri>"`.
///
/// - **Windows**: Writes `HKCU\Software\Classes\nxm` and `HKCU\Software\Classes\nxm\shell\open\command`
///   (no Administrator privileges required).
/// - **Linux**: Writes `~/.local/share/applications/silo-nxm.desktop` and registers it via
///   `xdg-mime default silo-nxm.desktop x-scheme-handler/nxm`.
/// - **macOS**: When running inside a `.app` bundle, registers the bundle's `nxm` URL type
///   and installs an Apple Event handler so LaunchServices delivers links to the running app.
///   Unbundled development builds create a tiny `SiloNxmHandler.app` trampoline in
///   `~/Applications` instead.
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
const NXM_HANDLER_APP_NAME: &str = "SiloNxmHandler.app";
#[cfg(target_os = "macos")]
const NXM_HANDLER_BUNDLE_ID: &str = "com.cendy.silo.nxm";
#[cfg(target_os = "macos")]
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

#[cfg(target_os = "macos")]
fn register_nxm_macos(exe_path: &Path) -> io::Result<()> {
    let exe_path = exe_path
        .canonicalize()
        .unwrap_or_else(|_| exe_path.to_path_buf());
    let home = dirs::home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Home directory not found"))?;

    if let Some(bundle) = enclosing_app_bundle(&exe_path) {
        let _ = Command::new(LSREGISTER).arg("-f").arg(&bundle).status();
        set_default_url_handler("nxm", "com.cendy.silo")?;

        // Remove the trampoline used by unbundled development builds.
        let support_dir = home
            .join("Library")
            .join("Application Support")
            .join("Silo");
        let _ = fs::remove_dir_all(home.join("Applications").join(NXM_HANDLER_APP_NAME));
        let _ = fs::remove_file(support_dir.join("nxm-handler-exe"));

        info!(bundle = %bundle.display(), "Registered macOS nxm:// URL handler bundle");
        return Ok(());
    }

    let apps_dir = home.join("Applications");
    fs::create_dir_all(&apps_dir)?;

    let app_path = apps_dir.join(NXM_HANDLER_APP_NAME);
    let support_dir = home
        .join("Library")
        .join("Application Support")
        .join("Silo");
    fs::create_dir_all(&support_dir)?;
    let marker_path = support_dir.join("nxm-handler-exe");
    let marker = format!("v1\n{}", exe_path.display());

    let rebuild = match fs::read_to_string(&marker_path) {
        Ok(existing) => existing != marker || !app_path.exists(),
        Err(_) => true,
    };

    if rebuild {
        if app_path.exists() {
            fs::remove_dir_all(&app_path)?;
        }

        let script_path = support_dir.join("nxm-handler.applescript");
        fs::write(&script_path, nxm_handler_script(&exe_path))?;
        let status = Command::new("/usr/bin/osacompile")
            .arg("-o")
            .arg(&app_path)
            .arg(&script_path)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(
                "osacompile failed to build the nxm:// URL handler",
            ));
        }

        let plist = app_path.join("Contents").join("Info.plist");
        plutil_replace(
            &plist,
            "CFBundleIdentifier",
            &["-string", NXM_HANDLER_BUNDLE_ID],
        )?;
        plutil_replace(&plist, "CFBundleName", &["-string", "Silo NXM Handler"])?;
        plutil_insert_or_replace(&plist, "CFBundleVersion", &["-string", "1"])?;
        plutil_insert_or_replace(&plist, "LSUIElement", &["-bool", "true"])?;
        plutil_insert_or_replace(
            &plist,
            "CFBundleURLTypes",
            &[
                "-json",
                r#"[{"CFBundleURLName":"com.cendy.silo.nxm","CFBundleURLSchemes":["nxm"]}]"#,
            ],
        )?;

        fs::write(&marker_path, marker)?;
    }

    let _ = Command::new(LSREGISTER).arg("-f").arg(&app_path).status();
    set_default_url_handler("nxm", NXM_HANDLER_BUNDLE_ID)?;
    info!(handler = %app_path.display(), "Registered macOS nxm:// URL handler app");
    Ok(())
}

#[cfg(target_os = "macos")]
fn nxm_handler_script(exe_path: &Path) -> String {
    let exe = exe_path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        "on open location theURL\n    do shell script \"/usr/bin/nohup \" & quoted form of \"{exe}\" & \" \" & quoted form of theURL & \" >/dev/null 2>&1 &\"\nend open location\n"
    )
}

#[cfg(target_os = "macos")]
fn run_plutil(plist: &Path, args: &[&str]) -> io::Result<()> {
    let status = Command::new("/usr/bin/plutil")
        .args(args)
        .arg(plist)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "plutil {} failed on {}",
            args.join(" "),
            plist.display()
        )))
    }
}

#[cfg(target_os = "macos")]
fn plutil_replace(plist: &Path, key: &str, value: &[&str]) -> io::Result<()> {
    let mut args = vec!["-replace", key];
    args.extend_from_slice(value);
    run_plutil(plist, &args)
}

#[cfg(target_os = "macos")]
fn plutil_insert_or_replace(plist: &Path, key: &str, value: &[&str]) -> io::Result<()> {
    let mut args = vec!["-insert", key];
    args.extend_from_slice(value);
    if run_plutil(plist, &args).is_ok() {
        return Ok(());
    }
    plutil_replace(plist, key, value)
}

#[cfg(target_os = "macos")]
fn set_default_url_handler(scheme: &str, bundle_id: &str) -> io::Result<()> {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSSetDefaultHandlerForURLScheme(scheme: CFStringRef, handler: CFStringRef) -> i32;
    }

    let scheme = CFString::new(scheme);
    let handler = CFString::new(bundle_id);
    let status = unsafe {
        LSSetDefaultHandlerForURLScheme(scheme.as_concrete_TypeRef(), handler.as_concrete_TypeRef())
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "LSSetDefaultHandlerForURLScheme returned {status}"
        )))
    }
}

#[cfg(target_os = "macos")]
fn enclosing_app_bundle(exe_path: &Path) -> Option<PathBuf> {
    exe_path
        .ancestors()
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("app"))
        .map(Path::to_path_buf)
}

#[cfg(target_os = "macos")]
const DIRECT_OBJECT: u32 = 0x2D2D_2D2D; // '----'

#[cfg(target_os = "macos")]
struct UrlHandlerIvars {
    on_url: Box<dyn Fn(String) + Send + 'static>,
}

#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SiloNxmUrlHandler"]
    #[ivars = UrlHandlerIvars]
    struct NxmUrlHandler;

    unsafe impl NSObjectProtocol for NxmUrlHandler {}

    impl NxmUrlHandler {
        #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
        fn handle_get_url(
            &self,
            event: &NSAppleEventDescriptor,
            _reply: &NSAppleEventDescriptor,
        ) {
            let Some(descriptor) = event.paramDescriptorForKeyword(DIRECT_OBJECT) else {
                return;
            };
            let Some(url) = descriptor.stringValue() else {
                return;
            };
            (self.ivars().on_url)(url.to_string());
        }
    }
);

#[cfg(target_os = "macos")]
impl NxmUrlHandler {
    fn new(on_url: Box<dyn Fn(String) + Send + 'static>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(UrlHandlerIvars { on_url });
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(target_os = "macos")]
fn as_any_object<T>(object: &T) -> &AnyObject {
    unsafe { &*(object as *const T as *const AnyObject) }
}

/// Installs the handler that receives `nxm://` URLs delivered by LaunchServices.
/// Must be called from the main thread before the event loop starts.
#[cfg(target_os = "macos")]
pub fn install_macos_url_handler(on_url: impl Fn(String) + Send + 'static) -> io::Result<()> {
    const INTERNET_EVENT_CLASS: u32 = 0x4755_524C; // 'GURL'
    const GET_URL_EVENT: u32 = 0x4755_524C; // 'GURL'

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| io::Error::other("the URL handler must be installed on the main thread"))?;
    let handler = NxmUrlHandler::new(Box::new(on_url), mtm);

    unsafe {
        NSAppleEventManager::sharedAppleEventManager()
            .setEventHandler_andSelector_forEventClass_andEventID(
                as_any_object(&*handler),
                sel!(handleGetURLEvent:withReplyEvent:),
                INTERNET_EVENT_CLASS,
                GET_URL_EVENT,
            );
    }

    // The event manager does not retain the handler.
    std::mem::forget(handler);
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
