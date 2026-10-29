pub mod adapters;
pub mod archive;
pub mod commands;
pub mod config;
pub mod discovery;
pub mod errors;
pub mod inventory;
pub mod logging;
pub mod migration;
pub mod pairing;
pub mod paths;
pub mod planner;
pub mod platform;
pub mod recovery;
pub mod transport;
pub mod verification;

use commands::*;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init_logging();

    tauri::Builder::default()
        .manage(AppRuntimeState::default())
        .invoke_handler(tauri::generate_handler![
            cmd_get_system_overview,
            cmd_scan_codex_inventory,
            cmd_get_cached_manifest,
            cmd_propose_project_mappings,
            cmd_build_import_plan,
            cmd_start_receiver_mode,
            cmd_start_sender_discovery,
            cmd_list_discovered_peers,
            cmd_stop_network_services,
            cmd_get_receiver_session_snapshot,
            cmd_approve_receiver_pairing,
            cmd_reject_receiver_pairing,
            cmd_sender_pair_with_receiver,
            cmd_sender_poll_approval,
            cmd_sender_execute_transfer,
            cmd_apply_migration,
            cmd_discover_recovery_journals,
            cmd_execute_rollback,
            cmd_export_encrypted_bundle,
            cmd_import_encrypted_bundle,
            cmd_export_diagnostics,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Codex Move application");
}
