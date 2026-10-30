use crate::errors::{AppError, AppResult, ErrorCode};
use crate::inventory::compute_file_sha256;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalStatus {
    BackupCreated,
    ApplyingChanges,
    Completed,
    RolledBack,
    RollbackPartialFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JournalOperation {
    /// Target file existed before migration and was backed up to `backup_rel_path`
    ModifiedExistingFile {
        target_abs_path: String,
        backup_rel_path: String,
        original_sha256: String,
    },
    /// Target file did not exist before migration and was newly created
    CreatedNewFile { target_abs_path: String },
    /// Target directory did not exist before migration and was newly created
    CreatedNewDirectory { target_abs_path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryJournal {
    pub migration_id: String,
    pub started_at: String,
    pub updated_at: String,
    pub target_codex_home: String,
    pub backup_dir: String,
    pub staging_dir: Option<String>,
    pub status: JournalStatus,
    pub current_checkpoint: String,
    pub operations: Vec<JournalOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackReport {
    pub migration_id: String,
    pub fully_restored: bool,
    pub restored_files_count: usize,
    pub removed_new_files_count: usize,
    pub failed_paths: Vec<String>,
    pub retained_backup_dir: String,
}

/// Returns the `.codex-move-recovery` directory placed alongside `target_codex_home`.
pub fn recovery_root_for_codex_home(target_codex_home: &Path) -> PathBuf {
    let parent = target_codex_home.parent().unwrap_or_else(|| Path::new("."));
    parent.join(".codex-move-recovery")
}

/// Initializes a new persistent recovery journal and backup directory before any target file is modified.
pub fn begin_recovery_journal(
    migration_id: &str,
    target_codex_home: &Path,
    staging_dir: Option<&Path>,
) -> AppResult<RecoveryJournal> {
    let rec_root = recovery_root_for_codex_home(target_codex_home);
    let mig_rec_dir = rec_root.join(migration_id);
    let backup_dir = mig_rec_dir.join("backup");
    std::fs::create_dir_all(&backup_dir)?;

    let journal = RecoveryJournal {
        migration_id: migration_id.to_string(),
        started_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
        target_codex_home: target_codex_home.to_string_lossy().to_string(),
        backup_dir: backup_dir.to_string_lossy().to_string(),
        staging_dir: staging_dir.map(|p| p.to_string_lossy().to_string()),
        status: JournalStatus::BackupCreated,
        current_checkpoint: "Initialized".to_string(),
        operations: Vec::new(),
    };

    persist_journal(&journal)?;
    Ok(journal)
}

pub fn persist_journal(journal: &RecoveryJournal) -> AppResult<()> {
    let target_home = PathBuf::from(&journal.target_codex_home);
    let rec_root = recovery_root_for_codex_home(&target_home);
    let mig_rec_dir = rec_root.join(&journal.migration_id);
    std::fs::create_dir_all(&mig_rec_dir)?;

    let journal_path = mig_rec_dir.join("journal.json");
    let tmp_path = mig_rec_dir.join("journal.json.tmp");
    let bytes = serde_json::to_vec_pretty(journal)?;
    {
        let mut f = File::create(&tmp_path)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, &journal_path)?;
    Ok(())
}

/// Records and backs up a target file right before it is modified or created,
/// verifying the backup copy's readability and SHA-256 hash before allowing any write.
pub fn backup_and_record_target_file(
    journal: &mut RecoveryJournal,
    target_file: &Path,
    logical_backup_rel: &str,
) -> AppResult<()> {
    let target_abs = target_file.to_string_lossy().to_string();

    // Avoid duplicate backup entries for the same file within the same transaction
    let already_recorded = journal.operations.iter().any(|op| match op {
        JournalOperation::ModifiedExistingFile {
            target_abs_path, ..
        }
        | JournalOperation::CreatedNewFile { target_abs_path } => target_abs_path == &target_abs,
        _ => false,
    });
    if already_recorded {
        return Ok(());
    }

    if target_file.exists() {
        let orig_sha = compute_file_sha256(target_file)?;
        let backup_root = PathBuf::from(&journal.backup_dir);
        let clean_rel = logical_backup_rel.replace('\\', "/");
        let backup_dst = backup_root.join(&clean_rel);
        if let Some(parent) = backup_dst.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::copy(target_file, &backup_dst)?;

        // Verify backup readability and exact SHA-256 match BEFORE permitting target modification
        let backup_sha = compute_file_sha256(&backup_dst)?;
        if backup_sha != orig_sha {
            return Err(AppError::new(
                ErrorCode::BackupVerificationFailed,
                format!(
                    "Backup verification failed for '{}': hash mismatch.",
                    target_file.display()
                ),
            ));
        }

        journal
            .operations
            .push(JournalOperation::ModifiedExistingFile {
                target_abs_path: target_abs,
                backup_rel_path: clean_rel,
                original_sha256: orig_sha,
            });
    } else {
        journal.operations.push(JournalOperation::CreatedNewFile {
            target_abs_path: target_abs,
        });
    }

    journal.updated_at = Utc::now().to_rfc3339();
    persist_journal(journal)?;
    Ok(())
}

/// Discovers any incomplete or rollback-eligible migrations on startup or on demand.
pub fn discover_recovery_journals(target_codex_home: &Path) -> Vec<RecoveryJournal> {
    let rec_root = recovery_root_for_codex_home(target_codex_home);
    let Ok(entries) = std::fs::read_dir(&rec_root) else {
        return Vec::new();
    };

    let mut journals = Vec::new();
    for entry in entries.flatten() {
        let j_path = entry.path().join("journal.json");
        if j_path.is_file() {
            if let Ok(raw) = std::fs::read_to_string(&j_path) {
                if let Ok(j) = serde_json::from_str::<RecoveryJournal>(&raw) {
                    journals.push(j);
                }
            }
        }
    }
    journals.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    journals
}

/// Executes compensating rollback in reverse chronological order:
/// - Restores modified files from verified backup copies and re-verifies SHA-256.
/// - Deletes newly created files and empty newly created directories.
/// - Cleans up temporary staging files while retaining backup material if any path failed.
pub fn execute_rollback(journal: &mut RecoveryJournal) -> AppResult<RollbackReport> {
    let backup_root = PathBuf::from(&journal.backup_dir);
    let mut restored_files_count = 0usize;
    let mut removed_new_files_count = 0usize;
    let mut failed_paths = Vec::new();

    for op in journal.operations.iter().rev() {
        match op {
            JournalOperation::ModifiedExistingFile {
                target_abs_path,
                backup_rel_path,
                original_sha256,
            } => {
                let target_p = PathBuf::from(target_abs_path);
                let backup_p = backup_root.join(backup_rel_path);
                if !backup_p.is_file() {
                    failed_paths.push(format!(
                        "{target_abs_path} (missing backup file {})",
                        backup_p.display()
                    ));
                    continue;
                }
                if let Some(parent) = target_p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                // Remove any SQLite WAL/SHM created during interrupted apply
                if target_abs_path.ends_with(".sqlite") {
                    let _ = std::fs::remove_file(format!("{target_abs_path}-wal"));
                    let _ = std::fs::remove_file(format!("{target_abs_path}-shm"));
                }

                match std::fs::copy(&backup_p, &target_p) {
                    Ok(_) => match compute_file_sha256(&target_p) {
                        Ok(restored_sha) if &restored_sha == original_sha256 => {
                            restored_files_count += 1;
                        }
                        _ => {
                            failed_paths.push(format!(
                                "{target_abs_path} (post-rollback SHA-256 mismatch)"
                            ));
                        }
                    },
                    Err(e) => {
                        failed_paths.push(format!("{target_abs_path} ({e})"));
                    }
                }
            }
            JournalOperation::CreatedNewFile { target_abs_path } => {
                let target_p = PathBuf::from(target_abs_path);
                if target_p.exists() || target_p.is_symlink() {
                    if target_abs_path.ends_with(".sqlite") {
                        let _ = std::fs::remove_file(format!("{target_abs_path}-wal"));
                        let _ = std::fs::remove_file(format!("{target_abs_path}-shm"));
                    }
                    if let Err(e) = std::fs::remove_file(&target_p) {
                        failed_paths.push(format!("{target_abs_path} ({e})"));
                    } else {
                        removed_new_files_count += 1;
                    }
                }
            }
            JournalOperation::CreatedNewDirectory { target_abs_path } => {
                let target_p = PathBuf::from(target_abs_path);
                if target_p.is_dir() {
                    let _ = std::fs::remove_dir(&target_p);
                }
            }
        }
    }

    let fully_restored = failed_paths.is_empty();
    journal.status = if fully_restored {
        JournalStatus::RolledBack
    } else {
        JournalStatus::RollbackPartialFailure
    };
    journal.updated_at = Utc::now().to_rfc3339();
    persist_journal(journal)?;

    // Clean up staging directory if present
    if let Some(ref stg) = journal.staging_dir {
        let stg_path = PathBuf::from(stg);
        if stg_path.is_dir() {
            let _ = std::fs::remove_dir_all(stg_path);
        }
    }

    Ok(RollbackReport {
        migration_id: journal.migration_id.clone(),
        fully_restored,
        restored_files_count,
        removed_new_files_count,
        failed_paths,
        retained_backup_dir: journal.backup_dir.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_crash_recovery_and_exact_byte_rollback() {
        let dir = tempdir().unwrap();
        let target_codex = dir.path().join(".codex");
        std::fs::create_dir_all(&target_codex).unwrap();

        let cfg_file = target_codex.join("config.toml");
        let original_bytes = b"model = \"gpt-5-original\"\n";
        std::fs::write(&cfg_file, original_bytes).unwrap();
        let orig_sha = compute_file_sha256(&cfg_file).unwrap();

        let new_session_file = target_codex.join("sessions").join("new_rollout.jsonl");

        let mut journal = begin_recovery_journal("mig-crash-01", &target_codex, None).unwrap();
        journal.status = JournalStatus::ApplyingChanges;

        // 1. Backup existing config.toml and overwrite it
        backup_and_record_target_file(&mut journal, &cfg_file, "codex_home/config.toml").unwrap();
        std::fs::write(&cfg_file, b"model = \"corrupted-half-written\"").unwrap();

        // 2. Record and create a new session file
        backup_and_record_target_file(
            &mut journal,
            &new_session_file,
            "codex_home/sessions/new_rollout.jsonl",
        )
        .unwrap();
        std::fs::create_dir_all(new_session_file.parent().unwrap()).unwrap();
        std::fs::write(&new_session_file, b"{\"partial\":true}").unwrap();

        // Simulate process crash and restart discovery
        let discovered = discover_recovery_journals(&target_codex);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].status, JournalStatus::ApplyingChanges);

        // Execute rollback
        let mut crashed_journal = discovered.into_iter().next().unwrap();
        let report = execute_rollback(&mut crashed_journal).unwrap();
        assert!(report.fully_restored);
        assert_eq!(report.restored_files_count, 1);
        assert_eq!(report.removed_new_files_count, 1);

        // Verify original config.toml is restored byte-for-byte and new file is removed
        assert_eq!(compute_file_sha256(&cfg_file).unwrap(), orig_sha);
        assert!(!new_session_file.exists());
    }
}
