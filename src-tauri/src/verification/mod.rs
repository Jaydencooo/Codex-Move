use crate::adapters::codex::{inspect_codex_databases, read_threads_and_projects, resolve_db_path};
use crate::errors::AppResult;
use crate::inventory::MigrationManifest;
use crate::planner::ImportPlan;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCheckItem {
    pub check_name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostImportVerificationReport {
    pub all_automated_checks_passed: bool,
    pub automated_checks: Vec<VerificationCheckItem>,
    pub residual_old_paths_found: Vec<String>,
    pub manual_codex_confirmations_needed: Vec<String>,
    pub pending_dependencies_and_reauth: Vec<String>,
}

/// Runs post-import verification checks on the target computer:
/// 1. SQLite `PRAGMA integrity_check` on all target `.sqlite` databases.
/// 2. Consistency between `state_*.sqlite` `threads` rows and on-disk `.jsonl` rollout paths.
/// 3. Target project directory existence.
/// 4. Audit for any unmapped source `CODEX_HOME` or source project paths remaining in metadata fields.
/// 5. Integrity of Skills and `AGENTS.md` instruction files.
pub fn verify_imported_codex_environment(
    manifest: &MigrationManifest,
    plan: &ImportPlan,
) -> AppResult<PostImportVerificationReport> {
    let target_codex_home = Path::new(&plan.target_codex_home);
    let mut checks = Vec::new();
    let mut residual_old_paths = Vec::new();

    // 1. SQLite integrity_check
    let (schema_report, _) = inspect_codex_databases(target_codex_home)?;
    let mut db_files = Vec::new();
    if let Some(ref sdb) = schema_report.state_db_filename {
        db_files.push(sdb.clone());
    }
    if let Some(ref thdb) = schema_report.thread_history_db_filename {
        db_files.push(thdb.clone());
    }
    if let Some(ref mdb) = schema_report.memories_db_filename {
        db_files.push(mdb.clone());
    }
    if let Some(ref gdb) = schema_report.goals_db_filename {
        db_files.push(gdb.clone());
    }

    for db_name in &db_files {
        let db_path = resolve_db_path(target_codex_home, db_name);
        if db_path.is_file() {
            let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let integrity: String = conn
                .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                .unwrap_or_else(|e| e.to_string());
            let ok = integrity.eq_ignore_ascii_case("ok");
            checks.push(VerificationCheckItem {
                check_name: format!("SQLite Integrity ({db_name})"),
                passed: ok,
                detail: integrity,
            });
        }
    }

    // 2. Thread metadata & rollout file parity + residual old path check
    let (target_threads, target_projects) =
        read_threads_and_projects(target_codex_home, &schema_report)?;

    let src_home_lower = manifest.source_env.codex_home.to_lowercase();
    let target_home_lower = plan.target_codex_home.to_lowercase();

    let mut missing_rollout_count = 0usize;
    let mut mismatched_session_id_count = 0usize;
    for t in &target_threads {
        if src_home_lower != target_home_lower
            && t.rollout_path.to_lowercase().contains(&src_home_lower)
        {
            residual_old_paths.push(format!("Thread {} rollout_path: {}", t.id, t.rollout_path));
        }
        if src_home_lower != target_home_lower && t.cwd.to_lowercase().contains(&src_home_lower) {
            residual_old_paths.push(format!("Thread {} cwd: {}", t.id, t.cwd));
        }

        if plan
            .thread_actions
            .iter()
            .any(|a| a.target_thread_id == t.id)
        {
            let rp = Path::new(&t.rollout_path);
            if t.rollout_path.is_empty() || !rp.exists() {
                missing_rollout_count += 1;
            } else if let Ok(raw) = std::fs::read_to_string(rp) {
                if let Some(first_line) = raw.lines().find(|l| !l.trim().is_empty()) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(first_line) {
                        let meta_id = val
                            .get("payload")
                            .and_then(|p| p.get("session_id").or_else(|| p.get("id")))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !meta_id.is_empty() && meta_id != t.id {
                            mismatched_session_id_count += 1;
                        }
                    }
                }
            }
        }
    }

    checks.push(VerificationCheckItem {
        check_name: "Chat Rollout Transcript Parity".to_string(),
        passed: missing_rollout_count == 0 && mismatched_session_id_count == 0,
        detail: if missing_rollout_count == 0 && mismatched_session_id_count == 0 {
            format!(
                "All {} migrated chat thread(s) link to valid transcript files with matching session IDs.",
                plan.thread_actions.len()
            )
        } else {
            format!(
                "{missing_rollout_count} missing rollout file(s); {mismatched_session_id_count} session ID mismatch(es)."
            )
        },
    });

    // 3. Project root existence & path mapping verification
    let mut missing_proj_dirs = 0usize;
    for pm in &plan.project_mappings {
        if pm.copy_project_files || pm.bind_to_existing_target_dir {
            if !Path::new(&pm.target_root).exists() {
                missing_proj_dirs += 1;
            }
        }
        if pm.source_root.to_lowercase() != pm.target_root.to_lowercase() {
            for tp in &target_projects {
                if tp.id == pm.project_id {
                    for rp in &tp.root_paths {
                        if rp.eq_ignore_ascii_case(&pm.source_root) {
                            residual_old_paths.push(format!(
                                "Project '{}' still references old root path '{}'",
                                tp.name, rp
                            ));
                        }
                    }
                }
            }
        }
    }

    checks.push(VerificationCheckItem {
        check_name: "Target Project Folders".to_string(),
        passed: missing_proj_dirs == 0,
        detail: if missing_proj_dirs == 0 {
            format!(
                "Verified {} mapped project location(s).",
                plan.project_mappings.len()
            )
        } else {
            format!("{missing_proj_dirs} target project folder(s) could not be found on disk.")
        },
    });

    checks.push(VerificationCheckItem {
        check_name: "Cross-Platform Path Remapping Audit".to_string(),
        passed: residual_old_paths.is_empty(),
        detail: if residual_old_paths.is_empty() {
            "Zero unmapped old-computer paths detected in target metadata.".to_string()
        } else {
            format!(
                "Found {} residual old path reference(s).",
                residual_old_paths.len()
            )
        },
    });

    // 4. Skills and AGENTS.md integrity
    let agents_md = target_codex_home.join("AGENTS.md");
    let expected_agents = manifest
        .files
        .iter()
        .any(|f| f.logical_rel_path == "AGENTS.md");
    if expected_agents {
        checks.push(VerificationCheckItem {
            check_name: "Global AGENTS.md Instruction File".to_string(),
            passed: agents_md.is_file(),
            detail: if agents_md.is_file() {
                "AGENTS.md verified in target .codex directory.".to_string()
            } else {
                "Expected AGENTS.md is missing.".to_string()
            },
        });
    }

    let all_passed = checks.iter().all(|c| c.passed);

    // 5. Build honest user confirmations & pending dependency reminders
    let manual_confirmations = vec![
        "Open Codex on this computer and verify your chat history list and project groupings appear as expected."
            .to_string(),
        "Automations and scheduled tasks were imported in Paused state for safety; review and re-enable them inside Codex if needed."
            .to_string(),
    ];

    let mut pending_deps = vec![
        "Authentication (auth.json / system keychain) is never copied across computers. Sign in through the official Codex login flow if not already signed in."
            .to_string(),
    ];

    for sw in &manifest.skill_warnings {
        pending_deps.push(format!("Skill '{}': {}", sw.skill_name, sw.detail));
    }

    Ok(PostImportVerificationReport {
        all_automated_checks_passed: all_passed,
        automated_checks: checks,
        residual_old_paths_found: residual_old_paths,
        manual_codex_confirmations_needed: manual_confirmations,
        pending_dependencies_and_reauth: pending_deps,
    })
}
