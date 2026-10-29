use crate::archive::verify_staging_snapshot;
use crate::config::{SecurityLimits, CURRENT_FORMAT_VERSION, CURRENT_PROTOCOL_VERSION};
use crate::errors::{AppError, AppResult, ErrorCode};
use crate::inventory::MigrationManifest;
use crate::pairing::{
    compute_pairing_hmac_proof, constant_time_eq, create_sender_pake_handshake,
    derive_sas_verification_code, verify_pairing_request, EphemeralTlsIdentity,
    PairingApprovalResponse, PairingAuthMode, PairingRateLimiter, PairingRequestPayload,
    ReceiverPairingTicket,
};
use crate::paths::resolve_safe_child_path;
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Json, Router,
};
use axum_server::tls_rustls::RustlsConfig;
use rand::RngCore;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesHandshake {
    pub protocol_version: String,
    pub supported_format_versions: Vec<String>,
    pub supported_state_db_versions: Vec<u32>,
    pub supports_chunk_resume: bool,
    pub max_chunk_bytes: usize,
    pub os: String,
    pub arch: String,
    pub case_insensitive_fs: bool,
}

impl Default for CapabilitiesHandshake {
    fn default() -> Self {
        Self {
            protocol_version: CURRENT_PROTOCOL_VERSION.to_string(),
            supported_format_versions: vec![CURRENT_FORMAT_VERSION.to_string()],
            supported_state_db_versions: vec![4, 5],
            supports_chunk_resume: true,
            max_chunk_bytes: 4 * 1024 * 1024,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            case_insensitive_fs: cfg!(windows) || cfg!(target_os = "macos"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferResumeStatus {
    pub migration_id: String,
    pub manifest_hash: String,
    /// Map of `entry_id -> received_bytes_offset`
    pub file_offsets: HashMap<String, u64>,
    pub completed_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiverSessionSnapshot {
    pub pairing_approved: bool,
    pub pending_sender_request: Option<PairingRequestPayload>,
    pub sas_verification_code: Option<String>,
    pub active_migration_id: Option<String>,
    pub active_manifest_hash: Option<String>,
    pub received_files_count: usize,
    pub total_files_count: usize,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub transfer_completed: bool,
    pub staging_dir: String,
}

struct ReceiverServerState {
    ticket: ReceiverPairingTicket,
    limiter: PairingRateLimiter,
    limits: SecurityLimits,
    auto_approve_pairing: bool,
    pending_request: Option<PairingRequestPayload>,
    pending_poll_token: Option<String>,
    pending_receiver_confirm_hex: Option<String>,
    pending_sas_code: Option<String>,
    approved_bearer_token: Option<String>,
    staging_dir: PathBuf,
    active_migration_id: Option<String>,
    active_manifest_hash: Option<String>,
    manifest: Option<MigrationManifest>,
    file_offsets: HashMap<String, u64>,
    completed_files: Vec<String>,
    received_bytes: u64,
    total_bytes: u64,
    transfer_completed: bool,
}

pub struct ReceiverServerHandle {
    pub addr: SocketAddr,
    pub shutdown_handle: axum_server::Handle,
    state: Arc<Mutex<ReceiverServerState>>,
}

impl ReceiverServerHandle {
    pub fn stop(&self) {
        self.shutdown_handle.shutdown();
    }

    pub fn approve_pending_sender(&self) -> AppResult<String> {
        let mut guard = self.state.lock().map_err(|_| {
            AppError::new(ErrorCode::NetworkError, "Receiver state mutex poisoned.")
        })?;
        if guard.pending_request.is_none() {
            return Err(AppError::new(
                ErrorCode::PairingAuthenticationFailed,
                "No pending sender pairing request to approve.",
            ));
        }
        let token = generate_bearer_token();
        guard.approved_bearer_token = Some(token.clone());
        Ok(token)
    }

    pub fn reject_or_revoke_sender(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.pending_request = None;
            guard.pending_poll_token = None;
            guard.pending_receiver_confirm_hex = None;
            guard.pending_sas_code = None;
            guard.approved_bearer_token = None;
        }
    }

    pub fn update_ticket(&self, new_ticket: ReceiverPairingTicket) {
        if let Ok(mut guard) = self.state.lock() {
            guard.ticket = new_ticket;
        }
    }

    pub fn snapshot(&self) -> ReceiverSessionSnapshot {
        let guard = self.state.lock().unwrap();
        ReceiverSessionSnapshot {
            pairing_approved: guard.approved_bearer_token.is_some(),
            pending_sender_request: guard.pending_request.clone(),
            sas_verification_code: guard.pending_sas_code.clone(),
            active_migration_id: guard.active_migration_id.clone(),
            active_manifest_hash: guard.active_manifest_hash.clone(),
            received_files_count: guard.completed_files.len(),
            total_files_count: guard.manifest.as_ref().map(|m| m.files.len()).unwrap_or(0),
            received_bytes: guard.received_bytes,
            total_bytes: guard.total_bytes,
            transfer_completed: guard.transfer_completed,
            staging_dir: guard.staging_dir.to_string_lossy().to_string(),
        }
    }

    pub fn take_manifest(&self) -> Option<MigrationManifest> {
        self.state.lock().ok().and_then(|g| g.manifest.clone())
    }
}

fn generate_bearer_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Starts the ephemeral HTTPS receiver server bound to `bind_addr`.
pub async fn start_https_receiver_server(
    bind_addr: SocketAddr,
    tls_identity: &EphemeralTlsIdentity,
    ticket: ReceiverPairingTicket,
    staging_dir: PathBuf,
    limits: SecurityLimits,
    auto_approve_pairing: bool,
) -> AppResult<ReceiverServerHandle> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    std::fs::create_dir_all(&staging_dir)?;

    let cert_pem = der_to_pem("CERTIFICATE", &tls_identity.cert_der);
    let key_pem = der_to_pem("PRIVATE KEY", &tls_identity.key_der);

    let rustls_config = RustlsConfig::from_pem(cert_pem.into_bytes(), key_pem.into_bytes())
        .await
        .map_err(|e| {
            AppError::new(
                ErrorCode::NetworkError,
                format!("Failed to configure TLS server: {e}"),
            )
        })?;

    let listener = std::net::TcpListener::bind(bind_addr)?;
    let local_addr = listener.local_addr()?;

    let shared_state = Arc::new(Mutex::new(ReceiverServerState {
        ticket,
        limiter: PairingRateLimiter::new(&limits),
        limits: limits.clone(),
        auto_approve_pairing,
        pending_request: None,
        pending_poll_token: None,
        pending_receiver_confirm_hex: None,
        pending_sas_code: None,
        approved_bearer_token: None,
        staging_dir,
        active_migration_id: None,
        active_manifest_hash: None,
        manifest: None,
        file_offsets: HashMap::new(),
        completed_files: Vec::new(),
        received_bytes: 0,
        total_bytes: 0,
        transfer_completed: false,
    }));

    let app = Router::new()
        .route("/api/codex-move/v1/ping", get(handle_ping))
        .route("/api/codex-move/v1/pair/request", post(handle_pair_request))
        .route("/api/codex-move/v1/pair/poll", get(handle_pair_poll))
        .route("/api/codex-move/v1/capabilities", post(handle_capabilities))
        .route("/api/codex-move/v1/manifest", post(handle_manifest))
        .route(
            "/api/codex-move/v1/transfer/status",
            get(handle_transfer_status),
        )
        .route(
            "/api/codex-move/v1/transfer/chunk",
            put(handle_transfer_chunk),
        )
        .route(
            "/api/codex-move/v1/transfer/complete",
            post(handle_transfer_complete),
        )
        .with_state(shared_state.clone());

    let shutdown_handle = axum_server::Handle::new();
    let handle_clone = shutdown_handle.clone();

    tokio::spawn(async move {
        let _ = axum_server::from_tcp_rustls(listener, rustls_config)
            .handle(handle_clone)
            .serve(app.into_make_service())
            .await;
    });

    Ok(ReceiverServerHandle {
        addr: local_addr,
        shutdown_handle,
        state: shared_state,
    })
}

fn der_to_pem(label: &str, der: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

async fn handle_ping(
    State(state): State<Arc<Mutex<ReceiverServerState>>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let guard = state.lock().unwrap();
    Ok(Json(serde_json::json!({
        "app": "codex-move",
        "protocol_version": CURRENT_PROTOCOL_VERSION,
        "session_id": guard.ticket.session_id,
        "cert_fingerprint_sha256": guard.ticket.cert_fingerprint_sha256,
        "pake_salt_hex": guard.ticket.pake_salt_hex,
        "masked_receiver_dh_pub_hex": guard.ticket.masked_receiver_dh_pub_hex,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    })))
}

async fn handle_pair_request(
    State(state): State<Arc<Mutex<ReceiverServerState>>>,
    Json(payload): Json<PairingRequestPayload>,
) -> Result<Json<PairingApprovalResponse>, (StatusCode, String)> {
    let mut guard = state.lock().unwrap();
    let ticket_clone = guard.ticket.clone();

    let (recv_confirm_opt, sas_code) =
        match verify_pairing_request(&ticket_clone, &mut guard.limiter, &payload) {
            Ok(res) => res,
            Err(e) => return Err((StatusCode::UNAUTHORIZED, e.to_payload().message)),
        };

    let poll_token = generate_bearer_token();
    guard.pending_request = Some(payload);
    guard.pending_poll_token = Some(poll_token.clone());
    guard.pending_receiver_confirm_hex = recv_confirm_opt.clone();
    guard.pending_sas_code = Some(sas_code.clone());
    guard.approved_bearer_token = None;

    if guard.auto_approve_pairing {
        guard.approved_bearer_token = Some(generate_bearer_token());
    }

    let approved = guard.approved_bearer_token.is_some();
    Ok(Json(PairingApprovalResponse {
        approved,
        poll_token: Some(poll_token),
        session_bearer_token: guard.approved_bearer_token.clone(),
        receiver_cert_fingerprint: guard.ticket.cert_fingerprint_sha256.clone(),
        receiver_confirm_proof_hex: recv_confirm_opt,
        sas_verification_code: Some(sas_code),
        message: if approved {
            "Pairing verified and approved.".to_string()
        } else {
            "Pairing verified. Waiting for user confirmation on receiving computer.".to_string()
        },
    }))
}

async fn handle_pair_poll(
    State(state): State<Arc<Mutex<ReceiverServerState>>>,
    headers: HeaderMap,
) -> Result<Json<PairingApprovalResponse>, (StatusCode, String)> {
    let mut guard = state.lock().unwrap();
    if guard.pending_request.is_none() {
        return Err((
            StatusCode::FORBIDDEN,
            "Pairing request was rejected or not initiated.".to_string(),
        ));
    }

    let Some(ref expected_poll_token) = guard.pending_poll_token else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "No active pairing poll token.".to_string(),
        ));
    };

    let provided_poll_token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.strip_prefix("Poll ")
                .or_else(|| s.strip_prefix("Bearer "))
        })
        .or_else(|| {
            headers
                .get("X-Pairing-Poll-Token")
                .and_then(|v| v.to_str().ok())
        })
        .unwrap_or("");

    if provided_poll_token.is_empty()
        || !constant_time_eq(
            provided_poll_token.as_bytes(),
            expected_poll_token.as_bytes(),
        )
    {
        guard.limiter.record_failure();
        return Err((
            StatusCode::UNAUTHORIZED,
            "Invalid or missing pairing poll token.".to_string(),
        ));
    }

    let approved = guard.approved_bearer_token.is_some();
    Ok(Json(PairingApprovalResponse {
        approved,
        poll_token: Some(expected_poll_token.clone()),
        session_bearer_token: guard.approved_bearer_token.clone(),
        receiver_cert_fingerprint: guard.ticket.cert_fingerprint_sha256.clone(),
        receiver_confirm_proof_hex: guard.pending_receiver_confirm_hex.clone(),
        sas_verification_code: guard.pending_sas_code.clone(),
        message: if approved {
            "Approved".to_string()
        } else {
            "Pending user approval".to_string()
        },
    }))
}

fn verify_bearer_header(
    headers: &HeaderMap,
    expected_token: &Option<String>,
) -> Result<(), (StatusCode, String)> {
    let Some(ref expected) = expected_token else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Pairing has not been approved by the receiver.".to_string(),
        ));
    };
    let provided = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if provided != expected {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Invalid or expired session bearer token.".to_string(),
        ));
    }
    Ok(())
}

async fn handle_capabilities(
    State(state): State<Arc<Mutex<ReceiverServerState>>>,
    headers: HeaderMap,
    Json(sender_caps): Json<CapabilitiesHandshake>,
) -> Result<Json<CapabilitiesHandshake>, (StatusCode, String)> {
    let guard = state.lock().unwrap();
    verify_bearer_header(&headers, &guard.approved_bearer_token)?;

    let receiver_caps = CapabilitiesHandshake::default();
    if sender_caps.protocol_version != receiver_caps.protocol_version {
        return Err((
            StatusCode::PRECONDITION_FAILED,
            format!(
                "Protocol version mismatch: sender={}, receiver={}",
                sender_caps.protocol_version, receiver_caps.protocol_version
            ),
        ));
    }

    Ok(Json(receiver_caps))
}

async fn handle_manifest(
    State(state): State<Arc<Mutex<ReceiverServerState>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<TransferResumeStatus>, (StatusCode, String)> {
    let mut guard = state.lock().unwrap();
    verify_bearer_header(&headers, &guard.approved_bearer_token)?;

    if body.len() as u64 > guard.limits.max_manifest_bytes {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "Manifest exceeds maximum size limit.".to_string(),
        ));
    }

    let mut hasher = Sha256::new();
    hasher.update(&body);
    let manifest_hash = hex::encode(hasher.finalize());

    let manifest: MigrationManifest = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid manifest JSON: {e}"),
        )
    })?;

    if manifest.format_version != CURRENT_FORMAT_VERSION {
        return Err((
            StatusCode::PRECONDITION_FAILED,
            format!(
                "Unsupported manifest format version: {}",
                manifest.format_version
            ),
        ));
    }

    // Bind (migration_id, manifest_hash) to support safe resume and reject mixed snapshots
    if let (Some(ref cur_id), Some(ref cur_hash)) =
        (&guard.active_migration_id, &guard.active_manifest_hash)
    {
        if cur_id != &manifest.migration_id || cur_hash != &manifest_hash {
            return Err((
                StatusCode::CONFLICT,
                "Snapshot mismatch: cannot resume transfer with a different migration_id or modified manifest."
                    .to_string(),
            ));
        }
    } else {
        guard.active_migration_id = Some(manifest.migration_id.clone());
        guard.active_manifest_hash = Some(manifest_hash.clone());
        guard.total_bytes = manifest.files.iter().map(|f| f.size_bytes).sum();
        let manifest_path = guard.staging_dir.join("manifest.json");
        std::fs::write(&manifest_path, &body)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // Mark 0-byte or symlink entries as already completed
        for f in &manifest.files {
            if f.symlink.is_some() {
                guard.completed_files.push(f.entry_id.clone());
            } else if f.size_bytes == 0 {
                if let Ok(dst) = resolve_safe_child_path(
                    &guard.staging_dir,
                    &f.archive_rel_path,
                    guard.limits.max_path_length,
                ) {
                    if let Some(p) = dst.parent() {
                        let _ = std::fs::create_dir_all(p);
                    }
                    let _ = std::fs::File::create(&dst);
                    guard.completed_files.push(f.entry_id.clone());
                }
            }
        }
        guard.manifest = Some(manifest.clone());
    }

    Ok(Json(TransferResumeStatus {
        migration_id: manifest.migration_id,
        manifest_hash,
        file_offsets: guard.file_offsets.clone(),
        completed_files: guard.completed_files.clone(),
    }))
}

#[derive(Deserialize)]
struct StatusQuery {
    migration_id: String,
    manifest_hash: String,
}

async fn handle_transfer_status(
    State(state): State<Arc<Mutex<ReceiverServerState>>>,
    headers: HeaderMap,
    Query(q): Query<StatusQuery>,
) -> Result<Json<TransferResumeStatus>, (StatusCode, String)> {
    let guard = state.lock().unwrap();
    verify_bearer_header(&headers, &guard.approved_bearer_token)?;

    if guard.active_migration_id.as_deref() != Some(q.migration_id.as_str())
        || guard.active_manifest_hash.as_deref() != Some(q.manifest_hash.as_str())
    {
        return Err((
            StatusCode::CONFLICT,
            "Snapshot mismatch during resume status check.".to_string(),
        ));
    }

    Ok(Json(TransferResumeStatus {
        migration_id: q.migration_id,
        manifest_hash: q.manifest_hash,
        file_offsets: guard.file_offsets.clone(),
        completed_files: guard.completed_files.clone(),
    }))
}

async fn handle_transfer_chunk(
    State(state): State<Arc<Mutex<ReceiverServerState>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut guard = state.lock().unwrap();
    verify_bearer_header(&headers, &guard.approved_bearer_token)?;

    let mig_id = headers
        .get("X-Migration-Id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let entry_id = headers
        .get("X-Entry-Id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let offset: u64 = headers
        .get("X-Chunk-Offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let expected_chunk_sha = headers
        .get("X-Chunk-Sha256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if guard.active_migration_id.as_deref() != Some(mig_id) {
        return Err((StatusCode::CONFLICT, "Migration ID mismatch.".to_string()));
    }

    let mut chunk_hasher = Sha256::new();
    chunk_hasher.update(&body);
    let actual_chunk_sha = hex::encode(chunk_hasher.finalize());
    if !actual_chunk_sha.eq_ignore_ascii_case(expected_chunk_sha) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Chunk SHA-256 verification failed.".to_string(),
        ));
    }

    let Some(ref manifest) = guard.manifest else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Manifest not uploaded.".to_string(),
        ));
    };
    let Some(file_entry) = manifest
        .files
        .iter()
        .find(|f| f.entry_id == entry_id)
        .cloned()
    else {
        return Err((StatusCode::NOT_FOUND, "Unknown file entry_id.".to_string()));
    };

    let current_offset = guard.file_offsets.get(entry_id).copied().unwrap_or(0);
    if offset != current_offset {
        return Err((
            StatusCode::CONFLICT,
            format!("Unexpected chunk offset: expected {current_offset}, got {offset}"),
        ));
    }

    let dst_path = resolve_safe_child_path(
        &guard.staging_dir,
        &file_entry.archive_rel_path,
        guard.limits.max_path_length,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_payload().message))?;

    if let Some(parent) = dst_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&dst_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    file.write_all(&body)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    file.flush()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let new_offset = offset + body.len() as u64;
    guard.file_offsets.insert(entry_id.to_string(), new_offset);
    guard.received_bytes += body.len() as u64;

    if new_offset >= file_entry.size_bytes {
        let actual_file_sha = crate::inventory::compute_file_sha256(&dst_path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_payload().message))?;
        if !actual_file_sha.eq_ignore_ascii_case(&file_entry.sha256) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Full file SHA-256 mismatch for {}",
                    file_entry.archive_rel_path
                ),
            ));
        }
        if !guard.completed_files.contains(&entry_id.to_string()) {
            guard.completed_files.push(entry_id.to_string());
        }
    }

    Ok(Json(serde_json::json!({
        "entry_id": entry_id,
        "new_offset": new_offset,
        "completed": new_offset >= file_entry.size_bytes,
    })))
}

async fn handle_transfer_complete(
    State(state): State<Arc<Mutex<ReceiverServerState>>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut guard = state.lock().unwrap();
    verify_bearer_header(&headers, &guard.approved_bearer_token)?;

    verify_staging_snapshot(&guard.staging_dir, &guard.limits)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_payload().message))?;

    guard.transfer_completed = true;
    Ok(Json(serde_json::json!({
        "status": "verified",
        "files_verified": guard.completed_files.len(),
    })))
}

/// Custom `ServerCertVerifier` that strictly pins the peer's X.509 certificate SHA-256 fingerprint!
/// Never accepts arbitrary untrusted certificates unless the exact SHA-256 matches `expected_fingerprint_sha256`.
#[derive(Debug)]
struct PinnedFingerprintCertVerifier {
    expected_fingerprint_sha256: String,
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedFingerprintCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let actual_hex = hex::encode(hasher.finalize());
        if actual_hex.eq_ignore_ascii_case(&self.expected_fingerprint_sha256) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "TLS certificate fingerprint mismatch: expected {}, got {}",
                self.expected_fingerprint_sha256, actual_hex
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.crypto_provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Custom `ServerCertVerifier` used ONLY during initial `/ping` metadata discovery when the user
/// enters a manual IP + 6-character short code without UDP multicast discovery.
/// Records the exact SHA-256 fingerprint of the TLS server's leaf certificate so the subsequent
/// X25519 PAKE handshake is cryptographically bound to the actual TLS channel!
#[derive(Debug)]
struct RecordingCertVerifier {
    recorded_fingerprint: Arc<Mutex<Option<String>>>,
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for RecordingCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let mut hasher = Sha256::new();
        hasher.update(end_entity.as_ref());
        let actual_hex = hex::encode(hasher.finalize());
        if let Ok(mut guard) = self.recorded_fingerprint.lock() {
            *guard = Some(actual_hex);
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.crypto_provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiverPingMetadata {
    pub cert_fingerprint_sha256: String,
    pub pake_salt_hex: String,
    #[serde(default)]
    pub masked_receiver_dh_pub_hex: String,
}

/// Fetches `/api/codex-move/v1/ping` and verifies that the server's TLS leaf certificate fingerprint
/// matches the JSON payload (and `expected_fingerprint` if known).
pub async fn fetch_receiver_ping_metadata(
    receiver_base_url: &str,
    expected_fingerprint: Option<&str>,
) -> AppResult<ReceiverPingMetadata> {
    let recorded_fp = Arc::new(Mutex::new(None));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(RecordingCertVerifier {
        recorded_fingerprint: recorded_fp.clone(),
        crypto_provider: provider.clone(),
    });

    let tls_config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| AppError::new(ErrorCode::NetworkError, e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    let client = reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::new(ErrorCode::NetworkError, e.to_string()))?;

    let url = format!("{receiver_base_url}/api/codex-move/v1/ping");
    let resp = client.get(&url).send().await.map_err(|e| {
        AppError::new(
            ErrorCode::NetworkError,
            format!("Failed to reach receiver at {url}: {e}"),
        )
    })?;

    if !resp.status().is_success() {
        return Err(AppError::new(
            ErrorCode::NetworkError,
            format!("Receiver ping failed with HTTP {}", resp.status()),
        ));
    }

    let meta: ReceiverPingMetadata = resp
        .json()
        .await
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;

    let tls_leaf_fp = recorded_fp
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default();

    if tls_leaf_fp.is_empty() || !tls_leaf_fp.eq_ignore_ascii_case(&meta.cert_fingerprint_sha256) {
        return Err(AppError::new(
            ErrorCode::TlsFingerprintMismatch,
            "TLS leaf certificate does not match receiver ping metadata! Potential MITM blocked.",
        ));
    }

    if let Some(exp) = expected_fingerprint {
        if !exp.trim().is_empty() && !tls_leaf_fp.eq_ignore_ascii_case(exp.trim()) {
            return Err(AppError::new(
                ErrorCode::TlsFingerprintMismatch,
                "TLS certificate fingerprint mismatch against discovered peer!",
            ));
        }
    }

    Ok(meta)
}

/// Builds an HTTPS `reqwest::Client` pinned strictly to `expected_cert_fingerprint_sha256`.
pub fn build_pinned_https_client(
    expected_cert_fingerprint_sha256: &str,
) -> AppResult<reqwest::Client> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedFingerprintCertVerifier {
        expected_fingerprint_sha256: expected_cert_fingerprint_sha256.to_string(),
        crypto_provider: provider.clone(),
    });

    let tls_config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| AppError::new(ErrorCode::NetworkError, e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    let client = reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::new(ErrorCode::NetworkError, e.to_string()))?;

    Ok(client)
}

/// Sender-side pairing handshake over pinned HTTPS.
/// Supports both HighEntropyToken (`cdxmove://...`) and 6-character ShortCodeArgon2 X25519 PAKE.
#[allow(clippy::too_many_arguments)]
pub async fn sender_request_pairing(
    receiver_base_url: &str,
    expected_cert_fingerprint: &str,
    auth_mode: PairingAuthMode,
    secret_or_short_code: &str,
    pake_salt_hex: &str,
    sender_device_name: &str,
    summary_threads_count: usize,
    summary_projects_count: usize,
    summary_total_bytes: u64,
) -> AppResult<PairingApprovalResponse> {
    let mut nonce_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let sender_nonce_hex = hex::encode(nonce_bytes);

    let (pinned_fp, req_payload, expected_recv_confirm_opt, expected_sas) = match auth_mode {
        PairingAuthMode::HighEntropyToken => {
            let hmac_proof_hex = compute_pairing_hmac_proof(
                auth_mode,
                secret_or_short_code,
                pake_salt_hex,
                expected_cert_fingerprint,
                &sender_nonce_hex,
            )?;
            let sas = derive_sas_verification_code(
                hmac_proof_hex.as_bytes(),
                expected_cert_fingerprint,
                &sender_nonce_hex,
            );
            let payload = PairingRequestPayload {
                sender_device_name: sender_device_name.to_string(),
                sender_os: std::env::consts::OS.to_string(),
                sender_arch: std::env::consts::ARCH.to_string(),
                sender_nonce_hex,
                auth_mode,
                hmac_proof_hex,
                sender_pake_pub_hex: None,
                summary_threads_count,
                summary_projects_count,
                summary_total_bytes,
            };
            (expected_cert_fingerprint.to_string(), payload, None, sas)
        }
        PairingAuthMode::ShortCodeArgon2 => {
            let exp_fp_opt = if expected_cert_fingerprint.trim().is_empty() {
                None
            } else {
                Some(expected_cert_fingerprint)
            };
            let ping_meta = fetch_receiver_ping_metadata(receiver_base_url, exp_fp_opt).await?;
            let (sender_pake_pub_hex, sender_proof_hex, expected_recv_confirm_hex, sas) =
                create_sender_pake_handshake(
                    secret_or_short_code,
                    &ping_meta.pake_salt_hex,
                    &ping_meta.masked_receiver_dh_pub_hex,
                    &ping_meta.cert_fingerprint_sha256,
                    &sender_nonce_hex,
                )?;
            let payload = PairingRequestPayload {
                sender_device_name: sender_device_name.to_string(),
                sender_os: std::env::consts::OS.to_string(),
                sender_arch: std::env::consts::ARCH.to_string(),
                sender_nonce_hex,
                auth_mode,
                hmac_proof_hex: sender_proof_hex,
                sender_pake_pub_hex: Some(sender_pake_pub_hex),
                summary_threads_count,
                summary_projects_count,
                summary_total_bytes,
            };
            (
                ping_meta.cert_fingerprint_sha256,
                payload,
                Some(expected_recv_confirm_hex),
                sas,
            )
        }
    };

    let client = build_pinned_https_client(&pinned_fp)?;

    let url = format!("{receiver_base_url}/api/codex-move/v1/pair/request");
    let resp = client.post(&url).json(&req_payload).send().await.map_err(|e| {
        if e.to_string().contains("fingerprint mismatch") {
            AppError::new(
                ErrorCode::TlsFingerprintMismatch,
                "TLS certificate fingerprint mismatch! Connection blocked to prevent Man-in-the-Middle attack.",
            )
        } else {
            AppError::new(
                ErrorCode::NetworkError,
                format!("Failed to connect to receiver at {url}: {e}"),
            )
        }
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            format!("Pairing rejected ({status}): {text}"),
        ));
    }

    let mut approval = resp
        .json::<PairingApprovalResponse>()
        .await
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;

    if let Some(ref exp_confirm) = expected_recv_confirm_opt {
        let actual_confirm = approval.receiver_confirm_proof_hex.as_deref().unwrap_or("");
        if !constant_time_eq(exp_confirm.as_bytes(), actual_confirm.as_bytes()) {
            return Err(AppError::new(
                ErrorCode::PairingAuthenticationFailed,
                "Mutual PAKE confirmation failed: receiver could not prove knowledge of the 6-character code!",
            ));
        }
    }

    if approval.sas_verification_code.is_none() {
        approval.sas_verification_code = Some(expected_sas);
    }

    Ok(approval)
}

/// Sender-side authenticated pairing approval poll over pinned HTTPS.
pub async fn sender_poll_pairing_approval(
    receiver_base_url: &str,
    expected_cert_fingerprint: &str,
    poll_token: &str,
) -> AppResult<PairingApprovalResponse> {
    let client = build_pinned_https_client(expected_cert_fingerprint)?;
    let url = format!("{receiver_base_url}/api/codex-move/v1/pair/poll");
    let resp = client
        .get(&url)
        .header("Authorization", format!("Poll {poll_token}"))
        .send()
        .await
        .map_err(|e| AppError::new(ErrorCode::NetworkError, e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return Err(AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            format!("Pairing poll rejected ({status}): {txt}"),
        ));
    }

    resp.json::<PairingApprovalResponse>()
        .await
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))
}

/// Sender-side streaming transfer with bounded memory chunks (default 4 MiB) and automatic resume!
pub async fn sender_stream_snapshot_to_receiver<F>(
    receiver_base_url: &str,
    expected_cert_fingerprint: &str,
    bearer_token: &str,
    staging_dir: &Path,
    manifest: &MigrationManifest,
    limits: &SecurityLimits,
    abort_after_chunks_for_test: Option<usize>,
    mut on_progress: F,
) -> AppResult<()>
where
    F: FnMut(usize, usize, u64, u64, String),
{
    let client = build_pinned_https_client(expected_cert_fingerprint)?;
    let auth_header = format!("Bearer {bearer_token}");

    // 1. Exchange capabilities before transferring data
    let cap_url = format!("{receiver_base_url}/api/codex-move/v1/capabilities");
    let cap_resp = client
        .post(&cap_url)
        .header("Authorization", &auth_header)
        .json(&CapabilitiesHandshake::default())
        .send()
        .await
        .map_err(|e| AppError::new(ErrorCode::NetworkError, e.to_string()))?;

    if !cap_resp.status().is_success() {
        let txt = cap_resp.text().await.unwrap_or_default();
        return Err(AppError::new(
            ErrorCode::CapabilitiesIncompatible,
            format!("Capabilities negotiation failed: {txt}"),
        ));
    }

    // 2. Upload or resume manifest
    let manifest_bytes = std::fs::read(staging_dir.join("manifest.json"))?;
    let man_url = format!("{receiver_base_url}/api/codex-move/v1/manifest");
    let man_resp = client
        .post(&man_url)
        .header("Authorization", &auth_header)
        .body(manifest_bytes)
        .send()
        .await
        .map_err(|e| AppError::new(ErrorCode::NetworkError, e.to_string()))?;

    if !man_resp.status().is_success() {
        let status = man_resp.status();
        let txt = man_resp.text().await.unwrap_or_default();
        let code = if status == StatusCode::CONFLICT {
            ErrorCode::TransferSnapshotMismatch
        } else {
            ErrorCode::NetworkError
        };
        return Err(AppError::new(code, txt));
    }

    let resume_state: TransferResumeStatus = man_resp
        .json()
        .await
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;

    let total_files = manifest.files.len();
    let total_bytes: u64 = manifest.files.iter().map(|f| f.size_bytes).sum();
    let mut transferred_bytes: u64 = resume_state.file_offsets.values().sum();
    let mut chunks_sent = 0usize;

    let chunk_url = format!("{receiver_base_url}/api/codex-move/v1/transfer/chunk");

    for (idx, file_entry) in manifest.files.iter().enumerate() {
        if file_entry.symlink.is_some()
            || file_entry.size_bytes == 0
            || resume_state.completed_files.contains(&file_entry.entry_id)
        {
            continue;
        }

        let local_path = resolve_safe_child_path(
            staging_dir,
            &file_entry.archive_rel_path,
            limits.max_path_length,
        )?;
        let mut file = std::fs::File::open(&local_path)?;
        let mut offset = resume_state
            .file_offsets
            .get(&file_entry.entry_id)
            .copied()
            .unwrap_or(0);

        if offset > 0 {
            file.seek(SeekFrom::Start(offset))?;
        }

        let mut chunk_buf = vec![0u8; limits.stream_chunk_bytes];
        loop {
            let n = file.read(&mut chunk_buf)?;
            if n == 0 {
                break;
            }

            if let Some(limit_chunks) = abort_after_chunks_for_test {
                if chunks_sent >= limit_chunks {
                    return Err(AppError::new(
                        ErrorCode::TransferInterrupted,
                        "Simulated network interruption during chunked transfer.",
                    ));
                }
            }

            let slice = &chunk_buf[..n];
            let mut hasher = Sha256::new();
            hasher.update(slice);
            let chunk_sha = hex::encode(hasher.finalize());

            let resp = client
                .put(&chunk_url)
                .header("Authorization", &auth_header)
                .header("X-Migration-Id", &manifest.migration_id)
                .header("X-Entry-Id", &file_entry.entry_id)
                .header("X-Chunk-Offset", offset.to_string())
                .header("X-Chunk-Sha256", chunk_sha)
                .body(slice.to_vec())
                .send()
                .await
                .map_err(|e| AppError::new(ErrorCode::TransferInterrupted, e.to_string()))?;

            if !resp.status().is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(AppError::new(ErrorCode::TransferInterrupted, txt));
            }

            offset += n as u64;
            transferred_bytes += n as u64;
            chunks_sent += 1;

            on_progress(
                idx + 1,
                total_files,
                transferred_bytes,
                total_bytes,
                file_entry.logical_rel_path.clone(),
            );
        }
    }

    // 3. Finalize and trigger receiver-side whole-snapshot verification
    let complete_url = format!("{receiver_base_url}/api/codex-move/v1/transfer/complete");
    let complete_resp = client
        .post(&complete_url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .map_err(|e| AppError::new(ErrorCode::NetworkError, e.to_string()))?;

    if !complete_resp.status().is_success() {
        let txt = complete_resp.text().await.unwrap_or_default();
        return Err(AppError::new(ErrorCode::TransferInterrupted, txt));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::create_staging_snapshot;
    use crate::inventory::{scan_codex_inventory, ScanOptions};
    use crate::pairing::{create_receiver_pairing_ticket, generate_ephemeral_tls_identity};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_two_instance_https_pinned_transfer_and_resume_after_disconnect() {
        let dir = tempdir().unwrap();
        let src_codex = dir.path().join("src_codex");
        let sender_staging = dir.path().join("sender_staging");
        let receiver_staging = dir.path().join("receiver_staging");

        std::fs::create_dir_all(src_codex.join("sessions")).unwrap();
        // Write a 300 KB session file so it spans multiple 64 KB chunks
        let mut big_lines = String::from(
            r#"{"timestamp":"2026-10-01T00:00:00Z","ordinal":0,"type":"session_meta","payload":{"session_id":"019a","id":"019a","cwd":"/Users/alice/app","model_provider":"openai"}}"#,
        );
        big_lines.push('\n');
        for i in 1..1500 {
            big_lines.push_str(&format!(
                r#"{{"timestamp":"2026-10-01T00:00:01Z","ordinal":{i},"type":"event_msg","payload":{{"type":"token_count","turn_id":"t-{i}"}}}}"#
            ));
            big_lines.push('\n');
        }
        std::fs::write(
            src_codex.join("sessions").join("rollout-1.jsonl"),
            big_lines,
        )
        .unwrap();

        let mut limits = SecurityLimits::default();
        limits.stream_chunk_bytes = 64 * 1024; // 64 KiB chunks for multi-chunk resume test

        let scan_opts = ScanOptions {
            codex_home: src_codex.to_string_lossy().to_string(),
            shared_skills_dir: None,
            include_project_files: false,
            excluded_project_dirs: vec![],
            compute_sha256: true,
        };
        let mut manifest = scan_codex_inventory(&scan_opts, &limits).unwrap();
        create_staging_snapshot(&mut manifest, &sender_staging, &limits).unwrap();

        // Start Receiver HTTPS server on 127.0.0.1:0 with manual approval required (auto_approve_pairing = false)
        let tls = generate_ephemeral_tls_identity().unwrap();
        let ticket =
            create_receiver_pairing_ticket("sess-test", "127.0.0.1", 0, &tls, &limits).unwrap();
        let server = start_https_receiver_server(
            "127.0.0.1:0".parse().unwrap(),
            &tls,
            ticket.clone(),
            receiver_staging.clone(),
            limits.clone(),
            false,
        )
        .await
        .unwrap();

        let base_url = format!("https://127.0.0.1:{}", server.addr.port());

        // 1. Verify MITM protection: wrong certificate fingerprint MUST be rejected
        let bad_fp_res = sender_request_pairing(
            &base_url,
            "0000000000000000000000000000000000000000000000000000000000000000",
            PairingAuthMode::HighEntropyToken,
            &ticket.high_entropy_token_b64,
            &ticket.pake_salt_hex,
            "Sender-PC",
            1,
            0,
            1000,
        )
        .await;
        assert!(bad_fp_res.is_err());

        // 1b. Verify Manual IP + 6-character short code X25519 PAKE handshake works even without prior UDP discovery!
        let short_pake_resp = sender_request_pairing(
            &base_url,
            "",
            PairingAuthMode::ShortCodeArgon2,
            &ticket.short_pake_code,
            "",
            "Sender-PC-ShortCode",
            1,
            0,
            1000,
        )
        .await
        .unwrap();
        assert!(!short_pake_resp.approved);
        assert!(short_pake_resp.receiver_confirm_proof_hex.is_some());
        assert_eq!(
            short_pake_resp.sas_verification_code,
            server.snapshot().sas_verification_code
        );

        // 2. Pair with pinned fingerprint -> receives poll_token while awaiting user confirmation
        let initial_resp = sender_request_pairing(
            &base_url,
            &tls.cert_fingerprint_sha256,
            PairingAuthMode::HighEntropyToken,
            &ticket.high_entropy_token_b64,
            &ticket.pake_salt_hex,
            "Sender-PC",
            1,
            0,
            1000,
        )
        .await
        .unwrap();
        assert!(!initial_resp.approved);
        assert!(initial_resp.session_bearer_token.is_none());
        let poll_token = initial_resp.poll_token.expect("poll_token must be issued");

        // 2b. Receiver approves the pending request
        server.approve_pending_sender().unwrap();

        // 2c. Unauthenticated or wrong-token LAN peer polling MUST be rejected with 401!
        let unauth_poll =
            sender_poll_pairing_approval(&base_url, &tls.cert_fingerprint_sha256, "").await;
        assert!(unauth_poll.is_err());
        let forged_poll = sender_poll_pairing_approval(
            &base_url,
            &tls.cert_fingerprint_sha256,
            "forged-invalid-poll-token",
        )
        .await;
        assert!(forged_poll.is_err());

        // 2d. Authenticated sender polling with valid poll_token receives session_bearer_token
        let approval =
            sender_poll_pairing_approval(&base_url, &tls.cert_fingerprint_sha256, &poll_token)
                .await
                .unwrap();
        assert!(approval.approved);
        let bearer = approval.session_bearer_token.unwrap();

        // 3. Simulate network drop after 2 chunks
        let interrupted = sender_stream_snapshot_to_receiver(
            &base_url,
            &tls.cert_fingerprint_sha256,
            &bearer,
            &sender_staging,
            &manifest,
            &limits,
            Some(2),
            |_, _, _, _, _| {},
        )
        .await;
        assert!(interrupted.is_err());

        let snap_mid = server.snapshot();
        assert_eq!(snap_mid.received_bytes, 2 * 64 * 1024);
        assert!(!snap_mid.transfer_completed);

        // 4. Resume transfer from exact byte offset 131072 and complete
        sender_stream_snapshot_to_receiver(
            &base_url,
            &tls.cert_fingerprint_sha256,
            &bearer,
            &sender_staging,
            &manifest,
            &limits,
            None,
            |_, _, _, _, _| {},
        )
        .await
        .unwrap();

        let snap_final = server.snapshot();
        assert!(snap_final.transfer_completed);
        server.stop();
    }
}
