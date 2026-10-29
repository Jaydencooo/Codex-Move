use crate::errors::{AppError, AppResult, ErrorCode};
use crate::paths::{rewrite_path_with_rules, PathMappingRule, PathStyle};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqliteSchemaReport {
    pub state_db_filename: Option<String>,
    pub state_schema_version: Option<u32>,
    pub state_migration_version: Option<i64>,
    pub thread_history_db_filename: Option<String>,
    pub thread_history_migration_version: Option<i64>,
    pub memories_db_filename: Option<String>,
    pub goals_db_filename: Option<String>,
    pub has_active_wal: bool,
    pub unknown_state_databases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    pub adapter_id: String,
    pub can_read_threads: bool,
    pub can_write_threads: bool,
    pub can_map_paths: bool,
    pub supported_state_versions: Vec<u32>,
    pub unknown_tables: Vec<String>,
    pub compatibility_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexThreadSummary {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub rollout_path: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived: bool,
    pub project_id: Option<String>,
    pub model_provider: String,
    #[serde(default)]
    pub content_fingerprint: Option<String>,
    #[serde(default)]
    pub record_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexProjectSummary {
    pub id: String,
    pub name: String,
    pub root_paths: Vec<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Discovers `state_*.sqlite` files dynamically without hardcoding a single `state_5.sqlite` name.
/// Supports both `codex_home/state_N.sqlite` and custom `sqlite_home` configured in `config.toml`.
pub fn inspect_codex_databases(
    codex_home: &Path,
) -> AppResult<(SqliteSchemaReport, AdapterCapabilities)> {
    let mut state_candidates: Vec<(u32, String, PathBuf)> = Vec::new();
    let mut unknown_state_dbs = Vec::new();
    let mut has_active_wal = false;

    let mut search_dirs = vec![codex_home.to_path_buf()];
    let sqlite_subdir = codex_home.join("sqlite");
    if sqlite_subdir.is_dir() {
        search_dirs.push(sqlite_subdir);
    }

    // Check if config.toml overrides `sqlite_home`
    let cfg_path = codex_home.join("config.toml");
    if cfg_path.is_file() {
        if let Ok(raw_cfg) = std::fs::read_to_string(&cfg_path) {
            if let Ok(doc) = raw_cfg.parse::<toml_edit::DocumentMut>() {
                if let Some(sh) = doc.get("sqlite_home").and_then(|v| v.as_str()) {
                    let custom_p = PathBuf::from(sh);
                    if custom_p.is_dir() && !search_dirs.contains(&custom_p) {
                        search_dirs.push(custom_p);
                    }
                }
            }
        }
    }

    let mut thread_history_db: Option<(String, PathBuf)> = None;
    let mut memories_db: Option<String> = None;
    let mut goals_db: Option<String> = None;

    for dir in &search_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            let fpath = entry.path();

            if fname.ends_with(".sqlite-wal") {
                if let Ok(meta) = entry.metadata() {
                    if meta.len() > 0 {
                        has_active_wal = true;
                    }
                }
            }

            if let Some(num_str) = fname
                .strip_prefix("state_")
                .and_then(|s| s.strip_suffix(".sqlite"))
            {
                if let Ok(ver) = num_str.parse::<u32>() {
                    if ver == 4 || ver == 5 {
                        state_candidates.push((ver, fname, fpath));
                    } else {
                        unknown_state_dbs.push(fname);
                    }
                }
            } else if let Some(num_str) = fname
                .strip_prefix("thread_history_")
                .and_then(|s| s.strip_suffix(".sqlite"))
            {
                if num_str == "1" {
                    thread_history_db = Some((fname, fpath));
                } else {
                    unknown_state_dbs.push(fname);
                }
            } else if let Some(num_str) = fname
                .strip_prefix("memories_")
                .and_then(|s| s.strip_suffix(".sqlite"))
            {
                if num_str == "1" {
                    memories_db = Some(fname);
                } else {
                    unknown_state_dbs.push(fname);
                }
            } else if let Some(num_str) = fname
                .strip_prefix("goals_")
                .and_then(|s| s.strip_suffix(".sqlite"))
            {
                if num_str == "1" {
                    goals_db = Some(fname);
                } else {
                    unknown_state_dbs.push(fname);
                }
            }
        }
    }

    state_candidates.sort_by(|a, b| b.0.cmp(&a.0));
    let selected_state = state_candidates.into_iter().next();

    let mut state_migration_version = None;
    let mut unknown_tables = Vec::new();
    let mut can_read_threads = false;
    let mut can_write_threads = false;
    let mut compatibility_notes = Vec::new();

    let known_tables: HashSet<&str> = [
        "_sqlx_migrations",
        "sqlite_sequence",
        "threads",
        "thread_dynamic_tools",
        "backfill_state",
        "thread_spawn_edges",
        "remote_control_enrollments",
        "external_agent_config_imports",
        "thread_sections",
        "rollout_migration_state",
        "rollout_migration_skipped_rollouts",
        "projects",
        "project_roots",
        "project_idempotency_keys",
        "thread_attachments",
        "guardian_review_feedback",
        "thread_read_receipts",
    ]
    .into_iter()
    .collect();

    if let Some((ver, ref fname, ref db_path)) = selected_state {
        let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        state_migration_version = query_max_migration_version(&conn);
        let dirty_migrations = query_dirty_migration_count(&conn);
        let integrity_ok = conn
            .query_row("PRAGMA quick_check(1)", [], |r| r.get::<_, String>(0))
            .map(|s| s.eq_ignore_ascii_case("ok"))
            .unwrap_or(false);

        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
        let table_names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();

        for t in &table_names {
            if !known_tables.contains(t.as_str()) {
                unknown_tables.push(t.clone());
            }
        }

        let migration_supported = state_migration_version
            .map(|mv| (1..=45).contains(&mv))
            .unwrap_or(true);

        if !integrity_ok {
            compatibility_notes.push(format!(
                "{fname} failed PRAGMA quick_check integrity verification; direct SQL write disabled."
            ));
        }
        if dirty_migrations > 0 {
            compatibility_notes.push(format!(
                "{fname} contains {dirty_migrations} incomplete/failed migration(s) in `_sqlx_migrations`."
            ));
        }
        if !migration_supported {
            compatibility_notes.push(format!(
                "{fname} migration version {:?} exceeds supported range (1..=45); direct SQL write disabled.",
                state_migration_version
            ));
        }
        if !unknown_tables.is_empty() {
            compatibility_notes.push(format!(
                "{fname} contains unrecognized table(s): {:?}; direct SQL write disabled to prevent data loss.",
                unknown_tables
            ));
        }

        if table_names.iter().any(|t| t == "threads") {
            let cols = query_table_columns(&conn, "threads")?;
            let required = [
                "id",
                "rollout_path",
                "cwd",
                "title",
                "created_at",
                "updated_at",
            ];
            if required.iter().all(|req| cols.contains(*req)) {
                can_read_threads = integrity_ok;
                can_write_threads = integrity_ok
                    && unknown_state_dbs.is_empty()
                    && unknown_tables.is_empty()
                    && dirty_migrations == 0
                    && migration_supported;
            } else {
                compatibility_notes.push(format!(
                    "{fname} is missing required columns in `threads` table."
                ));
            }
        }
        compatibility_notes.push(format!(
            "Detected state database {} (schema v{}, migration {:?})",
            fname, ver, state_migration_version
        ));
    } else if !unknown_state_dbs.is_empty() {
        compatibility_notes.push(format!(
            "Found unsupported state database(s): {:?}. Read-only raw backup available; direct SQL write disabled.",
            unknown_state_dbs
        ));
    } else {
        // No state DB yet (e.g. CLI-only or clean installation): adapter can still read/write JSONL and create/merge state_5
        can_read_threads = true;
        can_write_threads = true;
        compatibility_notes
            .push("No existing state_*.sqlite found; JSONL session mode active.".to_string());
    }

    let thread_history_migration_version = if let Some((ref th_fname, ref th_path)) =
        thread_history_db
    {
        if let Ok(conn) = Connection::open_with_flags(th_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
            let dirty = query_dirty_migration_count(&conn);
            let mv = query_max_migration_version(&conn);
            if dirty > 0 || mv.map(|v| !(1..=15).contains(&v)).unwrap_or(false) {
                can_write_threads = false;
                compatibility_notes.push(format!(
                    "Auxiliary database {th_fname} has unsupported or dirty migration version ({mv:?}, dirty={dirty})."
                ));
            }
            mv
        } else {
            None
        }
    } else {
        None
    };

    let report = SqliteSchemaReport {
        state_db_filename: selected_state.as_ref().map(|(_, f, _)| f.clone()),
        state_schema_version: selected_state.as_ref().map(|(v, _, _)| *v),
        state_migration_version,
        thread_history_db_filename: thread_history_db.map(|(f, _)| f),
        thread_history_migration_version,
        memories_db_filename: memories_db,
        goals_db_filename: goals_db,
        has_active_wal,
        unknown_state_databases: unknown_state_dbs,
    };

    let capabilities = AdapterCapabilities {
        adapter_id: "codex-adapter-v1 (state_4/state_5 + rollout-jsonl)".to_string(),
        can_read_threads,
        can_write_threads,
        can_map_paths: can_write_threads,
        supported_state_versions: vec![4, 5],
        unknown_tables,
        compatibility_notes,
    };

    Ok((report, capabilities))
}

fn query_max_migration_version(conn: &Connection) -> Option<i64> {
    conn.query_row(
        "SELECT MAX(version) FROM _sqlx_migrations WHERE success = 1",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
}

fn query_dirty_migration_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 0",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

pub fn query_table_columns(conn: &Connection, table_name: &str) -> AppResult<HashSet<String>> {
    // Only allow alphanumeric and underscore in table_name
    if !table_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(AppError::new(
            ErrorCode::UnsupportedDatabaseSchema,
            "Invalid SQLite table identifier.",
        ));
    }
    let pragma_sql = format!("PRAGMA table_info(\"{table_name}\")");
    let mut stmt = conn.prepare(&pragma_sql)?;
    let cols = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(cols)
}

/// Reads threads and projects from `state_*.sqlite` and/or `.codex-global-state.json` and `sessions/`.
pub fn read_threads_and_projects(
    codex_home: &Path,
    schema_report: &SqliteSchemaReport,
) -> AppResult<(Vec<CodexThreadSummary>, Vec<CodexProjectSummary>)> {
    let mut threads = Vec::new();
    let mut projects_map: HashMap<String, CodexProjectSummary> = HashMap::new();

    if let Some(ref db_name) = schema_report.state_db_filename {
        let db_path = resolve_db_path(codex_home, db_name);
        if db_path.is_file() {
            let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let cols = query_table_columns(&conn, "threads")?;
            let has_project_id = cols.contains("project_id");

            let sql = if has_project_id {
                "SELECT id, title, cwd, rollout_path, created_at, updated_at, archived, project_id, model_provider FROM threads ORDER BY updated_at DESC"
            } else {
                "SELECT id, title, cwd, rollout_path, created_at, updated_at, archived, NULL as project_id, model_provider FROM threads ORDER BY updated_at DESC"
            };

            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |row| {
                let rollout_path: String = row.get(3)?;
                let (content_fingerprint, record_count) =
                    compute_rollout_transcript_fingerprint(codex_home, &rollout_path);
                Ok(CodexThreadSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    cwd: row.get(2)?,
                    rollout_path,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                    archived: row.get::<_, i64>(6).unwrap_or(0) != 0,
                    project_id: row.get(7).ok().flatten(),
                    model_provider: row.get(8).unwrap_or_else(|_| "openai".to_string()),
                    content_fingerprint,
                    record_count,
                })
            })?;

            for r in rows {
                threads.push(r?);
            }

            // Read projects table if present
            let all_tables: HashSet<String> = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
                .query_map([], |r| r.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();

            if all_tables.contains("projects") && all_tables.contains("project_roots") {
                let mut p_stmt = conn.prepare(
                    "SELECT id, name, created_at_ms, updated_at_ms FROM projects ORDER BY position ASC",
                )?;
                let p_rows = p_stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })?;

                for pr in p_rows {
                    let (id, name, created_at_ms, updated_at_ms) = pr?;
                    let mut r_stmt = conn.prepare(
                        "SELECT path FROM project_roots WHERE project_id = ?1 ORDER BY position ASC",
                    )?;
                    let roots: Vec<String> = r_stmt
                        .query_map(params![&id], |r| r.get::<_, String>(0))?
                        .filter_map(|r| r.ok())
                        .collect();

                    projects_map.insert(
                        id.clone(),
                        CodexProjectSummary {
                            id,
                            name,
                            root_paths: roots,
                            created_at_ms,
                            updated_at_ms,
                        },
                    );
                }
            }
        }
    }

    // Also merge projects from `.codex-global-state.json` (`local-projects`)
    let gs_path = codex_home.join(".codex-global-state.json");
    if gs_path.is_file() {
        if let Ok(raw) = std::fs::read_to_string(&gs_path) {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                if let Some(lp) = val.get("local-projects").and_then(|v| v.as_object()) {
                    for (pid, p_obj) in lp {
                        let name = p_obj
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Project")
                            .to_string();
                        let roots: Vec<String> = p_obj
                            .get("rootPaths")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let created_at_ms =
                            p_obj.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0);
                        let updated_at_ms =
                            p_obj.get("updatedAt").and_then(|v| v.as_i64()).unwrap_or(0);

                        projects_map
                            .entry(pid.clone())
                            .and_modify(|existing| {
                                for r in &roots {
                                    if !existing.root_paths.contains(r) {
                                        existing.root_paths.push(r.clone());
                                    }
                                }
                            })
                            .or_insert(CodexProjectSummary {
                                id: pid.clone(),
                                name,
                                root_paths: roots,
                                created_at_ms,
                                updated_at_ms,
                            });
                    }
                }
            }
        }
    }

    // Also discover any project paths declared in `config.toml` `[projects."..."]`
    let cfg_path = codex_home.join("config.toml");
    if cfg_path.is_file() {
        if let Ok(raw_cfg) = std::fs::read_to_string(&cfg_path) {
            if let Ok(doc) = raw_cfg.parse::<toml_edit::DocumentMut>() {
                if let Some(proj_table) = doc.get("projects").and_then(|i| i.as_table()) {
                    for (path_key, _) in proj_table.iter() {
                        let already_known = projects_map.values().any(|p| {
                            p.root_paths
                                .iter()
                                .any(|r| r.eq_ignore_ascii_case(path_key))
                        });
                        if !already_known {
                            let synth_id = synthetic_project_id_for_path(path_key);
                            let folder_name = Path::new(&path_key.replace('\\', "/"))
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| path_key.to_string());
                            projects_map.insert(
                                synth_id.clone(),
                                CodexProjectSummary {
                                    id: synth_id,
                                    name: folder_name,
                                    root_paths: vec![path_key.to_string()],
                                    created_at_ms: 0,
                                    updated_at_ms: 0,
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    let mut projects: Vec<CodexProjectSummary> = projects_map.into_values().collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name));

    Ok((threads, projects))
}

pub fn resolve_db_path(codex_home: &Path, db_name: &str) -> PathBuf {
    let direct = codex_home.join(db_name);
    if direct.exists() {
        return direct;
    }
    let in_sqlite = codex_home.join("sqlite").join(db_name);
    if in_sqlite.exists() {
        return in_sqlite;
    }
    direct
}

fn synthetic_project_id_for_path(path_str: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path_str.to_lowercase().as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// Computes a deterministic SHA-256 content fingerprint and record count for a rollout `.jsonl` file,
/// normalizing out machine-specific paths (`cwd`, `runtime_workspace_roots`, `workspace_roots`) and
/// session IDs (`session_id`, `id`, `thread_id`) so identical transcripts across OSes or forked copies
/// can be verified by actual conversation content rather than title/timestamp heuristics.
pub fn compute_rollout_transcript_fingerprint(
    codex_home: &Path,
    rollout_path_str: &str,
) -> (Option<String>, usize) {
    let resolved = resolve_rollout_file_on_disk(codex_home, rollout_path_str);
    let Some(path) = resolved else {
        return (None, 0);
    };
    let Ok(file) = File::open(&path) else {
        return (None, 0);
    };
    let reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut record_count = 0usize;

    for line_res in reader.lines() {
        let Ok(raw_line) = line_res else {
            return (None, 0);
        };
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(mut val) = serde_json::from_str::<Value>(trimmed) else {
            return (None, 0);
        };
        if let Some(top_obj) = val.as_object_mut() {
            top_obj.remove("thread_id");
            top_obj.remove("session_id");
            if let Some(payload) = top_obj.get_mut("payload").and_then(|p| p.as_object_mut()) {
                payload.remove("cwd");
                payload.remove("runtime_workspace_roots");
                payload.remove("workspace_roots");
                payload.remove("session_id");
                payload.remove("id");
                payload.remove("thread_id");
            }
        }
        if let Ok(canonical) = serde_json::to_string(&val) {
            hasher.update(canonical.as_bytes());
            hasher.update(b"\n");
            record_count += 1;
        }
    }

    if record_count == 0 {
        (None, 0)
    } else {
        (Some(hex::encode(hasher.finalize())), record_count)
    }
}

fn resolve_rollout_file_on_disk(codex_home: &Path, rollout_path_str: &str) -> Option<PathBuf> {
    if rollout_path_str.trim().is_empty() {
        return None;
    }
    let direct = PathBuf::from(rollout_path_str);
    if direct.is_file() {
        return Some(direct);
    }
    let norm = rollout_path_str.replace('\\', "/");
    for sub in ["sessions/", "archived_sessions/"] {
        if let Some(idx) = norm.find(sub) {
            let rel = &norm[idx..];
            let candidate = codex_home.join(rel);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let rel_direct = codex_home.join(rollout_path_str);
    if rel_direct.is_file() {
        return Some(rel_direct);
    }
    None
}

/// Validates and transforms a rollout `.jsonl` file line-by-line using streaming I/O.
/// - Preserves chat messages, commands, and tool outputs verbatim!
/// - Updates `session_meta.payload.cwd`, `session_meta.payload.runtime_workspace_roots`,
///   and `turn_context.cwd` / `turn_context.workspace_roots` according to structured `rules`.
/// - When `new_thread_id` is `Some(new_id)` (e.g., for forked chats), updates structured session
///   identity fields (`session_meta.payload.session_id`, `session_meta.payload.id`, and record
///   `thread_id` / `session_id`) so the rollout file, index, and SQLite database share a unified ID.
pub fn transform_rollout_jsonl_file(
    src_path: &Path,
    dst_path: &Path,
    rules: &[PathMappingRule],
    target_style: PathStyle,
    new_thread_id: Option<&str>,
) -> AppResult<usize> {
    let in_file = File::open(src_path)?;
    let reader = BufReader::new(in_file);

    if let Some(parent) = dst_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out_file = File::create(dst_path)?;
    let mut writer = BufWriter::new(out_file);

    let mut line_count = 0usize;

    for (idx, line_res) in reader.lines().enumerate() {
        let raw_line = line_res?;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut parsed: Value = serde_json::from_str(trimmed).map_err(|e| {
            AppError::with_detail(
                ErrorCode::CorruptedSessionJsonl,
                format!(
                    "Malformed JSONL at line {} in {}",
                    idx + 1,
                    src_path.display()
                ),
                e.to_string(),
                true,
            )
        })?;

        let record_type = parsed
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if idx == 0 && record_type != "session_meta" {
            return Err(AppError::with_detail(
                ErrorCode::CorruptedSessionJsonl,
                format!(
                    "Rollout file {} does not start with a session_meta record.",
                    src_path.display()
                ),
                format!("Found record type: {record_type}"),
                true,
            ));
        }

        if let Some(new_id) = new_thread_id {
            if let Some(top_obj) = parsed.as_object_mut() {
                if top_obj.contains_key("thread_id") {
                    top_obj.insert("thread_id".to_string(), Value::String(new_id.to_string()));
                }
                if top_obj.contains_key("session_id") {
                    top_obj.insert("session_id".to_string(), Value::String(new_id.to_string()));
                }
            }
        }

        if record_type == "session_meta" {
            if let Some(payload) = parsed.get_mut("payload").and_then(|v| v.as_object_mut()) {
                rewrite_json_path_field(payload, "cwd", rules, target_style);
                rewrite_json_path_array_field(
                    payload,
                    "runtime_workspace_roots",
                    rules,
                    target_style,
                );
                if let Some(new_id) = new_thread_id {
                    if payload.contains_key("session_id") {
                        payload.insert("session_id".to_string(), Value::String(new_id.to_string()));
                    }
                    if payload.contains_key("id") {
                        payload.insert("id".to_string(), Value::String(new_id.to_string()));
                    }
                    if payload.contains_key("thread_id") {
                        payload.insert("thread_id".to_string(), Value::String(new_id.to_string()));
                    }
                }
            }
        } else if record_type == "turn_context" {
            if let Some(payload) = parsed.get_mut("payload").and_then(|v| v.as_object_mut()) {
                rewrite_json_path_field(payload, "cwd", rules, target_style);
                rewrite_json_path_array_field(payload, "workspace_roots", rules, target_style);
                if let Some(new_id) = new_thread_id {
                    if payload.contains_key("thread_id") {
                        payload.insert("thread_id".to_string(), Value::String(new_id.to_string()));
                    }
                    if payload.contains_key("session_id") {
                        payload.insert("session_id".to_string(), Value::String(new_id.to_string()));
                    }
                }
            }
        } else if let Some(new_id) = new_thread_id {
            if let Some(payload) = parsed.get_mut("payload").and_then(|v| v.as_object_mut()) {
                if payload.contains_key("thread_id") {
                    payload.insert("thread_id".to_string(), Value::String(new_id.to_string()));
                }
                if payload.contains_key("session_id") {
                    payload.insert("session_id".to_string(), Value::String(new_id.to_string()));
                }
            }
        }

        let serialized = serde_json::to_string(&parsed)?;
        writer.write_all(serialized.as_bytes())?;
        writer.write_all(b"\n")?;
        line_count += 1;
    }

    if line_count == 0 {
        return Err(AppError::new(
            ErrorCode::CorruptedSessionJsonl,
            format!("Rollout file {} is empty.", src_path.display()),
        ));
    }

    writer.flush()?;
    Ok(line_count)
}

fn rewrite_json_path_field(
    obj: &mut serde_json::Map<String, Value>,
    field: &str,
    rules: &[PathMappingRule],
    target_style: PathStyle,
) {
    if let Some(Value::String(cur)) = obj.get(field) {
        if let Some(mapped) = rewrite_path_with_rules(cur, rules, target_style) {
            obj.insert(field.to_string(), Value::String(mapped));
        }
    }
}

fn rewrite_json_path_array_field(
    obj: &mut serde_json::Map<String, Value>,
    field: &str,
    rules: &[PathMappingRule],
    target_style: PathStyle,
) {
    if let Some(Value::Array(arr)) = obj.get_mut(field) {
        for item in arr.iter_mut() {
            if let Value::String(cur) = item {
                if let Some(mapped) = rewrite_path_with_rules(cur, rules, target_style) {
                    *item = Value::String(mapped);
                }
            }
        }
    }
}

/// Transforms `.codex-global-state.json` safely:
/// - Rewrites `local-projects[*].rootPaths`, `thread-writable-roots`, `thread-workspace-root-hints`,
///   `thread-projectless-output-directories`, and host-prefixed keys (`local:...`).
/// - Remaps forked thread IDs across `thread-writable-roots`, `thread-workspace-root-hints`,
///   `thread-projectless-output-directories`, `thread-project-assignments`, `sidebar-project-thread-orders`,
///   `pinned-thread-ids`, and `app-server-migrated-pinned-thread-ids-by-host`.
/// - Strips machine-bound window coordinates and installation IDs.
/// - Merges with existing target `.codex-global-state.json` if present.
pub fn merge_and_transform_global_state(
    source_gs: &Value,
    target_existing_gs: Option<&Value>,
    rules: &[PathMappingRule],
    target_style: PathStyle,
    forked_thread_id_map: &HashMap<String, String>,
) -> Value {
    let mut base = target_existing_gs
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let Some(src_obj) = source_gs.as_object() else {
        return Value::Object(base);
    };

    let remap_tid = |tid: &str| -> String {
        forked_thread_id_map
            .get(tid)
            .cloned()
            .unwrap_or_else(|| tid.to_string())
    };

    // Device-specific ephemeral keys to exclude from overwriting target
    let excluded_keys: HashSet<&str> = [
        "electron-local-remote-control-installation-id",
        "electron-main-window-bounds",
        "electron-avatar-overlay-bounds",
        "electron-avatar-overlay-open",
        "electron-windows-core-runtime-frameworks-enabled",
        "electron-windows-primary-runtime-frameworks-enabled",
    ]
    .into_iter()
    .collect();

    for (k, v) in src_obj {
        if excluded_keys.contains(k.as_str()) {
            continue;
        }
        match k.as_str() {
            "local-projects" => {
                let mut target_lp = base
                    .get("local-projects")
                    .and_then(|x| x.as_object().cloned())
                    .unwrap_or_default();
                if let Some(src_lp) = v.as_object() {
                    for (pid, pval) in src_lp {
                        let mut p_clone = pval.clone();
                        if let Some(p_map) = p_clone.as_object_mut() {
                            rewrite_json_path_array_field(p_map, "rootPaths", rules, target_style);
                        }
                        target_lp.entry(pid.clone()).or_insert(p_clone);
                    }
                }
                base.insert("local-projects".to_string(), Value::Object(target_lp));
            }
            "thread-writable-roots" => {
                let mut target_twr = base
                    .get("thread-writable-roots")
                    .and_then(|x| x.as_object().cloned())
                    .unwrap_or_default();
                if let Some(src_twr) = v.as_object() {
                    for (tid, roots_val) in src_twr {
                        let mut cloned = roots_val.clone();
                        if let Value::Array(ref mut arr) = cloned {
                            for item in arr.iter_mut() {
                                if let Value::String(s) = item {
                                    if let Some(mapped) =
                                        rewrite_path_with_rules(s, rules, target_style)
                                    {
                                        *item = Value::String(mapped);
                                    }
                                }
                            }
                        }
                        target_twr.entry(remap_tid(tid)).or_insert(cloned);
                    }
                }
                base.insert(
                    "thread-writable-roots".to_string(),
                    Value::Object(target_twr),
                );
            }
            "thread-workspace-root-hints" | "thread-projectless-output-directories" => {
                let mut target_map = base
                    .get(k)
                    .and_then(|x| x.as_object().cloned())
                    .unwrap_or_default();
                if let Some(src_map) = v.as_object() {
                    for (tid, path_val) in src_map {
                        let mapped_val = if let Some(s) = path_val.as_str() {
                            Value::String(
                                rewrite_path_with_rules(s, rules, target_style)
                                    .unwrap_or_else(|| s.to_string()),
                            )
                        } else {
                            path_val.clone()
                        };
                        target_map.entry(remap_tid(tid)).or_insert(mapped_val);
                    }
                }
                base.insert(k.clone(), Value::Object(target_map));
            }
            "thread-project-assignments" => {
                let mut target_map = base
                    .get(k)
                    .and_then(|x| x.as_object().cloned())
                    .unwrap_or_default();
                if let Some(src_map) = v.as_object() {
                    for (tid, val_item) in src_map {
                        target_map
                            .entry(remap_tid(tid))
                            .or_insert_with(|| val_item.clone());
                    }
                }
                base.insert(k.clone(), Value::Object(target_map));
            }
            "sidebar-project-thread-orders" => {
                let mut target_map = base
                    .get(k)
                    .and_then(|x| x.as_object().cloned())
                    .unwrap_or_default();
                if let Some(src_map) = v.as_object() {
                    for (pid_key, val_item) in src_map {
                        let remapped_item = remap_thread_ids_in_order_value(val_item, &remap_tid);
                        if let Some(existing_item) = target_map.get_mut(pid_key) {
                            merge_thread_order_values(existing_item, &remapped_item);
                        } else {
                            target_map.insert(pid_key.clone(), remapped_item);
                        }
                    }
                }
                base.insert(k.clone(), Value::Object(target_map));
            }
            "pinned-thread-ids" => {
                let mut target_arr = base
                    .get(k)
                    .and_then(|x| x.as_array().cloned())
                    .unwrap_or_default();
                if let Some(src_arr) = v.as_array() {
                    for item in src_arr {
                        if let Some(tid) = item.as_str() {
                            let mapped_tid = Value::String(remap_tid(tid));
                            if !target_arr.contains(&mapped_tid) {
                                target_arr.push(mapped_tid);
                            }
                        }
                    }
                }
                base.insert(k.clone(), Value::Array(target_arr));
            }
            "app-server-project-id-by-legacy-project-id-by-host"
            | "app-server-projects-migration-by-host"
            | "app-server-migrated-pinned-thread-ids-by-host" => {
                let mut target_map = base
                    .get(k)
                    .and_then(|x| x.as_object().cloned())
                    .unwrap_or_default();
                if let Some(src_map) = v.as_object() {
                    for (host_key, host_val) in src_map {
                        let new_host_key = rewrite_path_with_rules(host_key, rules, target_style)
                            .unwrap_or_else(|| host_key.clone());
                        let remapped_val = if k == "app-server-migrated-pinned-thread-ids-by-host" {
                            remap_thread_ids_in_order_value(host_val, &remap_tid)
                        } else {
                            host_val.clone()
                        };
                        target_map.entry(new_host_key).or_insert(remapped_val);
                    }
                }
                base.insert(k.clone(), Value::Object(target_map));
            }
            _ => {
                base.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }

    Value::Object(base)
}

fn remap_thread_ids_in_order_value<F>(val: &Value, remap_tid: &F) -> Value
where
    F: Fn(&str) -> String,
{
    match val {
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|x| {
                    if let Some(s) = x.as_str() {
                        Value::String(remap_tid(s))
                    } else {
                        x.clone()
                    }
                })
                .collect(),
        ),
        Value::Object(obj) => {
            let mut cloned = obj.clone();
            if let Some(Value::Array(arr)) = cloned.get_mut("threadIds") {
                for item in arr.iter_mut() {
                    if let Value::String(s) = item {
                        *item = Value::String(remap_tid(s));
                    }
                }
            }
            Value::Object(cloned)
        }
        other => other.clone(),
    }
}

fn merge_thread_order_values(existing: &mut Value, incoming: &Value) {
    match (existing, incoming) {
        (Value::Array(dst_arr), Value::Array(src_arr)) => {
            for item in src_arr {
                if !dst_arr.contains(item) {
                    dst_arr.push(item.clone());
                }
            }
        }
        (Value::Object(dst_obj), Value::Object(src_obj)) => {
            if let (Some(Value::Array(dst_arr)), Some(Value::Array(src_arr))) =
                (dst_obj.get_mut("threadIds"), src_obj.get("threadIds"))
            {
                for item in src_arr {
                    if !dst_arr.contains(item) {
                        dst_arr.push(item.clone());
                    }
                }
            }
        }
        _ => {}
    }
}

/// Rewrites and merges `config.toml` while:
/// - Preserving target-specific `sqlite_home` or local credentials if present
/// - Mapping `[projects."<old_path>"]` table keys to new target paths
/// - Stripping or flagging secret env vars if user opted out of sensitive config
pub fn merge_and_transform_config_toml(
    source_toml: &str,
    target_existing_toml: Option<&str>,
    rules: &[PathMappingRule],
    target_style: PathStyle,
    strip_detected_secrets: bool,
) -> AppResult<(String, Vec<String>)> {
    let src_doc = source_toml.parse::<toml_edit::DocumentMut>()?;
    let mut out_doc = if let Some(existing) = target_existing_toml {
        existing.parse::<toml_edit::DocumentMut>()?
    } else {
        toml_edit::DocumentMut::new()
    };

    let mut warnings = Vec::new();

    for (key, item) in src_doc.iter() {
        match key {
            "sqlite_home" => {
                if !out_doc.contains_key("sqlite_home") {
                    if let Some(s) = item.as_str() {
                        if let Some(mapped) = rewrite_path_with_rules(s, rules, target_style) {
                            out_doc["sqlite_home"] = toml_edit::value(mapped);
                        }
                    }
                }
            }
            "projects" => {
                if let Some(src_projects) = item.as_table() {
                    if !out_doc.contains_key("projects") {
                        out_doc["projects"] = toml_edit::Item::Table(toml_edit::Table::new());
                    }
                    if let Some(dst_projects) = out_doc["projects"].as_table_mut() {
                        for (old_path_key, proj_cfg) in src_projects.iter() {
                            let new_key =
                                rewrite_path_with_rules(old_path_key, rules, target_style)
                                    .unwrap_or_else(|| old_path_key.to_string());
                            if !dst_projects.contains_key(&new_key) {
                                dst_projects.insert(&new_key, proj_cfg.clone());
                            }
                        }
                    }
                }
            }
            "hooks" => {
                warnings.push(
                    "Hooks configuration migrated in disabled/review-required state. Verify hook commands before enabling."
                        .to_string(),
                );
            }
            "mcp_servers" => {
                if let Some(src_mcp) = item.as_table() {
                    if !out_doc.contains_key("mcp_servers") {
                        out_doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
                    }
                    if let Some(dst_mcp) = out_doc["mcp_servers"].as_table_mut() {
                        for (srv_name, srv_cfg) in src_mcp.iter() {
                            let mut cloned = srv_cfg.clone();
                            if strip_detected_secrets {
                                if let Some(tbl) = cloned.as_table_mut() {
                                    tbl.remove("env");
                                    tbl.remove("bearer_token");
                                    tbl.remove("api_key");
                                }
                            }
                            if !dst_mcp.contains_key(srv_name) {
                                dst_mcp.insert(srv_name, cloned);
                                warnings.push(format!(
                                    "MCP server '{srv_name}' migrated. Verify its command executable exists on this computer."
                                ));
                            }
                        }
                    }
                }
            }
            _ => {
                if !out_doc.contains_key(key) {
                    out_doc.insert(key, item.clone());
                }
            }
        }
    }

    Ok((out_doc.to_string(), warnings))
}

/// Transforms an automation `.toml` file:
/// - Maps `cwds` paths to the target computer
/// - Forces `status = "paused"` so scheduled tasks never auto-execute before user review.
pub fn transform_automation_toml(
    raw_toml: &str,
    rules: &[PathMappingRule],
    target_style: PathStyle,
) -> AppResult<String> {
    let mut doc = raw_toml.parse::<toml_edit::DocumentMut>()?;
    doc["status"] = toml_edit::value("paused");

    if let Some(cwds) = doc.get_mut("cwds").and_then(|v| v.as_array_mut()) {
        for idx in 0..cwds.len() {
            if let Some(s) = cwds.get(idx).and_then(|v| v.as_str()) {
                if let Some(mapped) = rewrite_path_with_rules(s, rules, target_style) {
                    cwds.replace(idx, mapped);
                }
            }
        }
    }

    if let Some(auto_tbl) = doc.get_mut("automation").and_then(|v| v.as_table_mut()) {
        auto_tbl["status"] = toml_edit::value("paused");
        if let Some(cwd_str) = auto_tbl.get("cwd").and_then(|v| v.as_str()) {
            if let Some(mapped) = rewrite_path_with_rules(cwd_str, rules, target_style) {
                auto_tbl["cwd"] = toml_edit::value(mapped);
            }
        }
    }

    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_rollout_jsonl_transformation_preserves_messages_and_maps_cwd() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src_rollout.jsonl");
        let dst = dir.path().join("dst_rollout.jsonl");

        let line1 = r#"{"timestamp":"2026-10-01T00:00:00Z","ordinal":0,"type":"session_meta","payload":{"session_id":"019a","id":"019a","cwd":"C:\\Users\\alice\\repo","runtime_workspace_roots":["C:\\Users\\alice\\repo"],"model_provider":"openai"}}"#;
        let line2 = r#"{"timestamp":"2026-10-01T00:00:01Z","ordinal":1,"type":"response_item","payload":{"type":"message","role":"user","content":"Why is C:\\Users\\alice\\repo\\main.rs failing?"}}"#;
        std::fs::write(&src, format!("{line1}\n{line2}\n")).unwrap();

        let rules = vec![PathMappingRule {
            project_id: Some("p1".to_string()),
            source_prefix: r"C:\Users\alice\repo".to_string(),
            target_prefix: "/Users/bob/code/repo".to_string(),
        }];

        let count = transform_rollout_jsonl_file(
            &src,
            &dst,
            &rules,
            PathStyle::Posix,
            Some("0196ed4c-9999-7000-8000-000000000001"),
        )
        .unwrap();
        assert_eq!(count, 2);

        let out_content = std::fs::read_to_string(&dst).unwrap();
        let out_lines: Vec<&str> = out_content.lines().collect();
        let meta: Value = serde_json::from_str(out_lines[0]).unwrap();
        assert_eq!(meta["payload"]["cwd"], "/Users/bob/code/repo");
        assert_eq!(
            meta["payload"]["runtime_workspace_roots"][0],
            "/Users/bob/code/repo"
        );
        assert_eq!(
            meta["payload"]["session_id"],
            "0196ed4c-9999-7000-8000-000000000001"
        );
        assert_eq!(
            meta["payload"]["id"],
            "0196ed4c-9999-7000-8000-000000000001"
        );

        // Chat message body must NEVER be mutated by path replacement
        let msg: Value = serde_json::from_str(out_lines[1]).unwrap();
        assert_eq!(
            msg["payload"]["content"],
            "Why is C:\\Users\\alice\\repo\\main.rs failing?"
        );
    }

    #[test]
    fn test_unknown_schema_database_blocks_direct_write() {
        let dir = tempdir().unwrap();
        let unknown_db = dir.path().join("state_99.sqlite");
        let conn = Connection::open(&unknown_db).unwrap();
        conn.execute("CREATE TABLE mystery (id TEXT PRIMARY KEY)", [])
            .unwrap();
        drop(conn);

        let (report, caps) = inspect_codex_databases(dir.path()).unwrap();
        assert_eq!(report.unknown_state_databases, vec!["state_99.sqlite"]);
        assert!(!caps.can_write_threads);
    }

    #[test]
    fn test_automation_forced_to_paused() {
        let raw = r#"
version = 1
id = "daily-audit"
status = "active"
cwds = ["C:\\Users\\alice\\repo"]
"#;
        let rules = vec![PathMappingRule {
            project_id: None,
            source_prefix: r"C:\Users\alice\repo".to_string(),
            target_prefix: "/Users/bob/repo".to_string(),
        }];
        let transformed = transform_automation_toml(raw, &rules, PathStyle::Posix).unwrap();
        assert!(transformed.contains("status = \"paused\""));
        assert!(transformed.contains("/Users/bob/repo"));
    }
}
