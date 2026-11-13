pub mod ipc;
pub mod mounter;
pub mod paths;
pub mod protocol;
pub mod steam;

pub use ipc::{
    SILO_IPC_SOCKET_ID, extract_nxm_arg_from_env, silo_ipc_socket_name, spawn_nxm_ipc_listener,
    try_forward_nxm_to_running_instance,
};
pub use mounter::{
    BUNDLED_SILO_GUARD_DLL, BUNDLED_SILO_GUARD_MANIFEST, MountMethod, PROTECTED_MOD_FOLDERS,
    activate_profile_on_disk, clear_mods_directory, deploy_bundled_silo_guard,
    deploy_silo_guard_artifacts, is_protected_mod_folder, launch_smapi, mount_mod_folder,
    resolve_mods_dir, write_active_profile_contract,
};
pub use paths::{
    candidate_saves_paths, resolve_saves_path, silo_covers_cache_dir, silo_db_path, silo_home_dir,
    silo_vault_dir,
};
#[cfg(target_os = "macos")]
pub use protocol::install_macos_url_handler;
pub use protocol::{macos_info_plist_url_types_snippet, register_nxm_protocol_handler};
pub use steam::{
    detect_stardew_install_path, is_valid_stardew_install_dir, parse_steam_library_folders,
    pick_stardew_install_folder,
};
