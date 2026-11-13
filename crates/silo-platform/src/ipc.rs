use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, Listener, ListenerOptions, Name, Stream,
    ToFsName, ToNsName, prelude::*,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

pub const SILO_IPC_SOCKET_ID: &str = "silo-nxm-ipc.sock";

/// Resolves the cross-platform `interprocess::local_socket::Name` for Silo's single-instance IPC.
pub fn silo_ipc_socket_name() -> io::Result<Name<'static>> {
    if GenericNamespaced::is_supported() {
        SILO_IPC_SOCKET_ID
            .to_ns_name::<GenericNamespaced>()
            .map_err(io::Error::other)
    } else {
        let sock_path: PathBuf = std::env::temp_dir().join(SILO_IPC_SOCKET_ID);
        sock_path
            .to_fs_name::<GenericFilePath>()
            .map_err(io::Error::other)
    }
}

/// Attempts to forward an `nxm://...` URI to an already-running Silo instance over
/// `interprocess::local_socket`.
///
/// Returns `Ok(true)` if an existing Silo window accepted the URI (meaning the caller
/// process can exit immediately without launching a duplicate window), or `Ok(false)`
/// if no primary instance is currently listening.
pub fn try_forward_nxm_to_running_instance(nxm_uri: &str) -> io::Result<bool> {
    let name = silo_ipc_socket_name()?;
    match Stream::connect(name) {
        Ok(mut stream) => {
            let line = format!("{}\n", nxm_uri.trim());
            stream.write_all(line.as_bytes())?;
            stream.flush()?;
            info!(uri = %nxm_uri, "Forwarded nxm:// URI to already-running Silo instance");
            Ok(true)
        }
        Err(_conn_err) => Ok(false),
    }
}

/// Binds the single-instance `interprocess::local_socket` listener and spawns a background
/// thread that forwards incoming `nxm://` URIs to `tx`.
pub fn spawn_nxm_ipc_listener(tx: UnboundedSender<String>) -> io::Result<()> {
    if !GenericNamespaced::is_supported() {
        let sock_path = std::env::temp_dir().join(SILO_IPC_SOCKET_ID);
        if sock_path.exists() {
            let _ = std::fs::remove_file(&sock_path);
        }
    }

    let name = silo_ipc_socket_name()?;
    let listener: Listener = ListenerOptions::new().name(name).create_sync()?;

    std::thread::Builder::new()
        .name("silo-nxm-ipc-listener".to_string())
        .spawn(move || {
            info!("Listening for single-instance nxm:// IPC connections");
            for conn in listener.incoming() {
                match conn {
                    Ok(stream) => {
                        let reader = BufReader::new(stream);
                        for line in reader.lines().map_while(Result::ok) {
                            let trimmed = line.trim().to_string();
                            if !trimmed.is_empty() {
                                debug!(uri = %trimmed, "Received nxm:// URI over local socket IPC");
                                if tx.send(trimmed).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(err) => {
                        warn!(error = %err, "Error accepting incoming nxm:// local socket connection");
                    }
                }
            }
        })?;

    Ok(())
}

/// Scans command-line arguments for any `nxm://` URI passed by the OS protocol handler.
pub fn extract_nxm_arg_from_env() -> Option<String> {
    std::env::args()
        .skip(1)
        .map(|arg| arg.trim().trim_matches('"').to_string())
        .find(|arg| {
            arg.starts_with("nxm://") || arg.starts_with("NXM://")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_socket_name_resolves_cleanly() {
        let name = silo_ipc_socket_name();
        assert!(name.is_ok());
    }
}
