use codex_move_lib::{
    archive::{create_staging_snapshot, verify_staging_snapshot},
    config::SecurityLimits,
    inventory::{scan_codex_inventory, ContentCategory, ScanOptions},
    migration::{apply_migration_from_staging, FailureInjectionHook, MigrationOutcomeStatus},
    paths::PathStyle,
    planner::{build_import_plan, propose_default_project_mappings, ThreadConflictResolution},
    recovery::{discover_recovery_journals, execute_rollback},
    verification::verify_imported_codex_environment,
};
use rusqlite::{params, Connection};
use std::path::Path;
use tempfile::tempdir;

/// Helper to create a realistic source `.codex` fixture + projects with:
/// - `state_5.sqlite` containing threads, projects, project_roots, thread_attachments
/// - `thread_history_1.sqlite`
/// - `memories_1.sqlite` and `goals_1.sqlite` (with an active goal that must be paused)
/// - `sessions/2026/10/29/rollout-01.jsonl` & `rollout-02.jsonl`
/// - `session_index.jsonl`, `.codex-global-state.json`, `config.toml`, `AGENTS.md`
/// - `skills/custom-skill/SKILL.md` & `automations/daily/automation.toml`
/// - `auth.json` and `logs_2.sqlite` (which must be excluded by default)
fn build_realistic_source_fixture(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let codex_home = base.join("source_user").join(".codex");
    let proj_root = base
        .join("source_user")
        .join("Documents")
        .join("ChatGPT")
        .join("智能助手 🚀");

    std::fs::create_dir_all(codex_home.join("sessions/2026/10/29")).unwrap();
    std::fs::create_dir_all(codex_home.join("skills/custom-linter")).unwrap();
    std::fs::create_dir_all(codex_home.join("automations/daily")).unwrap();
    std::fs::create_dir_all(codex_home.join("memories")).unwrap();
    std::fs::create_dir_all(proj_root.join(".git")).unwrap();
    std::fs::create_dir_all(proj_root.join("src")).unwrap();
    std::fs::create_dir_all(proj_root.join("node_modules/ignored-pkg")).unwrap();

    // 1. Excluded credentials & runtime files
    std::fs::write(
        codex_home.join("auth.json"),
        r#"{"access_token":"sk-live-secret-token-1234567890"}"#,
    )
    .unwrap();
    std::fs::write(codex_home.join("installation_id"), "device-uuid-source").unwrap();
    std::fs::write(codex_home.join("logs_2.sqlite"), b"dummy-log-db").unwrap();

    // 2. Instruction and Skill files
    std::fs::write(
        codex_home.join("AGENTS.md"),
        "# Global Agent Instructions\nAlways run cargo test.",
    )
    .unwrap();
    std::fs::write(
        codex_home.join("skills/custom-linter/SKILL.md"),
        "---\nname: custom-linter\ndescription: Lint helper\n---\nRun linter.",
    )
    .unwrap();
    std::fs::write(
        codex_home.join("memories/MEMORY.md"),
        "# User Memories\nPrefers dark mode and Rust.",
    )
    .unwrap();

    // 3. Automation TOML (initially active -> must become paused on target)
    std::fs::write(
        codex_home.join("automations/daily/automation.toml"),
        format!(
            "version = 1\nid = \"auto-1\"\nname = \"Daily Check\"\nstatus = \"active\"\ncwds = [\"{}\"]\n",
            proj_root.to_string_lossy().replace('\\', "\\\\")
        ),
    )
    .unwrap();

    // 4. Project files (including untracked file, .env, and node_modules which should be excluded)
    std::fs::write(
        proj_root.join("src/main.rs"),
        "fn main() { println!(\"Hello\"); }",
    )
    .unwrap();
    std::fs::write(
        proj_root.join("untracked_notes.md"),
        "Important local draft",
    )
    .unwrap();
    std::fs::write(
        proj_root.join(".env"),
        "DATABASE_URL=postgres://localhost/db\n",
    )
    .unwrap();
    std::fs::write(
        proj_root.join("node_modules/ignored-pkg/index.js"),
        "module.exports = {};",
    )
    .unwrap();

    // 5. Rollout JSONL files
    let r1_path = codex_home.join("sessions/2026/10/29/rollout-01.jsonl");
    let r1_content = format!(
        "{{\"timestamp\":\"2026-10-29T08:00:00Z\",\"ordinal\":0,\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"thread-101\",\"id\":\"thread-101\",\"cwd\":\"{}\",\"runtime_workspace_roots\":[\"{}\"],\"model_provider\":\"openai\"}}}}\n{{\"timestamp\":\"2026-10-29T08:01:00Z\",\"ordinal\":1,\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"Check {} please\"}}}}\n",
        proj_root.to_string_lossy().replace('\\', "\\\\"),
        proj_root.to_string_lossy().replace('\\', "\\\\"),
        proj_root.to_string_lossy().replace('\\', "\\\\")
    );
    std::fs::write(&r1_path, r1_content).unwrap();

    let r2_path = codex_home.join("sessions/2026/10/29/rollout-02.jsonl");
    let r2_content = format!(
        "{{\"timestamp\":\"2026-10-29T09:00:00Z\",\"ordinal\":0,\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"thread-shared-fork\",\"id\":\"thread-shared-fork\",\"cwd\":\"{}\",\"runtime_workspace_roots\":[\"{}\"],\"model_provider\":\"openai\"}}}}\n{{\"timestamp\":\"2026-10-29T09:05:00Z\",\"ordinal\":1,\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"Source machine branch message\"}}}}\n",
        proj_root.to_string_lossy().replace('\\', "\\\\"),
        proj_root.to_string_lossy().replace('\\', "\\\\")
    );
    std::fs::write(&r2_path, r2_content).unwrap();

    // 6. session_index.jsonl
    std::fs::write(
        codex_home.join("session_index.jsonl"),
        "{\"id\":\"thread-101\",\"thread_name\":\"Architecture Review\",\"updated_at\":\"2026-10-29T08:01:00Z\"}\n{\"id\":\"thread-shared-fork\",\"thread_name\":\"Shared Forked Topic\",\"updated_at\":\"2026-10-29T09:05:00Z\"}\n",
    )
    .unwrap();

    // 7. .codex-global-state.json
    let gs = serde_json::json!({
        "local-projects": {
            "proj-uuid-1": {
                "id": "proj-uuid-1",
                "name": "智能助手 🚀",
                "rootPaths": [proj_root.to_string_lossy().to_string()],
                "createdAt": 1700000000000i64,
                "updatedAt": 1700000050000i64
            }
        },
        "thread-project-assignments": {
            "thread-101": {
                "projectKind": "local",
                "projectId": "proj-uuid-1"
            }
        },
        "thread-writable-roots": {
            "thread-101": [proj_root.to_string_lossy().to_string()]
        },
        "electron-local-remote-control-installation-id": "source-only-device-id"
    });
    std::fs::write(
        codex_home.join(".codex-global-state.json"),
        serde_json::to_vec_pretty(&gs).unwrap(),
    )
    .unwrap();

    // 8. config.toml
    let cfg = format!(
        "model = \"gpt-5\"\napproval_policy = \"on-request\"\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        proj_root.to_string_lossy().replace('\\', "\\\\")
    );
    std::fs::write(codex_home.join("config.toml"), cfg).unwrap();

    // 9. state_5.sqlite
    let state_db = codex_home.join("state_5.sqlite");
    let conn = Connection::open(&state_db).unwrap();
    conn.execute_batch(
        "CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY, description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL);
         INSERT INTO _sqlx_migrations VALUES (38, 'v5', CURRENT_TIMESTAMP, 1, X'00', 1);
         CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, metadata TEXT NOT NULL DEFAULT '{}', position INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL);
         CREATE TABLE project_roots (project_id TEXT NOT NULL, position INTEGER NOT NULL, path TEXT NOT NULL, PRIMARY KEY (project_id, position));
         CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, source TEXT NOT NULL, model_provider TEXT NOT NULL, cwd TEXT NOT NULL, title TEXT NOT NULL, sandbox_policy TEXT NOT NULL, approval_mode TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0, project_id TEXT);
         CREATE TABLE thread_attachments (id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, attachment_type TEXT NOT NULL, identity_key TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL);",
    )
    .unwrap();

    conn.execute(
        "INSERT INTO projects VALUES ('proj-uuid-1', '智能助手 🚀', '{}', 0, 1700000000000, 1700000050000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_roots VALUES ('proj-uuid-1', 0, ?1)",
        params![proj_root.to_string_lossy().to_string()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO threads VALUES ('thread-101', ?1, 1700000000, 1700000100, 'desktop', 'openai', ?2, 'Architecture Review', 'workspace-write', 'on-request', 0, 'proj-uuid-1')",
        params![
            r1_path.to_string_lossy().to_string(),
            proj_root.to_string_lossy().to_string()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO threads VALUES ('thread-shared-fork', ?1, 1700000000, 1700000200, 'cli', 'openai', ?2, 'Shared Forked Topic', 'workspace-write', 'on-request', 0, 'proj-uuid-1')",
        params![
            r2_path.to_string_lossy().to_string(),
            proj_root.to_string_lossy().to_string()
        ],
    )
    .unwrap();

    // 10. goals_1.sqlite with an active goal
    let goals_db = codex_home.join("goals_1.sqlite");
    let g_conn = Connection::open(&goals_db).unwrap();
    g_conn
        .execute_batch(
            "CREATE TABLE thread_goals (thread_id TEXT PRIMARY KEY NOT NULL, goal_id TEXT NOT NULL, objective TEXT NOT NULL, status TEXT NOT NULL, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL);
             INSERT INTO thread_goals VALUES ('thread-101', 'g-1', 'Run overnight refactor', 'active', 1700000000000, 1700000000000);",
        )
        .unwrap();

    (codex_home, proj_root)
}

#[test]
fn test_end_to_end_migration_merge_fork_and_failure_rollback() {
    std::env::set_var("CODEX_MOVE_ALLOW_RUNNING_CODEX", "1");
    let sandbox = tempdir().unwrap();
    let (src_codex, _src_proj) = build_realistic_source_fixture(sandbox.path());

    // Prepare Target Machine with existing Codex data, target auth.json, and a divergent "thread-shared-fork"
    let dst_codex = sandbox.path().join("target_user").join(".codex");
    let dst_projects_root = sandbox.path().join("target_user").join("Workspace");
    std::fs::create_dir_all(dst_codex.join("sessions/2026/10/29")).unwrap();
    std::fs::create_dir_all(&dst_projects_root).unwrap();

    // Target has its own auth.json that MUST NOT be overwritten
    std::fs::write(
        dst_codex.join("auth.json"),
        r#"{"access_token":"target-machine-preserved-auth"}"#,
    )
    .unwrap();
    std::fs::write(dst_codex.join("config.toml"), "model = \"gpt-5-target\"\n").unwrap();

    let dst_r2 = dst_codex.join("sessions/2026/10/29/rollout-02.jsonl");
    std::fs::write(
        &dst_r2,
        r#"{"timestamp":"2026-10-29T10:00:00Z","ordinal":0,"type":"session_meta","payload":{"session_id":"thread-shared-fork","id":"thread-shared-fork","cwd":"/Users/target/existing","model_provider":"openai"}}"#,
    )
    .unwrap();

    let dst_state_db = dst_codex.join("state_5.sqlite");
    let dst_conn = Connection::open(&dst_state_db).unwrap();
    dst_conn
        .execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, metadata TEXT NOT NULL DEFAULT '{}', position INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL);
             CREATE TABLE project_roots (project_id TEXT NOT NULL, position INTEGER NOT NULL, path TEXT NOT NULL, PRIMARY KEY (project_id, position));
             CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, source TEXT NOT NULL, model_provider TEXT NOT NULL, cwd TEXT NOT NULL, title TEXT NOT NULL, sandbox_policy TEXT NOT NULL, approval_mode TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0, project_id TEXT);",
        )
        .unwrap();
    dst_conn
        .execute(
            "INSERT INTO threads VALUES ('thread-shared-fork', ?1, 1700000000, 1700000999, 'cli', 'openai', '/Users/target/existing', 'Target Divergent Topic', 'workspace-write', 'on-request', 0, NULL)",
            params![dst_r2.to_string_lossy().to_string()],
        )
        .unwrap();
    drop(dst_conn);

    // Step 1: Scan source inventory
    let limits = SecurityLimits::default();
    let scan_opts = ScanOptions {
        codex_home: src_codex.to_string_lossy().to_string(),
        shared_skills_dir: None,
        include_project_files: true,
        excluded_project_dirs: vec!["node_modules".to_string()],
        compute_sha256: true,
    };
    let mut manifest = scan_codex_inventory(&scan_opts, &limits).unwrap();

    // Verify node_modules was excluded while untracked_notes.md was included
    assert!(manifest
        .files
        .iter()
        .any(|f| f.logical_rel_path == "untracked_notes.md"));
    assert!(!manifest
        .files
        .iter()
        .any(|f| f.logical_rel_path.contains("node_modules")));

    // Step 2: Create staging snapshot
    let staging_dir = sandbox.path().join("staging");
    create_staging_snapshot(&mut manifest, &staging_dir, &limits).unwrap();
    verify_staging_snapshot(&staging_dir, &limits).unwrap();

    // Step 3: Propose project mappings & build import plan
    let proj_mappings = propose_default_project_mappings(
        &manifest,
        &dst_projects_root.to_string_lossy(),
        PathStyle::current(),
    );
    assert_eq!(proj_mappings.len(), 1);

    let all_cats = vec![
        ContentCategory::ActiveAndArchivedChats,
        ContentCategory::ChatMetadataAndIndex,
        ContentCategory::UserSkills,
        ContentCategory::InstructionFiles,
        ContentCategory::CoreConfig,
        ContentCategory::ExtensionsMemoriesRulesHooks,
        ContentCategory::ProjectFiles,
    ];

    let plan = build_import_plan(
        &manifest,
        &dst_codex,
        None,
        &all_cats,
        &proj_mappings,
        false, // Exclude .env sensitive files by default
        true,
    )
    .unwrap();
    assert!(plan.can_proceed);
    assert!(plan
        .thread_actions
        .iter()
        .any(|a| a.resolution == ThreadConflictResolution::KeepBothForked));

    // Step 4: Test Fault Injection during Apply -> must automatically roll back target to exact original state!
    let failed_res = apply_migration_from_staging(
        &manifest,
        &staging_dir,
        &plan,
        &limits,
        &FailureInjectionHook {
            fail_after_files: Some(2),
        },
        |_| {},
    )
    .unwrap();
    assert!(matches!(
        failed_res.outcome,
        MigrationOutcomeStatus::FailedAndRolledBack
    ));
    assert!(failed_res.rollback_report.unwrap().fully_restored);

    // Re-create staging since rollback cleaned it up
    create_staging_snapshot(&mut manifest, &staging_dir, &limits).unwrap();

    // Step 5: Execute full migration without fault injection
    let success_res = apply_migration_from_staging(
        &manifest,
        &staging_dir,
        &plan,
        &limits,
        &FailureInjectionHook::default(),
        |_| {},
    )
    .unwrap();
    assert!(matches!(
        success_res.outcome,
        MigrationOutcomeStatus::Succeeded | MigrationOutcomeStatus::PartiallyCompletedWithWarnings
    ));
    assert_eq!(success_res.merged_threads_count, 1);
    assert_eq!(success_res.forked_threads_count, 1);

    // Verify both original target thread AND forked source thread exist in target state_5.sqlite
    let forked_action = plan
        .thread_actions
        .iter()
        .find(|a| a.source_thread_id == "thread-shared-fork")
        .expect("forked action must exist");
    let forked_uuid = forked_action.target_thread_id.clone();
    assert_ne!(forked_uuid, "thread-shared-fork");

    let check_conn = Connection::open(&dst_state_db).unwrap();
    let thread_ids: Vec<String> = check_conn
        .prepare("SELECT id FROM threads ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(thread_ids.contains(&"thread-101".to_string()));
    assert!(thread_ids.contains(&"thread-shared-fork".to_string()));
    assert!(thread_ids.contains(&forked_uuid));
    drop(check_conn);

    // Verify forked rollout .jsonl file has session_meta.payload.session_id == forked_uuid
    let forked_rollout_raw = std::fs::read_to_string(&forked_action.mapped_rollout_path).unwrap();
    assert!(forked_rollout_raw.contains(&forked_uuid));

    // Verify repeated import of the exact same snapshot deduplicates cleanly without creating duplicate forks!
    let repeat_plan = build_import_plan(
        &manifest,
        &dst_codex,
        None,
        &all_cats,
        &proj_mappings,
        false,
        true,
    )
    .unwrap();
    assert!(repeat_plan
        .thread_actions
        .iter()
        .all(|a| a.resolution == ThreadConflictResolution::DeduplicateIdentical));

    // Verify target auth.json was untouched
    let target_auth = std::fs::read_to_string(dst_codex.join("auth.json")).unwrap();
    assert!(target_auth.contains("target-machine-preserved-auth"));

    // Verify automation was imported in paused status
    let imported_auto =
        std::fs::read_to_string(dst_codex.join("automations/daily/automation.toml")).unwrap();
    assert!(imported_auto.contains("status = \"paused\""));

    // Verify goals_1.sqlite active goal was paused
    let g_conn = Connection::open(dst_codex.join("goals_1.sqlite")).unwrap();
    let goal_status: String = g_conn
        .query_row(
            "SELECT status FROM thread_goals WHERE thread_id = 'thread-101'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(goal_status, "paused");
    drop(g_conn);

    // Verify automated verification checks pass
    let ver = verify_imported_codex_environment(&manifest, &plan).unwrap();
    assert!(ver.all_automated_checks_passed);
    assert!(ver.residual_old_paths_found.is_empty());

    // Step 6: Verify user-initiated post-completion Rollback restores target cleanly
    let mut journals = discover_recovery_journals(&dst_codex);
    assert!(!journals.is_empty());
    let rb = execute_rollback(&mut journals[0]).unwrap();
    assert!(rb.fully_restored);
}
