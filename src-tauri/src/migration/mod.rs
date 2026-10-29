use crate::adapters::codex::{
    merge_and_transform_config_toml, merge_and_transform_global_state, query_table_columns,
    transform_automation_toml, transform_rollout_jsonl_file,
};
use crate::config::SecurityLimits;
use crate::errors::{AppError, AppResult, ErrorCode};
use crate::inventory::{ContentCategory, MigrationManifest};
use crate::paths::{
    resolve_safe_child_path, rewrite_path_with_rules, PathMappingRule, PathStyle,
    SymlinkClassification,
};
use crate::planner::{ImportPlan, PlannedThreadAction, ThreadConflictResolution};
use crate::platform::detect_active_codex_processes;
use crate::recovery::{
    backup_and_record_target_file, begin_recovery_journal, execute_rollback, persist_journal,
    JournalOperation, JournalStatus, RecoveryJournal, RollbackReport,
};
use crate::verification::{verify_imported_codex_environment, PostImportVerificationReport};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationPhase {
    Scan,
    Inspect,
    CreateSnapshot,
    TransferToStaging,
    VerifyStaging,
    BuildImportPlan,
    BackupTarget,
    ApplyChanges,
    VerifyResult,
    Completed,
    FailedAndRolledBack,
    FailedPartialRollback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationProgressUpdate {
    pub migration_id: String,
    pub phase: MigrationPhase,
    pub processed_files: usize,
    pub total_files: usize,
    pub processed_bytes: u64,
    pub total_bytes: u64,
    pub current_item: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationOutcomeStatus {
    Succeeded,
    PartiallyCompletedWithWarnings,
    FailedAndRolledBack,
    FailedPartialRollback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationExecutionResult {
    pub migration_id: String,
    pub outcome: MigrationOutcomeStatus,
    pub applied_files_count: usize,
    pub merged_threads_count: usize,
    pub deduplicated_threads_count: usize,
    pub forked_threads_count: usize,
    pub warnings: Vec<String>,
    pub verification: Option<PostImportVerificationReport>,
    pub rollback_report: Option<RollbackReport>,
    pub backup_dir: String,
}

#[derive(Debug, Clone, Default)]
pub struct FailureInjectionHook {
    /// If set, deliberately triggers an error after applying `fail_after_files` files during `ApplyChanges`
    /// to test crash/error rollback in integration tests.
    pub fail_after_files: Option<usize>,
}

/// Executes the target-side import from a verified `staging_dir` according to `plan`.
/// Uses a persistent `RecoveryJournal` and automatically rolls back if any unrecoverable error occurs.
pub fn apply_migration_from_staging<F>(
    manifest: &MigrationManifest,
    staging_dir: &Path,
    plan: &ImportPlan,
    limits: &SecurityLimits,
    failure_hook: &FailureInjectionHook,
    mut on_progress: F,
) -> AppResult<MigrationExecutionResult>
where
    F: FnMut(MigrationProgressUpdate),
{
    if !plan.can_proceed {
        return Err(AppError::new(
            ErrorCode::PathConflictDetected,
            format!(
                "Cannot execute import plan due to blocking issues: {}",
                plan.blocking_reasons.join("; ")
            ),
        ));
    }

    let target_codex_home = PathBuf::from(&plan.target_codex_home);

    // Ensure no Codex process is actively writing to target_codex_home
    let active_procs = detect_active_codex_processes(Some(&target_codex_home));
    if !active_procs.is_empty() && std::env::var("CODEX_MOVE_ALLOW_RUNNING_CODEX").is_err() {
        return Err(AppError::new(
            ErrorCode::CodexProcessRunning,
            format!(
                "Codex is currently running ({}). Please close Codex before applying migration changes.",
                active_procs
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    let mut journal = begin_recovery_journal(
        &manifest.migration_id,
        &target_codex_home,
        Some(staging_dir),
    )?;

    let selected_set: HashSet<ContentCategory> = plan.selected_categories.iter().copied().collect();

    let eligible_files: Vec<_> = manifest
        .files
        .iter()
        .filter(|f| selected_set.contains(&f.category))
        .filter(|f| plan.include_sensitive_files || !f.is_sensitive)
        .collect();

    let total_files = eligible_files.len();
    let total_bytes: u64 = eligible_files.iter().map(|f| f.size_bytes).sum();

    on_progress(MigrationProgressUpdate {
        migration_id: manifest.migration_id.clone(),
        phase: MigrationPhase::BackupTarget,
        processed_files: 0,
        total_files,
        processed_bytes: 0,
        total_bytes,
        current_item: "Initializing backup and transaction journal".to_string(),
    });

    journal.status = JournalStatus::ApplyingChanges;
    journal.current_checkpoint = "ApplyingFiles".to_string();
    persist_journal(&journal)?;

    let apply_res = execute_apply_steps(
        manifest,
        staging_dir,
        plan,
        &target_codex_home,
        &eligible_files,
        limits,
        failure_hook,
        &mut journal,
        total_files,
        total_bytes,
        &mut on_progress,
    );

    match apply_res {
        Ok((applied_files_count, mut warnings)) => {
            journal.current_checkpoint = "VerifyingResult".to_string();
            persist_journal(&journal)?;

            on_progress(MigrationProgressUpdate {
                migration_id: manifest.migration_id.clone(),
                phase: MigrationPhase::VerifyResult,
                processed_files: total_files,
                total_files,
                processed_bytes: total_bytes,
                total_bytes,
                current_item: "Verifying imported databases and paths".to_string(),
            });

            let verification = verify_imported_codex_environment(manifest, plan)?;
            if !verification.all_automated_checks_passed {
                warnings.push(
                    "One or more automated post-import checks reported warnings. Review details before starting Codex."
                        .to_string(),
                );
            }

            journal.status = JournalStatus::Completed;
            journal.current_checkpoint = "Completed".to_string();
            persist_journal(&journal)?;

            // Clean up staging directory after successful completion
            if staging_dir.exists() {
                let _ = std::fs::remove_dir_all(staging_dir);
            }

            on_progress(MigrationProgressUpdate {
                migration_id: manifest.migration_id.clone(),
                phase: MigrationPhase::Completed,
                processed_files: total_files,
                total_files,
                processed_bytes: total_bytes,
                total_bytes,
                current_item: "Migration complete".to_string(),
            });

            let merged_threads_count = plan
                .thread_actions
                .iter()
                .filter(|a| a.resolution == ThreadConflictResolution::ImportNew)
                .count();
            let deduplicated_threads_count = plan
                .thread_actions
                .iter()
                .filter(|a| a.resolution == ThreadConflictResolution::DeduplicateIdentical)
                .count();
            let forked_threads_count = plan
                .thread_actions
                .iter()
                .filter(|a| a.resolution == ThreadConflictResolution::KeepBothForked)
                .count();

            let outcome = if warnings.is_empty() && verification.all_automated_checks_passed {
                MigrationOutcomeStatus::Succeeded
            } else {
                MigrationOutcomeStatus::PartiallyCompletedWithWarnings
            };

            Ok(MigrationExecutionResult {
                migration_id: manifest.migration_id.clone(),
                outcome,
                applied_files_count,
                merged_threads_count,
                deduplicated_threads_count,
                forked_threads_count,
                warnings,
                verification: Some(verification),
                rollback_report: None,
                backup_dir: journal.backup_dir,
            })
        }
        Err(err) => {
            crate::logging::record_diagnostic(
                "ERROR",
                "migration",
                &format!("Apply phase failed ({err}); initiating automatic rollback."),
            );

            let rb_report = execute_rollback(&mut journal)?;
            let outcome = if rb_report.fully_restored {
                MigrationOutcomeStatus::FailedAndRolledBack
            } else {
                MigrationOutcomeStatus::FailedPartialRollback
            };

            Ok(MigrationExecutionResult {
                migration_id: manifest.migration_id.clone(),
                outcome,
                applied_files_count: 0,
                merged_threads_count: 0,
                deduplicated_threads_count: 0,
                forked_threads_count: 0,
                warnings: vec![err.to_payload().message],
                verification: None,
                rollback_report: Some(rb_report),
                backup_dir: journal.backup_dir,
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_apply_steps<F>(
    manifest: &MigrationManifest,
    staging_dir: &Path,
    plan: &ImportPlan,
    target_codex_home: &Path,
    eligible_files: &[&crate::inventory::ManifestFileEntry],
    limits: &SecurityLimits,
    failure_hook: &FailureInjectionHook,
    journal: &mut RecoveryJournal,
    total_files: usize,
    total_bytes: u64,
    on_progress: &mut F,
) -> AppResult<(usize, Vec<String>)>
where
    F: FnMut(MigrationProgressUpdate),
{
    let target_style = PathStyle::current();
    let mut warnings = Vec::new();
    let mut processed_files = 0usize;
    let mut processed_bytes = 0u64;

    if !target_codex_home.exists() {
        std::fs::create_dir_all(target_codex_home)?;
        journal
            .operations
            .push(JournalOperation::CreatedNewDirectory {
                target_abs_path: target_codex_home.to_string_lossy().to_string(),
            });
    }

    let project_target_lookup: HashMap<String, (String, bool)> = plan
        .project_mappings
        .iter()
        .map(|pm| {
            (
                pm.project_id.clone(),
                (pm.target_root.clone(), pm.copy_project_files),
            )
        })
        .collect();

    // Map from source rollout normalized path -> planned thread action (for deduplication skip & fork renaming)
    let mut rollout_action_lookup: HashMap<String, &PlannedThreadAction> = HashMap::new();
    let mut forked_thread_id_map: HashMap<String, String> = HashMap::new();

    for action in &plan.thread_actions {
        if action.resolution == ThreadConflictResolution::KeepBothForked {
            forked_thread_id_map.insert(
                action.source_thread_id.clone(),
                action.target_thread_id.clone(),
            );
        }
        if let Some(src_t) = manifest
            .threads
            .iter()
            .find(|t| t.id == action.source_thread_id)
        {
            if !src_t.rollout_path.trim().is_empty() {
                let norm = src_t.rollout_path.replace('\\', "/");
                rollout_action_lookup.insert(norm, action);
            }
        }
    }

    for entry in eligible_files {
        if let Some(fail_at) = failure_hook.fail_after_files {
            if processed_files >= fail_at {
                return Err(AppError::new(
                    ErrorCode::IoError,
                    "Injected failure during ApplyChanges phase.",
                ));
            }
        }

        // Determine destination path and optional forked thread ID on target machine
        let dst_info_opt = resolve_destination_for_entry(
            entry,
            target_codex_home,
            plan.target_shared_skills_dir.as_deref().map(Path::new),
            &project_target_lookup,
            &rollout_action_lookup,
            limits,
        )?;

        let Some((dst_path, forked_thread_id_opt)) = dst_info_opt else {
            processed_files += 1;
            processed_bytes += entry.size_bytes;
            continue;
        };

        // SQLite files in ChatMetadataAndIndex/Extensions are merged structurally after file loop
        if entry.logical_rel_path.ends_with(".sqlite")
            && matches!(
                entry.category,
                ContentCategory::ChatMetadataAndIndex
                    | ContentCategory::ExtensionsMemoriesRulesHooks
            )
        {
            processed_files += 1;
            processed_bytes += entry.size_bytes;
            continue;
        }

        on_progress(MigrationProgressUpdate {
            migration_id: manifest.migration_id.clone(),
            phase: MigrationPhase::ApplyChanges,
            processed_files,
            total_files,
            processed_bytes,
            total_bytes,
            current_item: entry.logical_rel_path.clone(),
        });

        backup_and_record_target_file(journal, &dst_path, &entry.archive_rel_path)?;

        if let Some(parent) = dst_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Handle symbolic links safely
        if let Some(ref sym_class) = entry.symlink {
            match sym_class {
                SymlinkClassification::InternalRelative { target_rel } => {
                    let clean_target = Path::new(target_rel);
                    if dst_path.exists() || dst_path.is_symlink() {
                        let _ = std::fs::remove_file(&dst_path);
                    }
                    if let Err(e) =
                        crate::platform::create_relative_file_symlink(clean_target, &dst_path)
                    {
                        warnings.push(format!(
                            "Could not create symbolic link '{}' on target OS ({e}); skipped link.",
                            entry.logical_rel_path
                        ));
                    }
                }
                SymlinkClassification::ExternalTarget { raw_target } => {
                    warnings.push(format!(
                        "Skipped external symbolic link '{}' -> '{}' for security.",
                        entry.logical_rel_path, raw_target
                    ));
                }
                SymlinkClassification::Broken { raw_target } => {
                    warnings.push(format!(
                        "Skipped broken symbolic link '{}' -> '{}'.",
                        entry.logical_rel_path, raw_target
                    ));
                }
            }
            processed_files += 1;
            continue;
        }

        let staged_file =
            resolve_safe_child_path(staging_dir, &entry.archive_rel_path, limits.max_path_length)?;

        // Category-specific transformations
        if entry.category == ContentCategory::ActiveAndArchivedChats
            && entry.logical_rel_path.ends_with(".jsonl")
        {
            transform_rollout_jsonl_file(
                &staged_file,
                &dst_path,
                &plan.path_mapping_rules,
                target_style,
                forked_thread_id_opt,
            )?;
        } else if entry.logical_rel_path == "config.toml" {
            let src_toml = std::fs::read_to_string(&staged_file)?;
            let existing_toml = if dst_path.is_file() {
                std::fs::read_to_string(&dst_path).ok()
            } else {
                None
            };
            let (merged_toml, cfg_warns) = merge_and_transform_config_toml(
                &src_toml,
                existing_toml.as_deref(),
                &plan.path_mapping_rules,
                target_style,
                !plan.include_sensitive_files,
            )?;
            warnings.extend(cfg_warns);
            write_file_atomically(&dst_path, merged_toml.as_bytes())?;
        } else if entry.logical_rel_path == ".codex-global-state.json" {
            let src_gs_raw = std::fs::read_to_string(&staged_file)?;
            let src_gs: serde_json::Value = serde_json::from_str(&src_gs_raw)?;
            let existing_gs = if dst_path.is_file() {
                std::fs::read_to_string(&dst_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            } else {
                None
            };
            let merged_gs = merge_and_transform_global_state(
                &src_gs,
                existing_gs.as_ref(),
                &plan.path_mapping_rules,
                target_style,
                &forked_thread_id_map,
            );
            let out_bytes = serde_json::to_vec_pretty(&merged_gs)?;
            write_file_atomically(&dst_path, &out_bytes)?;
        } else if entry.logical_rel_path == "session_index.jsonl" {
            merge_session_index_jsonl(&staged_file, &dst_path, &plan.thread_actions)?;
        } else if entry.logical_rel_path.starts_with("automations/")
            && entry.logical_rel_path.ends_with(".toml")
        {
            let raw_auto = std::fs::read_to_string(&staged_file)?;
            let transformed =
                transform_automation_toml(&raw_auto, &plan.path_mapping_rules, target_style)?;
            write_file_atomically(&dst_path, transformed.as_bytes())?;
        } else {
            // Standard binary-safe copy via atomic temporary file
            copy_file_atomically(&staged_file, &dst_path)?;
        }

        // Restore portable permissions on Unix
        #[cfg(unix)]
        if let Some(mode) = entry.permissions.unix_mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(mode));
        }

        processed_files += 1;
        processed_bytes += entry.size_bytes;
    }

    // Merge SQLite databases (`state_*.sqlite`, `thread_history_*.sqlite`, `memories_*.sqlite`, `goals_*.sqlite`)
    if plan
        .selected_categories
        .contains(&ContentCategory::ChatMetadataAndIndex)
    {
        if let Some(ref state_db_name) = manifest.source_env.sqlite_schemas.state_db_filename {
            let staged_state_db = staging_dir.join("codex_home").join(state_db_name);
            if staged_state_db.is_file() {
                let target_existing_schema =
                    crate::adapters::codex::inspect_codex_databases(target_codex_home)
                        .ok()
                        .map(|(r, _)| r);
                let resolved_target_db_name = target_existing_schema
                    .and_then(|r| r.state_db_filename)
                    .unwrap_or_else(|| state_db_name.clone());
                let target_state_db = target_codex_home.join(&resolved_target_db_name);
                backup_and_record_target_file(
                    journal,
                    &target_state_db,
                    &format!("codex_home/{resolved_target_db_name}"),
                )?;
                merge_state_sqlite_database(
                    &staged_state_db,
                    &target_state_db,
                    plan,
                    &plan.path_mapping_rules,
                    target_style,
                )?;
            }
        }

        if let Some(ref th_db_name) = manifest
            .source_env
            .sqlite_schemas
            .thread_history_db_filename
        {
            let staged_th_db = staging_dir.join("codex_home").join(th_db_name);
            if staged_th_db.is_file() {
                let target_th_db = target_codex_home.join(th_db_name);
                backup_and_record_target_file(
                    journal,
                    &target_th_db,
                    &format!("codex_home/{th_db_name}"),
                )?;
                merge_Auxiliary_sqlite_database(&staged_th_db, &target_th_db, plan)?;
            }
        }
    }

    if plan
        .selected_categories
        .contains(&ContentCategory::ExtensionsMemoriesRulesHooks)
    {
        for opt_db in [
            &manifest.source_env.sqlite_schemas.memories_db_filename,
            &manifest.source_env.sqlite_schemas.goals_db_filename,
        ]
        .into_iter()
        .flatten()
        {
            let staged_db = staging_dir.join("codex_home").join(opt_db);
            if staged_db.is_file() {
                let target_db = target_codex_home.join(opt_db);
                backup_and_record_target_file(
                    journal,
                    &target_db,
                    &format!("codex_home/{opt_db}"),
                )?;
                merge_Auxiliary_sqlite_database(&staged_db, &target_db, plan)?;
                if opt_db.starts_with("goals_") {
                    pause_imported_goals_in_sqlite(&target_db)?;
                }
            }
        }
    }

    Ok((processed_files, warnings))
}

fn resolve_destination_for_entry<'a>(
    entry: &crate::inventory::ManifestFileEntry,
    target_codex_home: &Path,
    target_shared_skills_dir: Option<&Path>,
    project_target_lookup: &HashMap<String, (String, bool)>,
    rollout_action_lookup: &HashMap<String, &'a PlannedThreadAction>,
    limits: &SecurityLimits,
) -> AppResult<Option<(PathBuf, Option<&'a str>)>> {
    if entry.category == ContentCategory::ProjectFiles {
        let Some(ref pid) = entry.project_id else {
            return Ok(None);
        };
        let Some((target_root, copy_enabled)) = project_target_lookup.get(pid) else {
            return Ok(None);
        };
        if !*copy_enabled {
            return Ok(None);
        }
        let dst = resolve_safe_child_path(
            Path::new(target_root),
            &entry.logical_rel_path,
            limits.max_path_length,
        )?;
        return Ok(Some((dst, None)));
    }

    if entry.category == ContentCategory::SharedAgentSkills {
        let Some(ss_root) = target_shared_skills_dir else {
            return Ok(None);
        };
        let dst =
            resolve_safe_child_path(ss_root, &entry.logical_rel_path, limits.max_path_length)?;
        return Ok(Some((dst, None)));
    }

    // Check if this rollout file belongs to a planned thread action (skip DeduplicateIdentical, rename & remap KeepBothForked)
    if entry.category == ContentCategory::ActiveAndArchivedChats {
        for (src_rollout_norm, action) in rollout_action_lookup {
            if src_rollout_norm.ends_with(&entry.logical_rel_path) {
                match action.resolution {
                    ThreadConflictResolution::DeduplicateIdentical => {
                        return Ok(None);
                    }
                    ThreadConflictResolution::KeepBothForked => {
                        return Ok(Some((
                            PathBuf::from(&action.mapped_rollout_path),
                            Some(action.target_thread_id.as_str()),
                        )));
                    }
                    ThreadConflictResolution::ImportNew => {
                        break;
                    }
                }
            }
        }
    }

    let dst = resolve_safe_child_path(
        target_codex_home,
        &entry.logical_rel_path,
        limits.max_path_length,
    )?;
    Ok(Some((dst, None)))
}

fn write_file_atomically(dst: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = dst.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut tmp, bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(dst).map_err(|e| AppError::Io(e.error))?;
    Ok(())
}

fn copy_file_atomically(src: &Path, dst: &Path) -> AppResult<()> {
    let parent = dst.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    let mut in_file = std::fs::File::open(src)?;
    std::io::copy(&mut in_file, &mut tmp)?;
    tmp.as_file().sync_all()?;
    tmp.persist(dst).map_err(|e| AppError::Io(e.error))?;
    Ok(())
}

fn merge_session_index_jsonl(
    staged_file: &Path,
    target_file: &Path,
    thread_actions: &[PlannedThreadAction],
) -> AppResult<()> {
    let mut lines_out = Vec::new();
    let mut existing_Set = HashSet::new();

    if target_file.is_file() {
        let existing_raw = std::fs::read_to_string(target_file)?;
        for l in existing_raw.lines() {
            let trimmed = l.trim();
            if !trimmed.is_empty() {
                existing_Set.insert(trimmed.to_string());
                lines_out.push(trimmed.to_string());
            }
        }
    }

    let action_map: HashMap<&str, &PlannedThreadAction> = thread_actions
        .iter()
        .map(|a| (a.source_thread_id.as_str(), a))
        .collect();

    let src_raw = std::fs::read_to_string(staged_file)?;
    for l in src_raw.lines() {
        let trimmed = l.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(mut obj) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(id_str) = obj
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            {
                if let Some(act) = action_map.get(id_str.as_str()) {
                    if act.resolution == ThreadConflictResolution::DeduplicateIdentical {
                        continue;
                    }
                    if act.resolution == ThreadConflictResolution::KeepBothForked {
                        obj["id"] = serde_json::Value::String(act.target_thread_id.clone());
                        obj["thread_name"] = serde_json::Value::String(act.title.clone());
                    }
                }
            }
            let ser = serde_json::to_string(&obj)?;
            if existing_Set.insert(ser.clone()) {
                lines_out.push(ser);
            }
        }
    }

    let joined = format!("{}\n", lines_out.join("\n"));
    write_file_atomically(target_file, joined.as_bytes())
}

/// Merges `state_4.sqlite` / `state_5.sqlite` into target database using parameterized SQL:
/// - If target DB does not exist yet, copies staged DB and rewrites paths and thread actions in place.
/// - If target DB already exists, attaches staged DB as `src_db` and merges `projects`, `project_roots`,
///   `thread_sections`, `threads`, `thread_attachments`, `thread_dynamic_tools`, `thread_spawn_edges`,
///   `thread_read_receipts`, and `guardian_review_feedback`, applying path mapping and fork resolution.
fn merge_state_sqlite_database(
    staged_db: &Path,
    target_db: &Path,
    plan: &ImportPlan,
    rules: &[PathMappingRule],
    target_style: PathStyle,
) -> AppResult<()> {
    if !target_db.exists() {
        copy_file_atomically(staged_db, target_db)?;
        rewrite_paths_and_forks_in_source_db(target_db, plan, rules, target_style)?;
        return Ok(());
    }

    // Create a working copy of staged_db with paths & forked thread IDs rewritten first,
    // then merge rows into target_db inside a transaction.
    let tmp_dir = tempfile::tempdir()?;
    let prepped_src = tmp_dir.path().join("prepped_state.sqlite");
    std::fs::copy(staged_db, &prepped_src)?;
    rewrite_paths_and_forks_in_source_db(&prepped_src, plan, rules, target_style)?;

    let mut dst_conn = Connection::open(target_db)?;
    let dst_tables: HashSet<String> = dst_conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();

    dst_conn.execute(
        "ATTACH DATABASE ?1 AS src_db",
        params![prepped_src.to_string_lossy().to_string()],
    )?;

    let tx = dst_conn.transaction()?;

    for table in [
        "projects",
        "project_roots",
        "thread_sections",
        "threads",
        "thread_attachments",
        "thread_dynamic_tools",
        "thread_spawn_edges",
        "thread_read_receipts",
        "guardian_review_feedback",
    ] {
        if dst_tables.contains(table) {
            merge_attached_table_intersection(&tx, table)?;
        }
    }

    tx.commit()?;
    let _ = dst_conn.execute("DETACH DATABASE src_db", []);
    Ok(())
}

fn rewrite_paths_inside_state_db(
    db_path: &Path,
    rules: &[PathMappingRule],
    target_style: PathStyle,
) -> AppResult<()> {
    let mut conn = Connection::open(db_path)?;
    let tables: HashSet<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();

    let tx = conn.transaction()?;

    if tables.contains("threads") {
        let rows: Vec<(String, String, String)> = {
            let mut stmt = tx.prepare("SELECT id, cwd, rollout_path FROM threads")?;
            let mapped = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .filter_map(|r| r.ok())
                .collect();
            mapped
        };
        for (id, cwd, rollout_path) in rows {
            let new_cwd = rewrite_path_with_rules(&cwd, rules, target_style).unwrap_or(cwd);
            let new_rollout =
                rewrite_path_with_rules(&rollout_path, rules, target_style).unwrap_or(rollout_path);
            tx.execute(
                "UPDATE threads SET cwd = ?1, rollout_path = ?2 WHERE id = ?3",
                params![new_cwd, new_rollout, id],
            )?;
        }
    }

    if tables.contains("project_roots") {
        let rows: Vec<(String, i64, String)> = {
            let mut stmt = tx.prepare("SELECT project_id, position, path FROM project_roots")?;
            let mapped = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .filter_map(|r| r.ok())
                .collect();
            mapped
        };
        for (pid, pos, p) in rows {
            if let Some(new_p) = rewrite_path_with_rules(&p, rules, target_style) {
                tx.execute(
                    "UPDATE project_roots SET path = ?1 WHERE project_id = ?2 AND position = ?3",
                    params![new_p, pid, pos],
                )?;
            }
        }
    }

    if tables.contains("thread_attachments") {
        let rows: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT id, payload FROM thread_attachments")?;
            let mapped = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect();
            mapped
        };
        for (id, payload_str) in rows {
            if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                if let Some(obj) = val.as_object_mut() {
                    for k in ["root", "workspaceRoot", "sourceCwd"] {
                        if let Some(serde_json::Value::String(s)) = obj.get(k) {
                            if let Some(mapped) = rewrite_path_with_rules(s, rules, target_style) {
                                obj.insert(k.to_string(), serde_json::Value::String(mapped));
                            }
                        }
                    }
                    let new_payload = serde_json::to_string(&val)?;
                    tx.execute(
                        "UPDATE thread_attachments SET payload = ?1 WHERE id = ?2",
                        params![new_payload, id],
                    )?;
                }
            }
        }
    }

    tx.commit()?;
    Ok(())
}

fn rewrite_paths_and_forks_in_source_db(
    src_db: &Path,
    plan: &ImportPlan,
    rules: &[PathMappingRule],
    target_style: PathStyle,
) -> AppResult<()> {
    rewrite_paths_inside_state_db(src_db, rules, target_style)?;
    let mut conn = Connection::open(src_db)?;
    let tables: HashSet<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();

    let tx = conn.transaction()?;

    for action in &plan.thread_actions {
        match action.resolution {
            ThreadConflictResolution::DeduplicateIdentical => {
                if tables.contains("threads") {
                    tx.execute(
                        "DELETE FROM threads WHERE id = ?1",
                        params![action.source_thread_id],
                    )?;
                }
                for aux_table in [
                    "thread_attachments",
                    "thread_dynamic_tools",
                    "thread_read_receipts",
                    "guardian_review_feedback",
                ] {
                    if tables.contains(aux_table)
                        && query_table_columns(&tx, aux_table)?.contains("thread_id")
                    {
                        let sql = format!("DELETE FROM \"{aux_table}\" WHERE thread_id = ?1");
                        tx.execute(&sql, params![action.source_thread_id])?;
                    }
                }
                if tables.contains("thread_spawn_edges") {
                    tx.execute(
                        "DELETE FROM thread_spawn_edges WHERE parent_thread_id = ?1 OR child_thread_id = ?1",
                        params![action.source_thread_id],
                    )?;
                }
            }
            ThreadConflictResolution::KeepBothForked => {
                if tables.contains("threads") {
                    tx.execute(
                        "UPDATE threads SET id = ?1, title = ?2, rollout_path = ?3, cwd = ?4 WHERE id = ?5",
                        params![
                            action.target_thread_id,
                            action.title,
                            action.mapped_rollout_path,
                            action.mapped_cwd,
                            action.source_thread_id
                        ],
                    )?;
                }
                if tables.contains("thread_attachments") {
                    let cols = query_table_columns(&tx, "thread_attachments")?;
                    if cols.contains("id") && cols.contains("thread_id") {
                        tx.execute(
                            "UPDATE thread_attachments SET id = id || '-' || ?1, thread_id = ?1 WHERE thread_id = ?2",
                            params![action.target_thread_id, action.source_thread_id],
                        )?;
                    } else if cols.contains("thread_id") {
                        tx.execute(
                            "UPDATE thread_attachments SET thread_id = ?1 WHERE thread_id = ?2",
                            params![action.target_thread_id, action.source_thread_id],
                        )?;
                    }
                }
                for aux_table in [
                    "thread_dynamic_tools",
                    "thread_read_receipts",
                    "guardian_review_feedback",
                ] {
                    if tables.contains(aux_table)
                        && query_table_columns(&tx, aux_table)?.contains("thread_id")
                    {
                        let sql = format!(
                            "UPDATE \"{aux_table}\" SET thread_id = ?1 WHERE thread_id = ?2"
                        );
                        tx.execute(
                            &sql,
                            params![action.target_thread_id, action.source_thread_id],
                        )?;
                    }
                }
                if tables.contains("thread_spawn_edges") {
                    tx.execute(
                        "UPDATE thread_spawn_edges SET parent_thread_id = ?1 WHERE parent_thread_id = ?2",
                        params![action.target_thread_id, action.source_thread_id],
                    )?;
                    tx.execute(
                        "UPDATE thread_spawn_edges SET child_thread_id = ?1 WHERE child_thread_id = ?2",
                        params![action.target_thread_id, action.source_thread_id],
                    )?;
                }
            }
            ThreadConflictResolution::ImportNew => {}
        }
    }

    tx.commit()?;
    Ok(())
}

fn merge_attached_table_intersection(
    tx: &rusqlite::Transaction,
    table_name: &str,
) -> AppResult<()> {
    let dst_cols = query_table_columns(tx, table_name)?;

    let src_pragma = format!("PRAGMA src_db.table_info(\"{table_name}\")");
    let src_cols: HashSet<String> = {
        let mut stmt = tx.prepare(&src_pragma)?;
        let mapped = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        mapped
    };

    // Ensure every NOT NULL column in target (without a default value) is present in source
    let dst_pragma = format!("PRAGMA main.table_info(\"{table_name}\")");
    {
        let mut stmt = tx.prepare(&dst_pragma)?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, i64>(3).unwrap_or(0),
                r.get::<_, Option<String>>(4).ok().flatten(),
                r.get::<_, i64>(5).unwrap_or(0),
            ))
        })?;
        for col_res in rows {
            let (col_name, notnull, dflt_val, pk) = col_res?;
            if notnull != 0 && dflt_val.is_none() && pk == 0 && !src_cols.contains(&col_name) {
                return Err(AppError::new(
                    ErrorCode::UnsupportedDatabaseSchema,
                    format!(
                        "Target table '{table_name}' requires NOT NULL column '{col_name}' with no default value, which is missing from source database."
                    ),
                ));
            }
        }
    }

    let mut common_cols: Vec<String> = dst_cols.intersection(&src_cols).cloned().collect();
    common_cols.sort();
    if common_cols.is_empty() {
        return Ok(());
    }

    let quoted_cols = common_cols
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");

    // Use `ON CONFLICT DO NOTHING` instead of `INSERT OR IGNORE` so NOT NULL / CHECK constraint errors are never silently ignored!
    let sql = format!(
        "INSERT INTO main.\"{table_name}\" ({quoted_cols}) SELECT {quoted_cols} FROM src_db.\"{table_name}\" WHERE true ON CONFLICT DO NOTHING"
    );
    tx.execute(&sql, [])?;
    Ok(())
}

fn rewrite_thread_actions_in_auxiliary_db(aux_db: &Path, plan: &ImportPlan) -> AppResult<()> {
    let mut conn = Connection::open(aux_db)?;
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .filter(|t| t != "_sqlx_migrations" && t != "sqlite_sequence")
        .collect();

    let tx = conn.transaction()?;
    for table in &tables {
        let cols = query_table_columns(&tx, table)?;
        if !cols.contains("thread_id") {
            continue;
        }
        let del_sql = format!("DELETE FROM \"{table}\" WHERE thread_id = ?1");
        let upd_sql = format!("UPDATE \"{table}\" SET thread_id = ?1 WHERE thread_id = ?2");
        for action in &plan.thread_actions {
            match action.resolution {
                ThreadConflictResolution::DeduplicateIdentical => {
                    tx.execute(&del_sql, params![action.source_thread_id])?;
                }
                ThreadConflictResolution::KeepBothForked => {
                    tx.execute(
                        &upd_sql,
                        params![action.target_thread_id, action.source_thread_id],
                    )?;
                }
                ThreadConflictResolution::ImportNew => {}
            }
        }
    }
    tx.commit()?;
    Ok(())
}

fn merge_Auxiliary_sqlite_database(
    staged_db: &Path,
    target_db: &Path,
    plan: &ImportPlan,
) -> AppResult<()> {
    if !target_db.exists() {
        copy_file_atomically(staged_db, target_db)?;
        rewrite_thread_actions_in_auxiliary_db(target_db, plan)?;
        return Ok(());
    }

    let tmp_dir = tempfile::tempdir()?;
    let prepped_aux = tmp_dir.path().join("prepped_aux.sqlite");
    std::fs::copy(staged_db, &prepped_aux)?;
    rewrite_thread_actions_in_auxiliary_db(&prepped_aux, plan)?;

    let mut dst_conn = Connection::open(target_db)?;
    let dst_tables: Vec<String> = dst_conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .filter(|t| t != "_sqlx_migrations" && t != "sqlite_sequence")
        .collect();

    dst_conn.execute(
        "ATTACH DATABASE ?1 AS src_db",
        params![prepped_aux.to_string_lossy().to_string()],
    )?;
    let tx = dst_conn.transaction()?;
    for table in &dst_tables {
        merge_attached_table_intersection(&tx, table)?;
    }
    tx.commit()?;
    let _ = dst_conn.execute("DETACH DATABASE src_db", []);
    Ok(())
}

fn pause_imported_goals_in_sqlite(goals_db: &Path) -> AppResult<()> {
    if !goals_db.is_file() {
        return Ok(());
    }
    let conn = Connection::open(goals_db)?;
    let _ = conn.execute(
        "UPDATE thread_goals SET status = 'paused' WHERE status = 'active'",
        [],
    );
    Ok(())
}
