use crate::adapters::codex::{
    inspect_codex_databases, read_threads_and_projects, CodexThreadSummary,
};
use crate::errors::AppResult;
use crate::inventory::{ContentCategory, MigrationManifest};
use crate::paths::{
    validate_project_target_roots, validate_relative_paths_for_target, PathMappingRule, PathStyle,
    PathValidationIssue,
};
use crate::platform::available_disk_space;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadConflictResolution {
    /// Identical content on both computers -> deduplicate automatically
    DeduplicateIdentical,
    /// Only exists on source -> import normally
    ImportNew,
    /// Same thread ID exists on target with divergent content/messages -> keep both by importing source branch as a parallel thread ID
    KeepBothForked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedThreadAction {
    pub source_thread_id: String,
    pub target_thread_id: String,
    pub title: String,
    pub resolution: ThreadConflictResolution,
    pub mapped_cwd: String,
    pub mapped_rollout_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectPathMappingConfig {
    pub project_id: String,
    pub project_name: String,
    pub source_root: String,
    pub target_root: String,
    pub copy_project_files: bool,
    pub bind_to_existing_target_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFieldConflict {
    pub field_path: String,
    pub source_summary: String,
    pub target_summary: String,
    pub resolution: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceRequirementReport {
    pub staging_bytes_needed: u64,
    pub backup_bytes_needed: u64,
    pub final_write_bytes_needed: u64,
    pub total_required_bytes: u64,
    pub available_bytes: u64,
    pub sufficient: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPlan {
    pub migration_id: String,
    pub target_codex_home: String,
    pub target_shared_skills_dir: Option<String>,
    pub target_has_existing_data: bool,
    pub selected_categories: Vec<ContentCategory>,
    pub include_sensitive_files: bool,
    pub path_mapping_rules: Vec<PathMappingRule>,
    pub project_mappings: Vec<ProjectPathMappingConfig>,
    pub thread_actions: Vec<PlannedThreadAction>,
    pub config_conflicts: Vec<ConfigFieldConflict>,
    pub validation_issues: Vec<PathValidationIssue>,
    pub space_report: SpaceRequirementReport,
    pub can_proceed: bool,
    pub blocking_reasons: Vec<String>,
}

/// Automatically generates default target paths for all projects inside `default_projects_root`
/// on the new computer, handling duplicate project names cleanly.
pub fn propose_default_project_mappings(
    manifest: &MigrationManifest,
    default_projects_root: &str,
    target_style: PathStyle,
) -> Vec<ProjectPathMappingConfig> {
    let sep = target_style.separator();
    let clean_base = default_projects_root.trim_end_matches(&['/', '\\'][..]);
    let mut used_names: HashSet<String> = HashSet::new();
    let mut mappings = Vec::new();

    for proj in &manifest.projects {
        let src_root = proj.source_root_paths.first().cloned().unwrap_or_default();

        let base_folder_name = sanitize_folder_name(&proj.name);
        let mut candidate_name = base_folder_name.clone();
        let mut counter = 2usize;
        while !used_names.insert(candidate_name.to_lowercase()) {
            candidate_name = format!("{base_folder_name}-{counter}");
            counter += 1;
        }

        let target_root = format!("{clean_base}{sep}{candidate_name}");
        let target_exists = Path::new(&target_root).exists();

        mappings.push(ProjectPathMappingConfig {
            project_id: proj.project_id.clone(),
            project_name: proj.name.clone(),
            source_root: src_root,
            target_root,
            copy_project_files: proj.include_project_files,
            bind_to_existing_target_dir: target_exists && !proj.include_project_files,
        });
    }

    mappings
}

fn sanitize_folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "codex-project".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Builds a comprehensive `ImportPlan` before any modification happens on the target machine.
#[allow(clippy::too_many_arguments)]
pub fn build_import_plan(
    manifest: &MigrationManifest,
    target_codex_home: &Path,
    target_shared_skills_dir: Option<&Path>,
    selected_categories: &[ContentCategory],
    project_mappings: &[ProjectPathMappingConfig],
    include_sensitive_files: bool,
    staging_already_complete: bool,
) -> AppResult<ImportPlan> {
    let target_style = PathStyle::current();
    let mut blocking_reasons = Vec::new();

    if !manifest.adapter_capabilities.can_write_threads
        && selected_categories.contains(&ContentCategory::ChatMetadataAndIndex)
    {
        blocking_reasons.push(
            "Source Codex database uses an unsupported schema version. Only raw read-only backup is permitted."
                .to_string(),
        );
    }

    // 1. Build structured PathMappingRules (projects first, then CODEX_HOME, then shared_skills)
    let mut rules = Vec::new();
    for pm in project_mappings {
        if !pm.source_root.is_empty() && !pm.target_root.is_empty() {
            rules.push(PathMappingRule {
                project_id: Some(pm.project_id.clone()),
                source_prefix: pm.source_root.clone(),
                target_prefix: pm.target_root.clone(),
            });
        }
    }
    rules.push(PathMappingRule {
        project_id: None,
        source_prefix: manifest.source_env.codex_home.clone(),
        target_prefix: target_codex_home.to_string_lossy().to_string(),
    });
    if let (Some(ref src_ss), Some(dst_ss)) = (
        &manifest.source_env.shared_skills_dir,
        target_shared_skills_dir,
    ) {
        rules.push(PathMappingRule {
            project_id: None,
            source_prefix: src_ss.clone(),
            target_prefix: dst_ss.to_string_lossy().to_string(),
        });
    }

    // 2. Validate target project roots for overlaps and unapproved existing directories
    let proj_tuples: Vec<(String, String, bool)> = project_mappings
        .iter()
        .map(|pm| {
            (
                pm.project_id.clone(),
                pm.target_root.clone(),
                pm.bind_to_existing_target_dir,
            )
        })
        .collect();
    let mut validation_issues = validate_project_target_roots(&proj_tuples, target_style);

    // 3. Validate relative file paths against target OS filesystem rules
    let selected_set: HashSet<ContentCategory> = selected_categories.iter().copied().collect();
    let rel_paths: Vec<String> = manifest
        .files
        .iter()
        .filter(|f| selected_set.contains(&f.category))
        .map(|f| f.logical_rel_path.clone())
        .collect();
    let target_case_insensitive = cfg!(windows) || cfg!(target_os = "macos");
    validation_issues.extend(validate_relative_paths_for_target(
        &rel_paths,
        target_style,
        target_case_insensitive,
    ));

    // 4. Inspect existing target Codex data for merge/dedup/fork resolution
    let target_has_existing_data = target_codex_home.exists()
        && (target_codex_home.join("config.toml").exists()
            || target_codex_home.join("sessions").exists());

    let mut existing_threads_map: HashMap<String, CodexThreadSummary> = HashMap::new();
    let mut existing_by_fingerprint: HashMap<String, CodexThreadSummary> = HashMap::new();
    let mut used_thread_ids: HashSet<String> =
        manifest.threads.iter().map(|t| t.id.clone()).collect();

    if target_has_existing_data {
        let (target_schema, target_caps) = inspect_codex_databases(target_codex_home)?;
        if !target_caps.can_write_threads {
            blocking_reasons.push(
                "Target computer has an unrecognized or corrupted Codex state database schema. Refusing to overwrite or corrupt target database."
                    .to_string(),
            );
        }
        if selected_categories.contains(&ContentCategory::ChatMetadataAndIndex) {
            if let (Some(src_ver), Some(dst_ver)) = (
                manifest.source_env.sqlite_schemas.state_schema_version,
                target_schema.state_schema_version,
            ) {
                if src_ver > dst_ver {
                    blocking_reasons.push(format!(
                        "Source Codex database schema (state_{src_ver}.sqlite) is newer than target computer's Codex schema (state_{dst_ver}.sqlite). Please update Codex on the receiving computer before migrating."
                    ));
                }
            }
            if let (Some(src_mig), Some(dst_mig)) = (
                manifest.source_env.sqlite_schemas.state_migration_version,
                target_schema.state_migration_version,
            ) {
                if src_mig > dst_mig {
                    blocking_reasons.push(format!(
                        "Source Codex database migration version ({src_mig}) is newer than target computer's migration version ({dst_mig}). Please update Codex on the receiving computer first to avoid losing new columns."
                    ));
                }
            }
            if let (Some(src_th_mig), Some(dst_th_mig)) = (
                manifest
                    .source_env
                    .sqlite_schemas
                    .thread_history_migration_version,
                target_schema.thread_history_migration_version,
            ) {
                if src_th_mig > dst_th_mig {
                    blocking_reasons.push(format!(
                        "Source thread_history database migration version ({src_th_mig}) is newer than target's ({dst_th_mig}). Please update Codex on the receiving computer first."
                    ));
                }
            }
        }
        if let Ok((t_threads, _)) = read_threads_and_projects(target_codex_home, &target_schema) {
            for t in t_threads {
                used_thread_ids.insert(t.id.clone());
                if let Some(ref fp) = t.content_fingerprint {
                    existing_by_fingerprint
                        .entry(fp.clone())
                        .or_insert_with(|| t.clone());
                }
                existing_threads_map.insert(t.id.clone(), t);
            }
        }
    }

    let mut thread_actions = Vec::new();
    for src_t in &manifest.threads {
        let mapped_cwd = crate::paths::rewrite_path_with_rules(&src_t.cwd, &rules, target_style)
            .unwrap_or_else(|| src_t.cwd.clone());
        let mapped_rollout =
            crate::paths::rewrite_path_with_rules(&src_t.rollout_path, &rules, target_style)
                .unwrap_or_else(|| src_t.rollout_path.clone());

        if let Some(target_t) = existing_threads_map.get(&src_t.id) {
            let content_verified_identical = match (
                src_t.content_fingerprint.as_ref(),
                target_t.content_fingerprint.as_ref(),
            ) {
                (Some(src_fp), Some(dst_fp)) => src_fp == dst_fp && src_t.record_count > 0,
                _ => false,
            };

            if content_verified_identical {
                thread_actions.push(PlannedThreadAction {
                    source_thread_id: src_t.id.clone(),
                    target_thread_id: src_t.id.clone(),
                    title: src_t.title.clone(),
                    resolution: ThreadConflictResolution::DeduplicateIdentical,
                    mapped_cwd,
                    mapped_rollout_path: target_t.rollout_path.clone(),
                });
            } else if let Some(existing_fork) = src_t
                .content_fingerprint
                .as_ref()
                .and_then(|fp| existing_by_fingerprint.get(fp))
            {
                // Repeated import of a thread that was already forked onto target in a prior run:
                // deduplicate against the existing fork rather than creating duplicate forks.
                thread_actions.push(PlannedThreadAction {
                    source_thread_id: src_t.id.clone(),
                    target_thread_id: existing_fork.id.clone(),
                    title: existing_fork.title.clone(),
                    resolution: ThreadConflictResolution::DeduplicateIdentical,
                    mapped_cwd,
                    mapped_rollout_path: existing_fork.rollout_path.clone(),
                });
            } else {
                // Divergent chat with the same ID: NEVER discard either side by title/timestamp!
                // Generate a collision-free valid UUIDv7 for the forked chat.
                let mut forked_id = Uuid::now_v7().to_string();
                while !used_thread_ids.insert(forked_id.clone()) {
                    forked_id = Uuid::now_v7().to_string();
                }
                let forked_rollout =
                    build_forked_rollout_path(&mapped_rollout, &src_t.id, &forked_id);
                thread_actions.push(PlannedThreadAction {
                    source_thread_id: src_t.id.clone(),
                    target_thread_id: forked_id,
                    title: format!("{} (Migrated Copy)", src_t.title),
                    resolution: ThreadConflictResolution::KeepBothForked,
                    mapped_cwd,
                    mapped_rollout_path: forked_rollout,
                });
            }
        } else {
            thread_actions.push(PlannedThreadAction {
                source_thread_id: src_t.id.clone(),
                target_thread_id: src_t.id.clone(),
                title: src_t.title.clone(),
                resolution: ThreadConflictResolution::ImportNew,
                mapped_cwd,
                mapped_rollout_path: mapped_rollout,
            });
        }
    }

    // 5. Inspect config conflicts
    let mut config_conflicts = Vec::new();
    let target_cfg_path = target_codex_home.join("config.toml");
    if target_cfg_path.is_file() {
        config_conflicts.push(ConfigFieldConflict {
            field_path: "auth.json & local keychain".to_string(),
            source_summary: "Excluded from migration".to_string(),
            target_summary: "Existing target authentication preserved".to_string(),
            resolution: "KeepTargetProtected".to_string(),
        });
        config_conflicts.push(ConfigFieldConflict {
            field_path: "config.toml [projects.*]".to_string(),
            source_summary: format!("{} project trust entries", manifest.projects.len()),
            target_summary: "Existing target project entries".to_string(),
            resolution: "MergeWithMappedPaths".to_string(),
        });
    }

    // 6. Compute disk space requirements (staging + backup + final write)
    let write_bytes: u64 = manifest
        .files
        .iter()
        .filter(|f| selected_set.contains(&f.category))
        .filter(|f| include_sensitive_files || !f.is_sensitive)
        .map(|f| f.size_bytes)
        .sum();

    let staging_bytes_needed = if staging_already_complete {
        0
    } else {
        write_bytes
    };

    let backup_bytes_needed = estimate_existing_codex_size(target_codex_home);
    let safety_margin = 32 * 1024 * 1024; // 32 MiB buffer for SQLite WAL & journal
    let total_required_bytes =
        staging_bytes_needed + backup_bytes_needed + write_bytes + safety_margin;

    let probe_path = target_codex_home.parent().unwrap_or_else(|| Path::new("."));
    let available_bytes = available_disk_space(probe_path);
    let sufficient = available_bytes == 0 || available_bytes >= total_required_bytes;

    if !sufficient {
        blocking_reasons.push(format!(
            "Insufficient disk space on target computer: requires {} MB (including staging and rollback backup), but only {} MB available.",
            total_required_bytes / (1024 * 1024),
            available_bytes / (1024 * 1024)
        ));
    }

    for issue in &validation_issues {
        if issue.blocking {
            blocking_reasons.push(issue.description.clone());
        }
    }

    let can_proceed = blocking_reasons.is_empty();

    Ok(ImportPlan {
        migration_id: manifest.migration_id.clone(),
        target_codex_home: target_codex_home.to_string_lossy().to_string(),
        target_shared_skills_dir: target_shared_skills_dir.map(|p| p.to_string_lossy().to_string()),
        target_has_existing_data,
        selected_categories: selected_categories.to_vec(),
        include_sensitive_files,
        path_mapping_rules: rules,
        project_mappings: project_mappings.to_vec(),
        thread_actions,
        config_conflicts,
        validation_issues,
        space_report: SpaceRequirementReport {
            staging_bytes_needed,
            backup_bytes_needed,
            final_write_bytes_needed: write_bytes,
            total_required_bytes,
            available_bytes,
            sufficient,
        },
        can_proceed,
        blocking_reasons,
    })
}

fn build_forked_rollout_path(
    path_str: &str,
    source_thread_id: &str,
    target_thread_id: &str,
) -> String {
    if !source_thread_id.is_empty() && path_str.contains(source_thread_id) {
        if let Some(pos) = path_str.rfind(source_thread_id) {
            let mut out = String::with_capacity(path_str.len() + target_thread_id.len());
            out.push_str(&path_str[..pos]);
            out.push_str(target_thread_id);
            out.push_str(&path_str[pos + source_thread_id.len()..]);
            return out;
        }
    }
    if let Some(stem) = path_str.strip_suffix(".jsonl") {
        format!("{stem}-{target_thread_id}.jsonl")
    } else if let Some(stem) = path_str.strip_suffix(".jsonl.zst") {
        format!("{stem}-{target_thread_id}.jsonl.zst")
    } else {
        format!("{path_str}-{target_thread_id}")
    }
}

fn estimate_existing_codex_size(target_codex_home: &Path) -> u64 {
    if !target_codex_home.is_dir() {
        return 0;
    }
    let mut total = 0u64;
    for sub in [
        "state_5.sqlite",
        "state_4.sqlite",
        "thread_history_1.sqlite",
        "memories_1.sqlite",
        "goals_1.sqlite",
        "config.toml",
        "session_index.jsonl",
        ".codex-global-state.json",
    ] {
        let p = target_codex_home.join(sub);
        if let Ok(meta) = p.metadata() {
            total += meta.len();
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::codex::{AdapterCapabilities, CodexThreadSummary, SqliteSchemaReport};
    use crate::inventory::SourceEnvironmentInfo;
    use tempfile::tempdir;

    #[test]
    fn test_divergent_threads_with_same_id_are_both_preserved() {
        let dir = tempdir().unwrap();
        let target_codex = dir.path().join(".codex");
        std::fs::create_dir_all(&target_codex).unwrap();
        std::fs::write(target_codex.join("config.toml"), "model = \"gpt-5\"\n").unwrap();

        // Create target state_5.sqlite with an existing thread "thread-001" updated at t=2000
        let db_path = target_codex.join("state_5.sqlite");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, source TEXT NOT NULL, model_provider TEXT NOT NULL, cwd TEXT NOT NULL, title TEXT NOT NULL, sandbox_policy TEXT NOT NULL, approval_mode TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, sandbox_policy, approval_mode, archived) VALUES ('thread-001', '/Users/bob/.codex/sessions/r1.jsonl', 1000, 2000, 'cli', 'openai', '/Users/bob/proj', 'Target Divergent Chat', 'workspace-write', 'on-request', 0)",
            [],
        )
        .unwrap();
        drop(conn);

        // Source manifest also has "thread-001" with SAME title and SAME updated_at (2000),
        // but different transcript content -> MUST NOT be falsely deduplicated!
        let manifest = MigrationManifest {
            format_version: "1.0.0".to_string(),
            migration_id: "mig-1".to_string(),
            app_version: "0.1.0".to_string(),
            created_at: "2026-10-29T00:00:00Z".to_string(),
            source_env: SourceEnvironmentInfo {
                os: "windows".to_string(),
                arch: "x86_64".to_string(),
                codex_home: r"C:\Users\alice\.codex".to_string(),
                shared_skills_dir: None,
                sqlite_schemas: SqliteSchemaReport {
                    state_db_filename: Some("state_5.sqlite".to_string()),
                    state_schema_version: Some(5),
                    state_migration_version: Some(38),
                    thread_history_db_filename: None,
                    thread_history_migration_version: None,
                    memories_db_filename: None,
                    goals_db_filename: None,
                    has_active_wal: false,
                    unknown_state_databases: vec![],
                },
            },
            adapter_capabilities: AdapterCapabilities {
                adapter_id: "v1".to_string(),
                can_read_threads: true,
                can_write_threads: true,
                can_map_paths: true,
                supported_state_versions: vec![4, 5],
                unknown_tables: vec![],
                compatibility_notes: vec![],
            },
            categories: vec![],
            threads: vec![CodexThreadSummary {
                id: "thread-001".to_string(),
                title: "Target Divergent Chat".to_string(),
                cwd: r"C:\Users\alice\proj".to_string(),
                rollout_path: r"C:\Users\alice\.codex\sessions\r1.jsonl".to_string(),
                created_at: 1000,
                updated_at: 2000,
                archived: false,
                project_id: None,
                model_provider: "openai".to_string(),
                content_fingerprint: Some("source-distinct-content-hash".to_string()),
                record_count: 4,
            }],
            projects: vec![],
            files: vec![],
            excluded_items: vec![],
            skill_warnings: vec![],
            sensitive_findings: vec![],
            general_warnings: vec![],
        };

        let plan = build_import_plan(
            &manifest,
            &target_codex,
            None,
            &[
                ContentCategory::ActiveAndArchivedChats,
                ContentCategory::ChatMetadataAndIndex,
            ],
            &[],
            false,
            true,
        )
        .unwrap();

        assert_eq!(plan.thread_actions.len(), 1);
        assert_eq!(
            plan.thread_actions[0].resolution,
            ThreadConflictResolution::KeepBothForked
        );
        let new_uuid = &plan.thread_actions[0].target_thread_id;
        assert_ne!(new_uuid, "thread-001");
        assert!(Uuid::parse_str(new_uuid).is_ok());
        assert!(plan.thread_actions[0]
            .mapped_rollout_path
            .ends_with(&format!("r1-{new_uuid}.jsonl")));
    }
}
