use crate::config::SecurityLimits;
use crate::errors::{AppError, AppResult, ErrorCode};
use crate::inventory::{compute_file_sha256, MigrationManifest};
use crate::paths::{resolve_safe_child_path, sanitize_relative_path};
use argon2::{Argon2, Params};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const ENCRYPTED_BUNDLE_MAGIC: &[u8; 8] = b"CDXMOVE1";

/// Creates a verified, consistent staging snapshot of the selected source files:
/// - Re-checks file sizes and modification timestamps while copying to detect live modifications.
/// - Uses SQLite Online Backup API (`rusqlite::backup::Backup`) for `.sqlite` files so WAL/page states are captured cleanly.
/// - Computes exact SHA-256 digests and updates `manifest.files` in place.
pub fn create_staging_snapshot(
    manifest: &mut MigrationManifest,
    staging_dir: &Path,
    limits: &SecurityLimits,
) -> AppResult<String> {
    if staging_dir.exists() {
        std::fs::remove_dir_all(staging_dir)?;
    }
    std::fs::create_dir_all(staging_dir)?;

    for entry in manifest.files.iter_mut() {
        let clean_rel = sanitize_relative_path(&entry.archive_rel_path, limits.max_path_length)?;
        let dst_path = resolve_safe_child_path(staging_dir, &clean_rel, limits.max_path_length)?;
        if let Some(parent) = dst_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if entry.symlink.is_some() {
            // Symlinks are recorded in manifest metadata; never follow external links into staging
            continue;
        }

        let src_path = PathBuf::from(&entry.source_abs_path);
        if !src_path.is_file() {
            return Err(AppError::new(
                ErrorCode::MigrationAbortedBySourceChange,
                format!(
                    "Source file disappeared during snapshot creation: {}",
                    src_path.display()
                ),
            ));
        }

        if src_path
            .extension()
            .map(|e| e.to_string_lossy() == "sqlite")
            .unwrap_or(false)
        {
            snapshot_sqlite_database(&src_path, &dst_path)?;
        } else {
            stream_copy_with_size_guard(&src_path, &dst_path, entry.size_bytes, limits)?;
        }

        let final_meta = std::fs::metadata(&dst_path)?;
        entry.size_bytes = final_meta.len();
        entry.sha256 = compute_file_sha256(&dst_path)?;
    }

    let manifest_json = serde_json::to_vec_pretty(manifest)?;
    if manifest_json.len() as u64 > limits.max_manifest_bytes {
        return Err(AppError::new(
            ErrorCode::DecompressionLimitExceeded,
            "Generated manifest exceeds maximum allowed size.",
        ));
    }

    let manifest_path = staging_dir.join("manifest.json");
    std::fs::write(&manifest_path, &manifest_json)?;

    let mut hasher = Sha256::new();
    hasher.update(&manifest_json);
    Ok(hex::encode(hasher.finalize()))
}

fn snapshot_sqlite_database(src_db: &Path, dst_db: &Path) -> AppResult<()> {
    let src_conn =
        rusqlite::Connection::open_with_flags(src_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut dst_conn = rusqlite::Connection::open(dst_db)?;
    let backup = rusqlite::backup::Backup::new(&src_conn, &mut dst_conn)?;
    backup.run_to_completion(200, std::time::Duration::from_millis(5), None)?;
    Ok(())
}

fn stream_copy_with_size_guard(
    src: &Path,
    dst: &Path,
    expected_bytes: u64,
    limits: &SecurityLimits,
) -> AppResult<()> {
    let meta_before = std::fs::metadata(src)?;
    let mtime_before = meta_before.modified().ok();

    let mut reader =
        BufReader::with_capacity(limits.stream_chunk_bytes.min(256 * 1024), File::open(src)?);
    let mut writer = BufWriter::with_capacity(
        limits.stream_chunk_bytes.min(256 * 1024),
        File::create(dst)?,
    );
    let mut buf = vec![0u8; 64 * 1024];
    let mut total_copied = 0u64;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total_copied += n as u64;
        writer.write_all(&buf[..n])?;
    }
    writer.flush()?;

    let meta_after = std::fs::metadata(src)?;
    let mtime_after = meta_after.modified().ok();
    if total_copied != expected_bytes || mtime_before != mtime_after {
        return Err(AppError::new(
            ErrorCode::MigrationAbortedBySourceChange,
            format!(
                "Source file '{}' changed while snapshot was being taken. Please close Codex and retry.",
                src.display()
            ),
        ));
    }

    Ok(())
}

/// Validates and extracts a staging directory against its `MigrationManifest`,
/// checking every file's byte count and SHA-256 hash.
pub fn verify_staging_snapshot(
    staging_dir: &Path,
    limits: &SecurityLimits,
) -> AppResult<(MigrationManifest, String)> {
    let manifest_path = staging_dir.join("manifest.json");
    let meta = std::fs::metadata(&manifest_path)?;
    if meta.len() > limits.max_manifest_bytes {
        return Err(AppError::new(
            ErrorCode::DecompressionLimitExceeded,
            "Manifest file exceeds security limit.",
        ));
    }

    let raw_bytes = std::fs::read(&manifest_path)?;
    let mut hasher = Sha256::new();
    hasher.update(&raw_bytes);
    let manifest_hash = hex::encode(hasher.finalize());

    let manifest: MigrationManifest = serde_json::from_slice(&raw_bytes)?;
    if manifest.files.len() > limits.max_file_count {
        return Err(AppError::new(
            ErrorCode::DecompressionLimitExceeded,
            "Manifest file count exceeds security limit.",
        ));
    }

    for entry in &manifest.files {
        let dst_path =
            resolve_safe_child_path(staging_dir, &entry.archive_rel_path, limits.max_path_length)?;
        if entry.symlink.is_some() {
            continue;
        }
        if !dst_path.is_file() {
            return Err(AppError::new(
                ErrorCode::TransferInterrupted,
                format!("Missing file in staging area: {}", entry.archive_rel_path),
            ));
        }
        let fmeta = std::fs::metadata(&dst_path)?;
        if fmeta.len() != entry.size_bytes {
            return Err(AppError::new(
                ErrorCode::TransferInterrupted,
                format!(
                    "Size mismatch for '{}': expected {}, got {}",
                    entry.archive_rel_path,
                    entry.size_bytes,
                    fmeta.len()
                ),
            ));
        }
        let actual_sha = compute_file_sha256(&dst_path)?;
        if !entry.sha256.is_empty() && !actual_sha.eq_ignore_ascii_case(&entry.sha256) {
            return Err(AppError::new(
                ErrorCode::TransferInterrupted,
                format!(
                    "SHA-256 hash mismatch for '{}': expected {}, got {}",
                    entry.archive_rel_path, entry.sha256, actual_sha
                ),
            ));
        }
    }

    Ok((manifest, manifest_hash))
}

/// Advanced fallback feature: Exports a staging snapshot as an encrypted `.cdxmove` bundle
/// using Argon2id key derivation + ChaCha20-Poly1305 authenticated encryption over Zstd-compressed tar.
pub fn export_encrypted_bundle(
    staging_dir: &Path,
    output_file: &Path,
    passphrase: &str,
) -> AppResult<()> {
    let mut tar_zst_bytes = Vec::new();
    {
        let encoder = zstd::Encoder::new(&mut tar_zst_bytes, 3)?;
        let mut tar_builder = tar::Builder::new(encoder);
        tar_builder.follow_symlinks(false);
        tar_builder.append_dir_all(".", staging_dir)?;
        let encoder = tar_builder.into_inner()?;
        encoder.finish()?;
    }

    let mut salt = [0u8; 16];
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    let mut derived_key = [0u8; 32];
    let params = Params::new(19456, 2, 1, Some(32))
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    argon2
        .hash_password_into(passphrase.as_bytes(), &salt, &mut derived_key)
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&derived_key));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), tar_zst_bytes.as_ref())
        .map_err(|_| AppError::new(ErrorCode::SerializationError, "Bundle encryption failed."))?;

    let mut out = File::create(output_file)?;
    out.write_all(ENCRYPTED_BUNDLE_MAGIC)?;
    out.write_all(&salt)?;
    out.write_all(&nonce_bytes)?;
    out.write_all(&ciphertext)?;
    out.flush()?;
    Ok(())
}

/// Decrypts and safely unpacks an encrypted `.cdxmove` bundle into `target_staging_dir`,
/// enforcing decompression limits and blocking path traversal or symlink escapes.
pub fn import_encrypted_bundle(
    bundle_file: &Path,
    target_staging_dir: &Path,
    passphrase: &str,
    limits: &SecurityLimits,
    max_total_unpacked_bytes: u64,
) -> AppResult<(MigrationManifest, String)> {
    let raw = std::fs::read(bundle_file)?;
    if raw.len() < 8 + 16 + 12 + 16 || &raw[0..8] != ENCRYPTED_BUNDLE_MAGIC {
        return Err(AppError::new(
            ErrorCode::SerializationError,
            "Invalid or corrupted Codex Move bundle header.",
        ));
    }

    let salt = &raw[8..24];
    let nonce_bytes = &raw[24..36];
    let ciphertext = &raw[36..];

    let mut derived_key = [0u8; 32];
    let params = Params::new(19456, 2, 1, Some(32))
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut derived_key)
        .map_err(|e| AppError::new(ErrorCode::SerializationError, e.to_string()))?;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&derived_key));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| {
            AppError::new(
                ErrorCode::PairingAuthenticationFailed,
                "Invalid bundle passphrase or tampered archive.",
            )
        })?;

    if target_staging_dir.exists() {
        std::fs::remove_dir_all(target_staging_dir)?;
    }
    std::fs::create_dir_all(target_staging_dir)?;

    let decoder = zstd::Decoder::new(plaintext.as_slice())?;
    let bounded_reader = decoder.take(max_total_unpacked_bytes + 1);
    let mut archive = tar::Archive::new(bounded_reader);

    let mut unpacked_bytes = 0u64;
    let mut unpacked_files = 0usize;

    for entry_res in archive.entries()? {
        let mut entry = entry_res?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(AppError::new(
                ErrorCode::ExternalSymlinkBlocked,
                "Raw symbolic/hard links inside untrusted tar streams are rejected.",
            ));
        }
        if !entry_type.is_file() {
            continue;
        }

        let path_str = entry.path()?.to_string_lossy().to_string();
        if path_str == "." || path_str == "./" {
            continue;
        }
        let stripped = path_str.strip_prefix("./").unwrap_or(&path_str);
        let safe_dst =
            resolve_safe_child_path(target_staging_dir, stripped, limits.max_path_length)?;
        if let Some(parent) = safe_dst.parent() {
            std::fs::create_dir_all(parent)?;
        }

        unpacked_files += 1;
        if unpacked_files > limits.max_file_count {
            return Err(AppError::new(
                ErrorCode::DecompressionLimitExceeded,
                "Archive file count exceeds security limit.",
            ));
        }

        let mut out_file = File::create(&safe_dst)?;
        let n = std::io::copy(&mut entry, &mut out_file)?;
        unpacked_bytes += n;
        if unpacked_bytes > max_total_unpacked_bytes {
            return Err(AppError::new(
                ErrorCode::DecompressionLimitExceeded,
                "Decompression bomb detected: unpacked size exceeds limit.",
            ));
        }
    }

    verify_staging_snapshot(target_staging_dir, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_encrypted_bundle_roundtrip_and_wrong_password_rejection() {
        let dir = tempdir().unwrap();
        let src_codex = dir.path().join("src_codex");
        let staging = dir.path().join("staging");
        let unpacked = dir.path().join("unpacked");
        let bundle = dir.path().join("backup.cdxmove");

        std::fs::create_dir_all(&src_codex).unwrap();
        std::fs::write(src_codex.join("AGENTS.md"), "# Test Agents").unwrap();

        let opts = crate::inventory::ScanOptions {
            codex_home: src_codex.to_string_lossy().to_string(),
            shared_skills_dir: None,
            include_project_files: false,
            excluded_project_dirs: vec![],
            compute_sha256: true,
        };
        let limits = SecurityLimits::default();
        let mut manifest = crate::inventory::scan_codex_inventory(&opts, &limits).unwrap();

        let hash1 = create_staging_snapshot(&mut manifest, &staging, &limits).unwrap();
        export_encrypted_bundle(&staging, &bundle, "correct-horse-battery-staple").unwrap();

        // Wrong password must fail authentication
        assert!(
            import_encrypted_bundle(&bundle, &unpacked, "wrong-password", &limits, 10_000_000)
                .is_err()
        );

        // Correct password succeeds and verifies hashes
        let (restored, hash2) = import_encrypted_bundle(
            &bundle,
            &unpacked,
            "correct-horse-battery-staple",
            &limits,
            10_000_000,
        )
        .unwrap();
        assert_eq!(hash1, hash2);
        assert_eq!(restored.files.len(), 1);
    }
}
