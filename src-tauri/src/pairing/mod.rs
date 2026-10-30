use crate::config::SecurityLimits;
use crate::errors::{AppError, AppResult, ErrorCode};
use argon2::{Argon2, Params};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use qrcode::{render::svg, QrCode};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use x25519_dalek::{PublicKey, StaticSecret};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EphemeralTlsIdentity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
    pub cert_fingerprint_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiverPairingTicket {
    pub session_id: String,
    pub host_ip: String,
    pub port: u16,
    pub cert_fingerprint_sha256: String,
    pub high_entropy_token_b64: String,
    pub short_pake_code: String,
    pub pake_salt_hex: String,
    #[serde(default)]
    pub masked_receiver_dh_pub_hex: String,
    #[serde(skip)]
    pub receiver_dh_private_hex: String,
    pub connection_code_string: String,
    pub qr_svg: String,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequestPayload {
    pub sender_device_name: String,
    pub sender_os: String,
    pub sender_arch: String,
    pub sender_nonce_hex: String,
    pub auth_mode: PairingAuthMode,
    pub hmac_proof_hex: String,
    #[serde(default)]
    pub sender_pake_pub_hex: Option<String>,
    pub summary_threads_count: usize,
    pub summary_projects_count: usize,
    pub summary_total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingAuthMode {
    HighEntropyToken,
    ShortCodeArgon2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingApprovalResponse {
    pub approved: bool,
    pub poll_token: Option<String>,
    pub session_bearer_token: Option<String>,
    pub receiver_cert_fingerprint: String,
    #[serde(default)]
    pub receiver_confirm_proof_hex: Option<String>,
    #[serde(default)]
    pub sas_verification_code: Option<String>,
    pub message: String,
}

pub struct PairingRateLimiter {
    pub created_at: Instant,
    pub ttl: Duration,
    pub failed_attempts: u32,
    pub max_attempts: u32,
    pub locked_until: Option<Instant>,
}

impl PairingRateLimiter {
    pub fn new(limits: &SecurityLimits) -> Self {
        Self {
            created_at: Instant::now(),
            ttl: Duration::from_secs(limits.pairing_ttl_seconds),
            failed_attempts: 0,
            max_attempts: limits.max_pairing_attempts,
            locked_until: None,
        }
    }

    pub fn check_allowed(&self) -> AppResult<()> {
        if self.created_at.elapsed() > self.ttl {
            return Err(AppError::new(
                ErrorCode::PairingExpired,
                "Pairing code has expired. Generate a new pairing code on the receiving computer.",
            ));
        }
        if self.failed_attempts >= self.max_attempts {
            return Err(AppError::new(
                ErrorCode::PairingRateLimited,
                "Too many failed pairing attempts. Please generate a new pairing code on the receiving computer.",
            ));
        }
        if let Some(until) = self.locked_until {
            if Instant::now() < until {
                let wait_secs = (until - Instant::now()).as_secs().max(1);
                return Err(AppError::new(
                    ErrorCode::PairingRateLimited,
                    format!("Pairing rate-limited. Please wait {wait_secs}s before retrying."),
                ));
            }
        }
        Ok(())
    }

    pub fn record_failure(&mut self) {
        self.failed_attempts += 1;
        let backoff_secs = 1u64 << self.failed_attempts.min(5);
        self.locked_until = Some(Instant::now() + Duration::from_secs(backoff_secs));
    }
}

/// Generates an ephemeral self-signed X.509 certificate and SHA-256 fingerprint using `rcgen`.
pub fn generate_ephemeral_tls_identity() -> AppResult<EphemeralTlsIdentity> {
    let subject_alt_names = vec![
        "codex-move.local".to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ];
    let certified_key = rcgen::generate_simple_self_signed(subject_alt_names).map_err(|e| {
        AppError::new(
            ErrorCode::NetworkError,
            format!("Failed to generate TLS certificate: {e}"),
        )
    })?;

    let cert_der = certified_key.cert.der().to_vec();
    let key_der = certified_key.key_pair.serialize_der();

    let mut hasher = Sha256::new();
    hasher.update(&cert_der);
    let cert_fingerprint_sha256 = hex::encode(hasher.finalize());

    Ok(EphemeralTlsIdentity {
        cert_der,
        key_der,
        cert_fingerprint_sha256,
    })
}

/// Creates a receiver pairing ticket with:
/// 1. High-entropy 256-bit token + certificate fingerprint encoded into a QR SVG and copyable connection string.
/// 2. 6-character alphanumeric code backed by a 128-bit salt, Argon2id key derivation bound to the TLS certificate fingerprint,
///    and an X25519 Encrypted Key Exchange (PAKE) ephemeral keypair that prevents offline dictionary attacks.
pub fn create_receiver_pairing_ticket(
    session_id: &str,
    host_ip: &str,
    port: u16,
    tls_identity: &EphemeralTlsIdentity,
    limits: &SecurityLimits,
) -> AppResult<ReceiverPairingTicket> {
    let mut token_bytes = [0u8; 32];
    let mut salt_bytes = [0u8; 16];
    let mut short_bytes = [0u8; 6];
    let mut dh_priv_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut token_bytes);
    rand::thread_rng().fill_bytes(&mut salt_bytes);
    rand::thread_rng().fill_bytes(&mut short_bytes);
    rand::thread_rng().fill_bytes(&mut dh_priv_bytes);

    let high_entropy_token_b64 = URL_SAFE_NO_PAD.encode(token_bytes);
    let pake_salt_hex = hex::encode(salt_bytes);

    // Readable unambiguous 6-char code (no 0/O/1/I)
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let short_pake_code: String = short_bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect();

    let receiver_secret = StaticSecret::from(dh_priv_bytes);
    let receiver_pub = PublicKey::from(&receiver_secret);
    let pw_mask = derive_argon2_password_mask(
        &short_pake_code,
        &pake_salt_hex,
        &tls_identity.cert_fingerprint_sha256,
        b"receiver-mask",
    )?;
    let masked_receiver_pub = xor_32(receiver_pub.as_bytes(), &pw_mask);
    let masked_receiver_dh_pub_hex = hex::encode(masked_receiver_pub);
    let receiver_dh_private_hex = hex::encode(dh_priv_bytes);

    let connection_code_string = format!(
        "cdxmove://{}@{}:{}/{}?t={}",
        session_id, host_ip, port, tls_identity.cert_fingerprint_sha256, high_entropy_token_b64
    );

    let qr = QrCode::new(connection_code_string.as_bytes()).map_err(|e| {
        AppError::new(
            ErrorCode::SerializationError,
            format!("QR generation error: {e}"),
        )
    })?;
    let qr_svg = qr
        .render::<svg::Color>()
        .min_dimensions(200, 200)
        .dark_color(svg::Color("#0f172a"))
        .light_color(svg::Color("#ffffff"))
        .build();

    Ok(ReceiverPairingTicket {
        session_id: session_id.to_string(),
        host_ip: host_ip.to_string(),
        port,
        cert_fingerprint_sha256: tls_identity.cert_fingerprint_sha256.clone(),
        high_entropy_token_b64,
        short_pake_code,
        pake_salt_hex,
        masked_receiver_dh_pub_hex,
        receiver_dh_private_hex,
        connection_code_string,
        qr_svg,
        expires_in_seconds: limits.pairing_ttl_seconds,
    })
}

fn xor_32(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = a[i] ^ b[i];
    }
    out
}

pub fn derive_argon2_password_mask(
    short_code: &str,
    pake_salt_hex: &str,
    receiver_cert_fingerprint: &str,
    role_tag: &[u8],
) -> AppResult<[u8; 32]> {
    let mut base_key = [0u8; 32];
    let composite_salt = format!("{pake_salt_hex}:{receiver_cert_fingerprint}");
    let params = Params::new(19456, 2, 1, Some(32))
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    argon2
        .hash_password_into(
            short_code.trim().to_ascii_uppercase().as_bytes(),
            composite_salt.as_bytes(),
            &mut base_key,
        )
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;

    let mut mac = HmacSha256::new_from_slice(&base_key)
        .map_err(|_| AppError::new(ErrorCode::SerializationError, "HMAC init failed."))?;
    mac.update(role_tag);
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    Ok(out)
}

/// Derives a 6-digit Short Authentication String (SAS) code from the pairing transcript and shared key
/// so both Sender and Receiver can visually confirm they are connected to the same session.
pub fn derive_sas_verification_code(
    shared_key: &[u8],
    receiver_cert_fingerprint: &str,
    sender_nonce_hex: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-move-sas-v1:");
    hasher.update(shared_key);
    hasher.update(b":");
    hasher.update(receiver_cert_fingerprint.as_bytes());
    hasher.update(b":");
    hasher.update(sender_nonce_hex.as_bytes());
    let digest = hasher.finalize();
    let val = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{val:06}")
}

/// Sender-side X25519 Encrypted Key Exchange (PAKE) computation for 6-character short code mode.
/// Returns `(masked_sender_pake_pub_hex, sender_hmac_proof_hex, expected_receiver_confirm_proof_hex, sas_code)`.
pub fn create_sender_pake_handshake(
    short_code: &str,
    pake_salt_hex: &str,
    masked_receiver_dh_pub_hex: &str,
    receiver_cert_fingerprint: &str,
    sender_nonce_hex: &str,
) -> AppResult<(String, String, String, String)> {
    let masked_recv_bytes = hex::decode(masked_receiver_dh_pub_hex.trim()).map_err(|_| {
        AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Invalid receiver PAKE public key encoding.",
        )
    })?;
    if masked_recv_bytes.len() != 32 {
        return Err(AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Invalid receiver PAKE public key length.",
        ));
    }
    let mut masked_recv_arr = [0u8; 32];
    masked_recv_arr.copy_from_slice(&masked_recv_bytes);

    let recv_mask = derive_argon2_password_mask(
        short_code,
        pake_salt_hex,
        receiver_cert_fingerprint,
        b"receiver-mask",
    )?;
    let recv_pub_bytes = xor_32(&masked_recv_arr, &recv_mask);
    let recv_pub = PublicKey::from(recv_pub_bytes);

    let mut sender_priv_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut sender_priv_bytes);
    let sender_secret = StaticSecret::from(sender_priv_bytes);
    let sender_pub = PublicKey::from(&sender_secret);

    let sender_mask = derive_argon2_password_mask(
        short_code,
        pake_salt_hex,
        receiver_cert_fingerprint,
        b"sender-mask",
    )?;
    let masked_sender_pub = xor_32(sender_pub.as_bytes(), &sender_mask);
    let masked_sender_pub_hex = hex::encode(masked_sender_pub);

    let dh_shared = sender_secret.diffie_hellman(&recv_pub);
    let pw_auth = derive_argon2_password_mask(
        short_code,
        pake_salt_hex,
        receiver_cert_fingerprint,
        b"pake-auth",
    )?;

    let mut session_hasher = Sha256::new();
    session_hasher.update(dh_shared.as_bytes());
    session_hasher.update(pw_auth);
    session_hasher.update(receiver_cert_fingerprint.as_bytes());
    session_hasher.update(masked_receiver_dh_pub_hex.as_bytes());
    session_hasher.update(masked_sender_pub_hex.as_bytes());
    session_hasher.update(sender_nonce_hex.as_bytes());
    let pake_session_key = session_hasher.finalize();

    let mut sender_mac = HmacSha256::new_from_slice(&pake_session_key)
        .map_err(|_| AppError::new(ErrorCode::PairingAuthenticationFailed, "HMAC init failed"))?;
    sender_mac.update(b"sender-confirm:");
    sender_mac.update(receiver_cert_fingerprint.as_bytes());
    sender_mac.update(b":");
    sender_mac.update(sender_nonce_hex.as_bytes());
    let sender_proof_hex = hex::encode(sender_mac.finalize().into_bytes());

    let mut recv_mac = HmacSha256::new_from_slice(&pake_session_key)
        .map_err(|_| AppError::new(ErrorCode::PairingAuthenticationFailed, "HMAC init failed"))?;
    recv_mac.update(b"receiver-confirm:");
    recv_mac.update(receiver_cert_fingerprint.as_bytes());
    recv_mac.update(b":");
    recv_mac.update(sender_nonce_hex.as_bytes());
    let expected_recv_confirm_hex = hex::encode(recv_mac.finalize().into_bytes());

    let sas_code = derive_sas_verification_code(
        &pake_session_key,
        receiver_cert_fingerprint,
        sender_nonce_hex,
    );

    Ok((
        masked_sender_pub_hex,
        sender_proof_hex,
        expected_recv_confirm_hex,
        sas_code,
    ))
}

/// Parses a `cdxmove://session@ip:port/fingerprint?t=token` connection string.
pub fn parse_connection_code_string(
    code_str: &str,
) -> AppResult<(String, String, u16, String, String)> {
    let trimmed = code_str.trim();
    let without_scheme = trimmed.strip_prefix("cdxmove://").ok_or_else(|| {
        AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Connection code must start with cdxmove://",
        )
    })?;

    let (session_id, rest) = without_scheme.split_once('@').ok_or_else(|| {
        AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Invalid connection code format (missing @).",
        )
    })?;

    let (host_port, path_query) = rest.split_once('/').ok_or_else(|| {
        AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Invalid connection code format (missing /).",
        )
    })?;

    let (host, port_str) = host_port.rsplit_once(':').ok_or_else(|| {
        AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Invalid connection code format (missing port).",
        )
    })?;
    let port: u16 = port_str.parse().map_err(|_| {
        AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Invalid port number in connection code.",
        )
    })?;

    let (fingerprint, query) = path_query.split_once("?t=").ok_or_else(|| {
        AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Invalid connection code format (missing ?t= token).",
        )
    })?;

    Ok((
        session_id.to_string(),
        host.to_string(),
        port,
        fingerprint.to_string(),
        query.to_string(),
    ))
}

/// Computes the cryptographic HMAC-SHA256 pairing proof binding the receiver's TLS certificate
/// fingerprint, sender nonce, and either the 256-bit token or the Argon2id-stretched short code.
pub fn compute_pairing_hmac_proof(
    mode: PairingAuthMode,
    secret_or_short_code: &str,
    pake_salt_hex: &str,
    receiver_cert_fingerprint: &str,
    sender_nonce_hex: &str,
) -> AppResult<String> {
    let key_material = match mode {
        PairingAuthMode::HighEntropyToken => URL_SAFE_NO_PAD
            .decode(secret_or_short_code.trim())
            .map_err(|_| {
                AppError::new(
                    ErrorCode::PairingAuthenticationFailed,
                    "Malformed high-entropy token.",
                )
            })?,
        PairingAuthMode::ShortCodeArgon2 => {
            // Never use a short code directly as a key: stretch with Argon2id bound to salt + cert fingerprint
            let mut derived = vec![0u8; 32];
            let composite_salt = format!("{pake_salt_hex}:{receiver_cert_fingerprint}");
            let params = Params::new(19456, 2, 1, Some(32))
                .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;
            let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
            argon2
                .hash_password_into(
                    secret_or_short_code.trim().to_ascii_uppercase().as_bytes(),
                    composite_salt.as_bytes(),
                    &mut derived,
                )
                .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;
            derived
        }
    };

    let mut mac = HmacSha256::new_from_slice(&key_material).map_err(|_| {
        AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Failed to initialize HMAC.",
        )
    })?;
    mac.update(receiver_cert_fingerprint.as_bytes());
    mac.update(b":");
    mac.update(sender_nonce_hex.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Verifies a sender's pairing request proof against the receiver's active ticket and rate limiter.
/// Returns `(Option<receiver_confirm_proof_hex>, sas_verification_code)`.
pub fn verify_pairing_request(
    ticket: &ReceiverPairingTicket,
    limiter: &mut PairingRateLimiter,
    request: &PairingRequestPayload,
) -> AppResult<(Option<String>, String)> {
    limiter.check_allowed()?;

    if request.auth_mode == PairingAuthMode::ShortCodeArgon2 {
        if let Some(ref sender_pake_hex) = request.sender_pake_pub_hex {
            let Ok(masked_sender_bytes) = hex::decode(sender_pake_hex.trim()) else {
                limiter.record_failure();
                return Err(AppError::new(
                    ErrorCode::PairingAuthenticationFailed,
                    "Malformed sender PAKE public key.",
                ));
            };
            if masked_sender_bytes.len() != 32 {
                limiter.record_failure();
                return Err(AppError::new(
                    ErrorCode::PairingAuthenticationFailed,
                    "Invalid sender PAKE public key length.",
                ));
            }
            let mut masked_sender_arr = [0u8; 32];
            masked_sender_arr.copy_from_slice(&masked_sender_bytes);

            let sender_mask = derive_argon2_password_mask(
                &ticket.short_pake_code,
                &ticket.pake_salt_hex,
                &ticket.cert_fingerprint_sha256,
                b"sender-mask",
            )?;
            let sender_pub_bytes = xor_32(&masked_sender_arr, &sender_mask);
            let sender_pub = PublicKey::from(sender_pub_bytes);

            let recv_priv_vec = hex::decode(&ticket.receiver_dh_private_hex).map_err(|_| {
                AppError::new(
                    ErrorCode::SerializationError,
                    "Invalid internal receiver DH secret.",
                )
            })?;
            let mut recv_priv_arr = [0u8; 32];
            recv_priv_arr.copy_from_slice(&recv_priv_vec);
            let recv_secret = StaticSecret::from(recv_priv_arr);

            let dh_shared = recv_secret.diffie_hellman(&sender_pub);
            let pw_auth = derive_argon2_password_mask(
                &ticket.short_pake_code,
                &ticket.pake_salt_hex,
                &ticket.cert_fingerprint_sha256,
                b"pake-auth",
            )?;

            let mut session_hasher = Sha256::new();
            session_hasher.update(dh_shared.as_bytes());
            session_hasher.update(pw_auth);
            session_hasher.update(ticket.cert_fingerprint_sha256.as_bytes());
            session_hasher.update(ticket.masked_receiver_dh_pub_hex.as_bytes());
            session_hasher.update(sender_pake_hex.trim().as_bytes());
            session_hasher.update(request.sender_nonce_hex.as_bytes());
            let pake_session_key = session_hasher.finalize();

            let mut sender_mac = HmacSha256::new_from_slice(&pake_session_key).map_err(|_| {
                AppError::new(ErrorCode::PairingAuthenticationFailed, "HMAC init failed")
            })?;
            sender_mac.update(b"sender-confirm:");
            sender_mac.update(ticket.cert_fingerprint_sha256.as_bytes());
            sender_mac.update(b":");
            sender_mac.update(request.sender_nonce_hex.as_bytes());
            let expected_sender_proof = hex::encode(sender_mac.finalize().into_bytes());

            if !constant_time_eq(
                expected_sender_proof.as_bytes(),
                request.hmac_proof_hex.as_bytes(),
            ) {
                limiter.record_failure();
                return Err(AppError::new(
                    ErrorCode::PairingAuthenticationFailed,
                    "PAKE short-code authentication failed: invalid 6-character code or certificate mismatch.",
                ));
            }

            let mut recv_mac = HmacSha256::new_from_slice(&pake_session_key).map_err(|_| {
                AppError::new(ErrorCode::PairingAuthenticationFailed, "HMAC init failed")
            })?;
            recv_mac.update(b"receiver-confirm:");
            recv_mac.update(ticket.cert_fingerprint_sha256.as_bytes());
            recv_mac.update(b":");
            recv_mac.update(request.sender_nonce_hex.as_bytes());
            let recv_confirm_hex = hex::encode(recv_mac.finalize().into_bytes());

            let sas_code = derive_sas_verification_code(
                &pake_session_key,
                &ticket.cert_fingerprint_sha256,
                &request.sender_nonce_hex,
            );
            return Ok((Some(recv_confirm_hex), sas_code));
        }
    }

    let expected_secret = match request.auth_mode {
        PairingAuthMode::HighEntropyToken => &ticket.high_entropy_token_b64,
        PairingAuthMode::ShortCodeArgon2 => &ticket.short_pake_code,
    };

    let expected_proof = compute_pairing_hmac_proof(
        request.auth_mode,
        expected_secret,
        &ticket.pake_salt_hex,
        &ticket.cert_fingerprint_sha256,
        &request.sender_nonce_hex,
    )?;

    if !constant_time_eq(expected_proof.as_bytes(), request.hmac_proof_hex.as_bytes()) {
        limiter.record_failure();
        return Err(AppError::new(
            ErrorCode::PairingAuthenticationFailed,
            "Pairing authentication failed: invalid connection code or certificate fingerprint mismatch.",
        ));
    }

    let sas_code = derive_sas_verification_code(
        expected_proof.as_bytes(),
        &ticket.cert_fingerprint_sha256,
        &request.sender_nonce_hex,
    );

    Ok((None, sas_code))
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pairing_ticket_and_rate_limiting() {
        let limits = SecurityLimits::default();
        let tls = generate_ephemeral_tls_identity().unwrap();
        let ticket =
            create_receiver_pairing_ticket("sess-1", "192.168.1.50", 53317, &tls, &limits).unwrap();
        let mut limiter = PairingRateLimiter::new(&limits);

        // 1. Valid HighEntropyToken proof succeeds
        let proof = compute_pairing_hmac_proof(
            PairingAuthMode::HighEntropyToken,
            &ticket.high_entropy_token_b64,
            &ticket.pake_salt_hex,
            &ticket.cert_fingerprint_sha256,
            "nonce123",
        )
        .unwrap();

        let valid_req = PairingRequestPayload {
            sender_device_name: "Old-Windows-PC".to_string(),
            sender_os: "windows".to_string(),
            sender_arch: "x86_64".to_string(),
            sender_nonce_hex: "nonce123".to_string(),
            auth_mode: PairingAuthMode::HighEntropyToken,
            hmac_proof_hex: proof,
            sender_pake_pub_hex: None,
            summary_threads_count: 10,
            summary_projects_count: 2,
            summary_total_bytes: 1024,
        };
        assert!(verify_pairing_request(&ticket, &mut limiter, &valid_req).is_ok());

        // 2. Valid ShortCodeArgon2 X25519 PAKE handshake succeeds and produces matching SAS & confirmation
        let (sender_pake_pub_hex, sender_proof_hex, expected_recv_confirm_hex, sender_sas) =
            create_sender_pake_handshake(
                &ticket.short_pake_code,
                &ticket.pake_salt_hex,
                &ticket.masked_receiver_dh_pub_hex,
                &ticket.cert_fingerprint_sha256,
                "nonce456",
            )
            .unwrap();
        let valid_short_req = PairingRequestPayload {
            sender_nonce_hex: "nonce456".to_string(),
            auth_mode: PairingAuthMode::ShortCodeArgon2,
            hmac_proof_hex: sender_proof_hex,
            sender_pake_pub_hex: Some(sender_pake_pub_hex),
            ..valid_req.clone()
        };
        let (recv_confirm_opt, recv_sas) =
            verify_pairing_request(&ticket, &mut limiter, &valid_short_req).unwrap();
        assert_eq!(
            recv_confirm_opt.as_deref(),
            Some(expected_recv_confirm_hex.as_str())
        );
        assert_eq!(sender_sas, recv_sas);
        assert_eq!(sender_sas.len(), 6);

        // 3. Invalid proof triggers rate limiting backoff
        let bad_req = PairingRequestPayload {
            hmac_proof_hex: "00000000".to_string(),
            ..valid_req
        };
        assert!(verify_pairing_request(&ticket, &mut limiter, &bad_req).is_err());
        assert_eq!(limiter.failed_attempts, 1);
        assert!(limiter.check_allowed().is_err());
    }
}
