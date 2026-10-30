use crate::archive::{
    create_staging_snapshot, export_encrypted_bundle, import_encrypted_bundle,
    verify_staging_snapshot,
};
use crate::config::{SecurityLimits, DEFAULT_DISCOVERY_PORT};
use crate::discovery::{
    build_receiver_announcement, detect_local_lan_ip, start_lan_discovery, DiscoveredPeer,
    DiscoveryServiceHandle,
};
use crate::errors::{AppError, AppResult, ErrorCode};
use crate::inventory::{scan_codex_inventory, ContentCategory, MigrationManifest, ScanOptions};
use crate::logging::{export_redacted_diagnostics, DiagnosticLogEntry};
use crate::migration::{
    apply_migration_from_staging, FailureInjectionHook, MigrationExecutionResult,
};
use crate::pairing::{
    create_receiver_pairing_ticket, generate_ephemeral_tls_identity, parse_connection_code_string,
    PairingApprovalResponse, PairingAuthMode, ReceiverPairingTicket,
};
use crate::paths::PathStyle;
use crate::planner::{
    build_import_plan, propose_default_project_mappings, ImportPlan, ProjectPathMappingConfig,
};
use crate::platform::{get_system_overview, SystemOverview};
use crate::recovery::{
    discover_recovery_journals, execute_rollback, RecoveryJournal, RollbackReport,
};
use crate::transport::{
    sender_poll_pairing_approval, sender_request_pairing, sender_stream_snapshot_to_receiver,
    start_https_receiver_server, ReceiverServerHandle, ReceiverSessionSnapshot,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, State};
use uuid::Uuid;

pub struct AppRuntimeState {
    pub limits: SecurityLimits,
    pub discovery_handle: Mutex<Option<DiscoveryServiceHandle>>,
    pub receiver_server: Mutex<Option<ReceiverServerHandle>>,
    pub active_ticket: Mutex<Option<ReceiverPairingTicket>>,
    pub cached_manifest: Mutex<Option<MigrationManifest>>,
    pub sender_staging_dir: Mutex<Option<PathBuf>>,
}

impl Default for AppRuntimeState {
    fn default() -> Self {
        Self {
            limits: SecurityLimits::default(),
            discovery_handle: Mutex::new(None),
            receiver_server: Mutex::new(None),
            active_ticket: Mutex::new(None),
            cached_manifest: Mutex::new(None),
            sender_staging_dir: Mutex::new(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartReceiverResponse {
    pub ticket: ReceiverPairingTicket,
    pub staging_dir: String,
}

#[tauri::command]
pub fn cmd_get_system_overview() -> SystemOverview {
    get_system_overview()
}

#[tauri::command]
pub fn cmd_scan_codex_inventory(
    options: ScanOptions,
    state: State<'_, AppRuntimeState>,
) -> AppResult<MigrationManifest> {
    let manifest = scan_codex_inventory(&options, &state.limits)?;
    if let Ok(mut guard) = state.cached_manifest.lock() {
        *guard = Some(manifest.clone());
    }
    Ok(manifest)
}

#[tauri::command]
pub fn cmd_get_cached_manifest(state: State<'_, AppRuntimeState>) -> AppResult<MigrationManifest> {
    if let Ok(guard) = state.cached_manifest.lock() {
        if let Some(ref m) = *guard {
            return Ok(m.clone());
        }
    }
    if let Ok(r_guard) = state.receiver_server.lock() {
        if let Some(ref srv) = *r_guard {
            if let Some(m) = srv.take_manifest() {
                if let Ok(mut m_guard) = state.cached_manifest.lock() {
                    *m_guard = Some(m.clone());
                }
                return Ok(m);
            }
        }
    }
    Err(AppError::new(
        ErrorCode::CodexHomeNotFound,
        "No cached or received migration manifest available.",
    ))
}

#[tauri::command]
pub fn cmd_propose_project_mappings(
    default_projects_root: String,
    state: State<'_, AppRuntimeState>,
) -> AppResult<Vec<ProjectPathMappingConfig>> {
    let guard = state
        .cached_manifest
        .lock()
        .map_err(|_| AppError::new(ErrorCode::SerializationError, "Manifest lock poisoned."))?;
    let Some(ref manifest) = *guard else {
        return Err(AppError::new(
            ErrorCode::CodexHomeNotFound,
            "No migration manifest loaded. Scan or receive a manifest first.",
        ));
    };
    Ok(propose_default_project_mappings(
        manifest,
        &default_projects_root,
        PathStyle::current(),
    ))
}

#[tauri::command]
pub fn cmd_build_import_plan(
    target_codex_home: String,
    target_shared_skills_dir: Option<String>,
    selected_categories: Vec<ContentCategory>,
    project_mappings: Vec<ProjectPathMappingConfig>,
    include_sensitive_files: bool,
    staging_already_complete: bool,
    state: State<'_, AppRuntimeState>,
) -> AppResult<ImportPlan> {
    let guard = state
        .cached_manifest
        .lock()
        .map_err(|_| AppError::new(ErrorCode::SerializationError, "Manifest lock poisoned."))?;
    let Some(ref manifest) = *guard else {
        return Err(AppError::new(
            ErrorCode::CodexHomeNotFound,
            "No migration manifest loaded.",
        ));
    };

    build_import_plan(
        manifest,
        std::path::Path::new(&target_codex_home),
        target_shared_skills_dir
            .as_deref()
            .map(std::path::Path::new),
        &selected_categories,
        &project_mappings,
        include_sensitive_files,
        staging_already_complete,
    )
}

#[tauri::command]
pub async fn cmd_start_receiver_mode(
    device_alias: String,
    auto_approve_pairing: bool,
    state: State<'_, AppRuntimeState>,
) -> AppResult<StartReceiverResponse> {
    // Stop any previous discovery or receiver session
    if let Ok(mut d_guard) = state.discovery_handle.lock() {
        if let Some(prev) = d_guard.take() {
            prev.stop();
        }
    }
    if let Ok(mut r_guard) = state.receiver_server.lock() {
        if let Some(prev) = r_guard.take() {
            prev.stop();
        }
    }

    let session_id = Uuid::now_v7().to_string();
    let lan_ip = detect_local_lan_ip();
    let tls_id = generate_ephemeral_tls_identity()?;

    // Bind HTTPS server to an available port (preferring 53317, falling back to OS-assigned port)
    let preferred_addr: std::net::SocketAddr =
        format!("0.0.0.0:{DEFAULT_DISCOVERY_PORT}").parse().unwrap();
    let staging_dir = std::env::temp_dir()
        .join("codex-move-staging")
        .join(&session_id);

    let temp_ticket = create_receiver_pairing_ticket(
        &session_id,
        &lan_ip,
        DEFAULT_DISCOVERY_PORT,
        &tls_id,
        &state.limits,
    )?;

    let server_handle = match start_https_receiver_server(
        preferred_addr,
        &tls_id,
        temp_ticket.clone(),
        staging_dir.clone(),
        state.limits.clone(),
        auto_approve_pairing,
    )
    .await
    {
        Ok(h) => h,
        Err(_) => {
            let fallback_addr: std::net::SocketAddr = "0.0.0.0:0".parse().unwrap();
            start_https_receiver_server(
                fallback_addr,
                &tls_id,
                temp_ticket.clone(),
                staging_dir.clone(),
                state.limits.clone(),
                auto_approve_pairing,
            )
            .await?
        }
    };

    let actual_port = server_handle.addr.port();
    let final_ticket = if actual_port == DEFAULT_DISCOVERY_PORT {
        temp_ticket
    } else {
        let updated = create_receiver_pairing_ticket(
            &session_id,
            &lan_ip,
            actual_port,
            &tls_id,
            &state.limits,
        )?;
        server_handle.update_ticket(updated.clone());
        updated
    };

    let announcement = build_receiver_announcement(
        &session_id,
        &device_alias,
        actual_port,
        &tls_id.cert_fingerprint_sha256,
        &final_ticket.pake_salt_hex,
    );
    let disc_handle = start_lan_discovery(Some(announcement), DEFAULT_DISCOVERY_PORT)?;

    if let Ok(mut d_guard) = state.discovery_handle.lock() {
        *d_guard = Some(disc_handle);
    }
    if let Ok(mut r_guard) = state.receiver_server.lock() {
        *r_guard = Some(server_handle);
    }
    if let Ok(mut t_guard) = state.active_ticket.lock() {
        *t_guard = Some(final_ticket.clone());
    }

    Ok(StartReceiverResponse {
        ticket: final_ticket,
        staging_dir: staging_dir.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub fn cmd_start_sender_discovery(state: State<'_, AppRuntimeState>) -> AppResult<()> {
    if let Ok(mut d_guard) = state.discovery_handle.lock() {
        if let Some(prev) = d_guard.take() {
            prev.stop();
        }
        let handle = start_lan_discovery(None, DEFAULT_DISCOVERY_PORT)?;
        *d_guard = Some(handle);
    }
    Ok(())
}

#[tauri::command]
pub fn cmd_list_discovered_peers(state: State<'_, AppRuntimeState>) -> Vec<DiscoveredPeer> {
    if let Ok(guard) = state.discovery_handle.lock() {
        if let Some(ref h) = *guard {
            return h.list_active_peers();
        }
    }
    Vec::new()
}

#[tauri::command]
pub fn cmd_stop_network_services(state: State<'_, AppRuntimeState>) -> AppResult<()> {
    if let Ok(mut d_guard) = state.discovery_handle.lock() {
        if let Some(prev) = d_guard.take() {
            prev.stop();
        }
    }
    if let Ok(mut r_guard) = state.receiver_server.lock() {
        if let Some(prev) = r_guard.take() {
            prev.stop();
        }
    }
    Ok(())
}

#[tauri::command]
pub fn cmd_get_receiver_session_snapshot(
    state: State<'_, AppRuntimeState>,
) -> Option<ReceiverSessionSnapshot> {
    let snap = state
        .receiver_server
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.snapshot()));

    // If transfer completed, also cache the received manifest for planning
    if let Some(ref s) = snap {
        if s.transfer_completed {
            if let Ok(r_guard) = state.receiver_server.lock() {
                if let Some(ref srv) = *r_guard {
                    if let Some(m) = srv.take_manifest() {
                        if let Ok(mut m_guard) = state.cached_manifest.lock() {
                            *m_guard = Some(m);
                        }
                    }
                }
            }
        }
    }
    snap
}

#[tauri::command]
pub fn cmd_approve_receiver_pairing(state: State<'_, AppRuntimeState>) -> AppResult<String> {
    let guard = state
        .receiver_server
        .lock()
        .map_err(|_| AppError::new(ErrorCode::NetworkError, "Receiver lock poisoned."))?;
    let Some(ref srv) = *guard else {
        return Err(AppError::new(
            ErrorCode::NetworkError,
            "Receiver service is not active.",
        ));
    };
    srv.approve_pending_sender()
}

#[tauri::command]
pub fn cmd_reject_receiver_pairing(state: State<'_, AppRuntimeState>) -> AppResult<()> {
    if let Ok(guard) = state.receiver_server.lock() {
        if let Some(ref srv) = *guard {
            srv.reject_or_revoke_sender();
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectByCodeRequest {
    pub connection_code_or_short_code: String,
    pub manual_host_ip: Option<String>,
    pub manual_port: Option<u16>,
    pub peer_cert_fingerprint: Option<String>,
    pub peer_pake_salt_hex: Option<String>,
    pub sender_device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstablishedSenderConnection {
    pub receiver_base_url: String,
    pub receiver_cert_fingerprint: String,
    pub approved: bool,
    pub poll_token: Option<String>,
    pub session_bearer_token: Option<String>,
    pub sas_verification_code: Option<String>,
    pub message: String,
}

#[tauri::command]
pub async fn cmd_sender_pair_with_receiver(
    req: ConnectByCodeRequest,
    state: State<'_, AppRuntimeState>,
) -> AppResult<EstablishedSenderConnection> {
    let (threads_count, projects_count, total_bytes) = state
        .cached_manifest
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref().map(|m| {
                let b: u64 = m.files.iter().map(|f| f.size_bytes).sum();
                (m.threads.len(), m.projects.len(), b)
            })
        })
        .unwrap_or((0, 0, 0));

    let trimmed = req.connection_code_or_short_code.trim();
    if trimmed.starts_with("cdxmove://") {
        let (_session_id, host, port, fingerprint, token_b64) =
            parse_connection_code_string(trimmed)?;
        let base_url = format!("https://{host}:{port}");
        let resp = sender_request_pairing(
            &base_url,
            &fingerprint,
            PairingAuthMode::HighEntropyToken,
            &token_b64,
            "",
            &req.sender_device_name,
            threads_count,
            projects_count,
            total_bytes,
        )
        .await?;

        Ok(EstablishedSenderConnection {
            receiver_base_url: base_url,
            receiver_cert_fingerprint: resp.receiver_cert_fingerprint,
            approved: resp.approved,
            poll_token: resp.poll_token,
            session_bearer_token: resp.session_bearer_token,
            sas_verification_code: resp.sas_verification_code,
            message: resp.message,
        })
    } else {
        let host = req.manual_host_ip.ok_or_else(|| {
            AppError::new(
                ErrorCode::PairingAuthenticationFailed,
                "Target IP address is required when using a 6-character pairing code.",
            )
        })?;
        let port = req.manual_port.unwrap_or(DEFAULT_DISCOVERY_PORT);
        let fingerprint = req.peer_cert_fingerprint.unwrap_or_default();
        let salt_hex = req.peer_pake_salt_hex.unwrap_or_default();
        let base_url = format!("https://{host}:{port}");

        let resp = sender_request_pairing(
            &base_url,
            &fingerprint,
            PairingAuthMode::ShortCodeArgon2,
            trimmed,
            &salt_hex,
            &req.sender_device_name,
            threads_count,
            projects_count,
            total_bytes,
        )
        .await?;

        Ok(EstablishedSenderConnection {
            receiver_base_url: base_url,
            receiver_cert_fingerprint: resp.receiver_cert_fingerprint,
            approved: resp.approved,
            poll_token: resp.poll_token,
            session_bearer_token: resp.session_bearer_token,
            sas_verification_code: resp.sas_verification_code,
            message: resp.message,
        })
    }
}

#[tauri::command]
pub async fn cmd_sender_poll_approval(
    receiver_base_url: String,
    receiver_cert_fingerprint: String,
    poll_token: String,
) -> AppResult<PairingApprovalResponse> {
    sender_poll_pairing_approval(&receiver_base_url, &receiver_cert_fingerprint, &poll_token).await
}

#[tauri::command]
pub async fn cmd_sender_execute_transfer(
    receiver_base_url: String,
    receiver_cert_fingerprint: String,
    bearer_token: String,
    selected_categories: Vec<ContentCategory>,
    include_sensitive_files: bool,
    window: tauri::Window,
    state: State<'_, AppRuntimeState>,
) -> AppResult<String> {
    let mut manifest = {
        let guard = state
            .cached_manifest
            .lock()
            .map_err(|_| AppError::new(ErrorCode::SerializationError, "Manifest lock poisoned."))?;
        guard.clone().ok_or_else(|| {
            AppError::new(
                ErrorCode::CodexHomeNotFound,
                "Please scan source Codex data before starting transfer.",
            )
        })?
    };

    // Filter manifest files to user-selected categories & sensitivity choices
    let sel_set: std::collections::HashSet<ContentCategory> =
        selected_categories.into_iter().collect();
    manifest
        .files
        .retain(|f| sel_set.contains(&f.category) && (include_sensitive_files || !f.is_sensitive));

    let staging_dir = std::env::temp_dir()
        .join("codex-move-sender-snapshot")
        .join(&manifest.migration_id);

    let manifest_hash = create_staging_snapshot(&mut manifest, &staging_dir, &state.limits)?;
    if let Ok(mut s_guard) = state.sender_staging_dir.lock() {
        *s_guard = Some(staging_dir.clone());
    }

    let limits_clone = state.limits.clone();
    let mig_id_clone = manifest.migration_id.clone();

    sender_stream_snapshot_to_receiver(
        &receiver_base_url,
        &receiver_cert_fingerprint,
        &bearer_token,
        &staging_dir,
        &manifest,
        &limits_clone,
        None,
        |done_files, total_files, done_bytes, total_bytes, current_item| {
            let _ = window.emit(
                "transfer-progress",
                serde_json::json!({
                    "migration_id": mig_id_clone,
                    "processed_files": done_files,
                    "total_files": total_files,
                    "processed_bytes": done_bytes,
                    "total_bytes": total_bytes,
                    "current_item": current_item,
                }),
            );
        },
    )
    .await?;

    // Clean up sender temporary snapshot once verified by receiver
    let _ = std::fs::remove_dir_all(&staging_dir);

    Ok(manifest_hash)
}

#[tauri::command]
pub fn cmd_apply_migration(
    staging_dir: String,
    plan: ImportPlan,
    window: tauri::Window,
    state: State<'_, AppRuntimeState>,
) -> AppResult<MigrationExecutionResult> {
    let stg_path = PathBuf::from(&staging_dir);
    let (manifest, _) = verify_staging_snapshot(&stg_path, &state.limits)?;
    if let Ok(mut guard) = state.cached_manifest.lock() {
        *guard = Some(manifest.clone());
    }

    let res = apply_migration_from_staging(
        &manifest,
        &stg_path,
        &plan,
        &state.limits,
        &FailureInjectionHook::default(),
        |update| {
            let _ = window.emit("migration-progress", &update);
        },
    )?;

    Ok(res)
}

#[tauri::command]
pub fn cmd_discover_recovery_journals(target_codex_home: String) -> Vec<RecoveryJournal> {
    discover_recovery_journals(std::path::Path::new(&target_codex_home))
}

#[tauri::command]
pub fn cmd_execute_rollback(
    target_codex_home: String,
    migration_id: String,
) -> AppResult<RollbackReport> {
    let mut journals = discover_recovery_journals(std::path::Path::new(&target_codex_home));
    let Some(mut target_j) = journals.drain(..).find(|j| j.migration_id == migration_id) else {
        return Err(AppError::new(
            ErrorCode::BackupVerificationFailed,
            format!("No recovery journal found for migration ID {migration_id}"),
        ));
    };
    execute_rollback(&mut target_j)
}

#[tauri::command]
pub fn cmd_export_encrypted_bundle(
    output_file: String,
    passphrase: String,
    state: State<'_, AppRuntimeState>,
) -> AppResult<String> {
    let mut manifest = {
        let guard = state
            .cached_manifest
            .lock()
            .map_err(|_| AppError::new(ErrorCode::SerializationError, "Manifest lock poisoned."))?;
        guard.clone().ok_or_else(|| {
            AppError::new(
                ErrorCode::CodexHomeNotFound,
                "Scan source Codex directory first.",
            )
        })?
    };

    let staging_dir = std::env::temp_dir()
        .join("codex-move-bundle-export")
        .join(&manifest.migration_id);
    let hash = create_staging_snapshot(&mut manifest, &staging_dir, &state.limits)?;
    export_encrypted_bundle(
        &staging_dir,
        std::path::Path::new(&output_file),
        &passphrase,
    )?;
    let _ = std::fs::remove_dir_all(&staging_dir);
    Ok(hash)
}

#[tauri::command]
pub fn cmd_import_encrypted_bundle(
    bundle_file: String,
    passphrase: String,
    state: State<'_, AppRuntimeState>,
) -> AppResult<(MigrationManifest, String)> {
    let staging_dir = std::env::temp_dir()
        .join("codex-move-bundle-import")
        .join(Uuid::now_v7().to_string());
    let (manifest, _hash) = import_encrypted_bundle(
        std::path::Path::new(&bundle_file),
        &staging_dir,
        &passphrase,
        &state.limits,
        10 * 1024 * 1024 * 1024, // 10 GiB limit
    )?;
    if let Ok(mut guard) = state.cached_manifest.lock() {
        *guard = Some(manifest.clone());
    }
    Ok((manifest, staging_dir.to_string_lossy().to_string()))
}

#[tauri::command]
pub fn cmd_export_diagnostics() -> Vec<DiagnosticLogEntry> {
    export_redacted_diagnostics()
}
