use crate::adapters::codex::{
    inspect_codex_databases, read_threads_and_projects, AdapterCapabilities, CodexProjectSummary,
    CodexThreadSummary, SqliteSchemaReport,
};
use crate::config::{
    default_project_excluded_dirs, SecurityLimits, APP_VERSION, CURRENT_FORMAT_VERSION,
};
use crate::errors::{AppError, AppResult, ErrorCode};
use crate::paths::{classify_symlink, SymlinkClassification};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContentCategory {
    ActiveAndArchivedChats,
    ChatMetadataAndIndex,
    UserSkills,
    SharedAgentSkills,
    InstructionFiles,
    CoreConfig,
    ExtensionsMemoriesRulesHooks,
    ChatAttachments,
    ProjectFiles,
    PluginManifests,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationDisposition {
    DirectCopy,
    NeedsPathMapping,
    NeedsReinstallOnTarget,
    NeedsReauthOnTarget,
    UnsupportedInCurrentVersion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GitRepoStatus {
    NoGit,
    StandardRepo,
    ExternalWorktree { gitdir_pointer: String },
    HasSubmodules { count: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensitiveFinding {
    pub relative_path: String,
    pub finding_kind: String,
    pub description: String,
    pub selected_by_user: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDependencyWarning {
    pub skill_name: String,
    pub relative_path: String,
    pub warning_kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInventoryEntry {
    pub project_id: String,
    pub name: String,
    pub source_root_paths: Vec<String>,
    pub exists_on_disk: bool,
    pub include_project_files: bool,
    pub git_status: GitRepoStatus,
    pub has_uncommitted_changes: bool,
    pub has_untracked_files: bool,
    pub total_files: usize,
    pub total_bytes: u64,
    pub sensitive_findings: Vec<SensitiveFinding>,
    pub nested_under_project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortablePermissions {
    pub readonly: bool,
    pub unix_mode: Option<u32>,
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFileEntry {
    pub entry_id: String,
    pub category: ContentCategory,
    pub archive_rel_path: String,
    pub logical_rel_path: String,
    pub source_abs_path: String,
    pub project_id: Option<String>,
    pub size_bytes: u64,
    pub modified_unix_secs: u64,
    pub sha256: String,
    pub permissions: PortablePermissions,
    pub symlink: Option<SymlinkClassification>,
    pub disposition: MigrationDisposition,
    pub is_sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorySummary {
    pub category: ContentCategory,
    pub disposition: MigrationDisposition,
    pub item_count: usize,
    pub total_bytes: u64,
    pub selected_by_default: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExcludedItemRecord {
    pub logical_path: String,
    pub reason: String,
    pub disposition: MigrationDisposition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceEnvironmentInfo {
    pub os: String,
    pub arch: String,
    pub codex_home: String,
    pub shared_skills_dir: Option<String>,
    pub sqlite_schemas: SqliteSchemaReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationManifest {
    pub format_version: String,
    pub migration_id: String,
    pub app_version: String,
    pub created_at: String,
    pub source_env: SourceEnvironmentInfo,
    pub adapter_capabilities: AdapterCapabilities,
    pub categories: Vec<CategorySummary>,
    pub threads: Vec<CodexThreadSummary>,
    pub projects: Vec<ProjectInventoryEntry>,
    pub files: Vec<ManifestFileEntry>,
    pub excluded_items: Vec<ExcludedItemRecord>,
    pub skill_warnings: Vec<SkillDependencyWarning>,
    pub sensitive_findings: Vec<SensitiveFinding>,
    pub general_warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanOptions {
    pub codex_home: String,
    pub shared_skills_dir: Option<String>,
    pub include_project_files: bool,
    pub excluded_project_dirs: Vec<String>,
    pub compute_sha256: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            codex_home: String::new(),
            shared_skills_dir: None,
            include_project_files: true,
            excluded_project_dirs: default_project_excluded_dirs(),
            compute_sha256: false,
        }
    }
}

/// Performs a full read-only scan of `CODEX_HOME`, shared skills, and associated projects.
pub fn scan_codex_inventory(
    options: &ScanOptions,
    limits: &SecurityLimits,
) -> AppResult<MigrationManifest> {
    let codex_home = PathBuf::from(options.codex_home.trim());
    if !codex_home.exists() || !codex_home.is_dir() {
        return Err(AppError::new(
            ErrorCode::CodexHomeNotFound,
            format!("Codex directory does not exist: {}", codex_home.display()),
        ));
    }

    let (sqlite_schemas, adapter_capabilities) = inspect_codex_databases(&codex_home)?;
    let (threads, raw_projects) = read_threads_and_projects(&codex_home, &sqlite_schemas)?;

    let mut files = Vec::new();
    let mut excluded_items = Vec::new();
    let mut skill_warnings = Vec::new();
    let mut sensitive_findings = Vec::new();
    let mut general_warnings = Vec::new();

    if sqlite_schemas.has_active_wal {
        general_warnings.push(
            "Detected active SQLite WAL file(s). Close Codex before generating the final migration snapshot to guarantee cross-file consistency."
                .to_string(),
        );
    }

    // 1. Always record excluded credentials & runtime caches explicitly
    let default_excluded_root_files = [
        (
            "auth.json",
            "Credentials excluded by default; sign in on the new computer.",
            MigrationDisposition::NeedsReauthOnTarget,
        ),
        (
            "installation_id",
            "Device-specific installation identity excluded.",
            MigrationDisposition::UnsupportedInCurrentVersion,
        ),
        (
            "cap_sid",
            "Device capability session identifier excluded.",
            MigrationDisposition::UnsupportedInCurrentVersion,
        ),
        (
            ".sqlite-maintenance.lock",
            "Temporary SQLite lock file excluded.",
            MigrationDisposition::UnsupportedInCurrentVersion,
        ),
        (
            ".desktop-created",
            "Device bootstrap marker excluded.",
            MigrationDisposition::UnsupportedInCurrentVersion,
        ),
    ];

    for (fname, reason, disp) in default_excluded_root_files {
        if codex_home.join(fname).exists() {
            excluded_items.push(ExcludedItemRecord {
                logical_path: fname.to_string(),
                reason: reason.to_string(),
                disposition: disp,
            });
        }
    }

    let default_excluded_dirs = [
        (
            ".sandbox",
            "Platform-specific sandbox environment excluded.",
        ),
        (
            ".sandbox-bin",
            "Platform-specific sandbox binaries excluded.",
        ),
        (".sandbox-secrets", "Ephemeral sandbox secrets excluded."),
        (".tmp", "Temporary directory excluded."),
        ("tmp", "Temporary directory excluded."),
        ("cache", "Runtime cache excluded."),
        ("mcp-oauth-locks", "Ephemeral OAuth lock files excluded."),
        ("project-metadata-locks", "Ephemeral lock files excluded."),
        ("thread-writer-locks", "Ephemeral writer locks excluded."),
        (
            "plugins/.plugin-appserver",
            "Platform-native plugin binaries excluded; plugins will be reinstalled on target.",
        ),
        ("plugins/cache", "Plugin runtime cache excluded."),
    ];

    for (dname, reason) in default_excluded_dirs {
        if codex_home.join(dname).exists() {
            excluded_items.push(ExcludedItemRecord {
                logical_path: dname.to_string(),
                reason: reason.to_string(),
                disposition: MigrationDisposition::NeedsReinstallOnTarget,
            });
        }
    }

    // Also mark logs_*.sqlite and queue_*.sqlite as excluded runtime telemetry/queues
    if let Ok(root_entries) = std::fs::read_dir(&codex_home) {
        for entry in root_entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if (name.starts_with("logs_") || name.starts_with("queue_"))
                && name.ends_with(".sqlite")
            {
                excluded_items.push(ExcludedItemRecord {
                    logical_path: name,
                    reason: "High-churn local debug log / transient queue database excluded."
                        .to_string(),
                    disposition: MigrationDisposition::UnsupportedInCurrentVersion,
                });
            }
        }
    }

    // 2. Scan Active & Archived Chats (`sessions/` and `archived_sessions/`)
    for sub in ["sessions", "archived_sessions"] {
        let dir = codex_home.join(sub);
        if dir.is_dir() {
            scan_directory_into_manifest(
                &dir,
                &codex_home,
                "codex_home",
                ContentCategory::ActiveAndArchivedChats,
                MigrationDisposition::NeedsPathMapping,
                None,
                &[],
                options.compute_sha256,
                limits,
                &mut files,
                &mut sensitive_findings,
                &mut skill_warnings,
            )?;
        }
    }

    // 3. Scan Chat Metadata & Index (`state_*.sqlite`, `thread_history_*.sqlite`, `session_index.jsonl`, `.codex-global-state.json`)
    let mut metadata_files = vec![
        "session_index.jsonl".to_string(),
        ".codex-global-state.json".to_string(),
    ];
    if let Some(ref sdb) = sqlite_schemas.state_db_filename {
        metadata_files.push(sdb.clone());
    }
    if let Some(ref thdb) = sqlite_schemas.thread_history_db_filename {
        metadata_files.push(thdb.clone());
    }
    for mfile in metadata_files {
        let p = codex_home.join(&mfile);
        if p.is_file() {
            push_single_file_entry(
                &p,
                &format!("codex_home/{mfile}"),
                &mfile,
                ContentCategory::ChatMetadataAndIndex,
                MigrationDisposition::NeedsPathMapping,
                None,
                options.compute_sha256,
                &mut files,
                &mut sensitive_findings,
            )?;
        }
    }

    // 4. Scan User Skills (`~/.codex/skills/`, skipping `.system` internal bundle cache)
    let skills_dir = codex_home.join("skills");
    if skills_dir.is_dir() {
        scan_directory_into_manifest(
            &skills_dir,
            &codex_home,
            "codex_home",
            ContentCategory::UserSkills,
            MigrationDisposition::DirectCopy,
            None,
            &[".system".to_string()],
            options.compute_sha256,
            limits,
            &mut files,
            &mut sensitive_findings,
            &mut skill_warnings,
        )?;
    }

    // 5. Scan Shared Skills (`~/.agents/skills/`)
    let shared_dir_opt = options
        .shared_skills_dir
        .as_ref()
        .map(PathBuf::from)
        .or_else(crate::platform::discover_shared_agents_skills_dir);

    let mut recorded_shared_dir = None;
    if let Some(ref s_dir) = shared_dir_opt {
        if s_dir.is_dir() {
            recorded_shared_dir = Some(s_dir.to_string_lossy().to_string());
            scan_directory_into_manifest(
                s_dir,
                s_dir,
                "shared_skills",
                ContentCategory::SharedAgentSkills,
                MigrationDisposition::DirectCopy,
                None,
                &[".system".to_string()],
                options.compute_sha256,
                limits,
                &mut files,
                &mut sensitive_findings,
                &mut skill_warnings,
            )?;
        }
    }

    // 6. Scan Instruction Files (`AGENTS.md`, `AGENTS.override.md`)
    for instr in ["AGENTS.md", "AGENTS.override.md"] {
        let p = codex_home.join(instr);
        if p.is_file() {
            push_single_file_entry(
                &p,
                &format!("codex_home/{instr}"),
                instr,
                ContentCategory::InstructionFiles,
                MigrationDisposition::DirectCopy,
                None,
                options.compute_sha256,
                &mut files,
                &mut sensitive_findings,
            )?;
        }
    }

    // 7. Scan Core Config (`config.toml`) and Plugin Manifests
    let cfg_path = codex_home.join("config.toml");
    let mut plugin_count = 0usize;
    if cfg_path.is_file() {
        push_single_file_entry(
            &cfg_path,
            "codex_home/config.toml",
            "config.toml",
            ContentCategory::CoreConfig,
            MigrationDisposition::NeedsPathMapping,
            None,
            options.compute_sha256,
            &mut files,
            &mut sensitive_findings,
        )?;

        if let Ok(raw_cfg) = std::fs::read_to_string(&cfg_path) {
            inspect_config_toml_for_secrets_and_plugins(
                &raw_cfg,
                &mut sensitive_findings,
                &mut plugin_count,
            );
        }
    }

    // 8. Scan Extensions: Memories, Rules, Hooks, Automations, Goals
    for ext_dir in ["memories", "automations", "rules"] {
        let d = codex_home.join(ext_dir);
        if d.is_dir() {
            scan_directory_into_manifest(
                &d,
                &codex_home,
                "codex_home",
                ContentCategory::ExtensionsMemoriesRulesHooks,
                MigrationDisposition::NeedsPathMapping,
                None,
                &[".git".to_string()],
                options.compute_sha256,
                limits,
                &mut files,
                &mut sensitive_findings,
                &mut skill_warnings,
            )?;
        }
    }
    for ext_file in [
        Some("hooks.json".to_string()),
        sqlite_schemas.memories_db_filename.clone(),
        sqlite_schemas.goals_db_filename.clone(),
    ]
    .into_iter()
    .flatten()
    {
        let p = codex_home.join(&ext_file);
        if p.is_file() {
            push_single_file_entry(
                &p,
                &format!("codex_home/{ext_file}"),
                &ext_file,
                ContentCategory::ExtensionsMemoriesRulesHooks,
                MigrationDisposition::NeedsPathMapping,
                None,
                options.compute_sha256,
                &mut files,
                &mut sensitive_findings,
            )?;
        }
    }

    // 9. Scan Chat Attachments (`attachments/`, `generated_images/`)
    for att_sub in ["attachments", "generated_images"] {
        let d = codex_home.join(att_sub);
        if d.is_dir() {
            scan_directory_into_manifest(
                &d,
                &codex_home,
                "codex_home",
                ContentCategory::ChatAttachments,
                MigrationDisposition::DirectCopy,
                None,
                &[],
                options.compute_sha256,
                limits,
                &mut files,
                &mut sensitive_findings,
                &mut skill_warnings,
            )?;
        }
    }

    // 10. Scan Projects & Check Nesting / Git Worktrees / Sensitive Files
    let projects = scan_projects_inventory(
        &raw_projects,
        options.include_project_files,
        &options.excluded_project_dirs,
        options.compute_sha256,
        limits,
        &mut files,
        &mut sensitive_findings,
        &mut skill_warnings,
        &mut general_warnings,
    )?;

    // Build Category Summaries
    let categories = build_category_summaries(&files, plugin_count);

    Ok(MigrationManifest {
        format_version: CURRENT_FORMAT_VERSION.to_string(),
        migration_id: Uuid::now_v7().to_string(),
        app_version: APP_VERSION.to_string(),
        created_at: Utc::now().to_rfc3339(),
        source_env: SourceEnvironmentInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            codex_home: codex_home.to_string_lossy().to_string(),
            shared_skills_dir: recorded_shared_dir,
            sqlite_schemas,
        },
        adapter_capabilities,
        categories,
        threads,
        projects,
        files,
        excluded_items,
        skill_warnings,
        sensitive_findings,
        general_warnings,
    })
}

#[allow(clippy::too_many_arguments)]
fn scan_directory_into_manifest(
    dir: &Path,
    base_root: &Path,
    archive_prefix: &str,
    category: ContentCategory,
    disposition: MigrationDisposition,
    project_id: Option<&str>,
    excluded_dir_names: &[String],
    compute_sha256: bool,
    limits: &SecurityLimits,
    files: &mut Vec<ManifestFileEntry>,
    sensitive_findings: &mut Vec<SensitiveFinding>,
    skill_warnings: &mut Vec<SkillDependencyWarning>,
) -> AppResult<()> {
    let excluded_set: HashSet<String> = excluded_dir_names
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();

    let walker = walkdir::WalkDir::new(dir).follow_links(false);

    for entry_res in walker {
        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path == dir {
            continue;
        }

        let rel = match path.strip_prefix(base_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let logical_rel = rel.to_string_lossy().replace('\\', "/");

        // Check if any component is in excluded_set or is a temporary lock/wal file
        if rel.components().any(|c| {
            let comp = c.as_os_str().to_string_lossy().to_ascii_lowercase();
            excluded_set.contains(&comp)
        }) {
            continue;
        }

        let fname = entry.file_name().to_string_lossy();
        if fname.ends_with(".sqlite-wal")
            || fname.ends_with(".sqlite-shm")
            || fname.ends_with(".lock")
            || fname.contains(".tmp-")
        {
            continue;
        }

        let file_type = entry.file_type();
        if file_type.is_dir() {
            continue;
        }

        if files.len() >= limits.max_file_count {
            return Err(AppError::new(
                ErrorCode::DecompressionLimitExceeded,
                format!(
                    "File count exceeds configured limit of {}. Adjust exclusions to proceed.",
                    limits.max_file_count
                ),
            ));
        }

        let archive_rel = format!("{archive_prefix}/{logical_rel}");

        if file_type.is_symlink() {
            let sym_class = classify_symlink(base_root, path);
            let entry_id = format!("f-{:06}", files.len() + 1);
            files.push(ManifestFileEntry {
                entry_id,
                category,
                archive_rel_path: archive_rel,
                logical_rel_path: logical_rel,
                source_abs_path: path.to_string_lossy().to_string(),
                project_id: project_id.map(|s| s.to_string()),
                size_bytes: 0,
                modified_unix_secs: 0,
                sha256: String::new(),
                permissions: PortablePermissions {
                    readonly: false,
                    unix_mode: None,
                    executable: false,
                },
                symlink: Some(sym_class),
                disposition,
                is_sensitive: false,
            });
            continue;
        }

        if file_type.is_file() {
            push_single_file_entry(
                path,
                &archive_rel,
                &logical_rel,
                category,
                disposition,
                project_id,
                compute_sha256,
                files,
                sensitive_findings,
            )?;

            if matches!(
                category,
                ContentCategory::UserSkills | ContentCategory::SharedAgentSkills
            ) {
                inspect_skill_file_for_platform_deps(path, &logical_rel, skill_warnings);
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn push_single_file_entry(
    path: &Path,
    archive_rel: &str,
    logical_rel: &str,
    category: ContentCategory,
    disposition: MigrationDisposition,
    project_id: Option<&str>,
    compute_sha256: bool,
    files: &mut Vec<ManifestFileEntry>,
    sensitive_findings: &mut Vec<SensitiveFinding>,
) -> AppResult<()> {
    let meta = std::fs::symlink_metadata(path)?;
    let size_bytes = meta.len();
    let modified_unix_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let permissions = extract_portable_permissions(path, &meta);
    let is_sensitive = detect_sensitive_file(path, logical_rel, sensitive_findings);

    let sha256 = if compute_sha256 {
        compute_file_sha256(path)?
    } else {
        String::new()
    };

    let entry_id = format!("f-{:06}", files.len() + 1);
    files.push(ManifestFileEntry {
        entry_id,
        category,
        archive_rel_path: archive_rel.to_string(),
        logical_rel_path: logical_rel.to_string(),
        source_abs_path: path.to_string_lossy().to_string(),
        project_id: project_id.map(|s| s.to_string()),
        size_bytes,
        modified_unix_secs,
        sha256,
        permissions,
        symlink: None,
        disposition,
        is_sensitive,
    });
    Ok(())
}

pub fn compute_file_sha256(path: &Path) -> AppResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn extract_portable_permissions(path: &Path, meta: &std::fs::Metadata) -> PortablePermissions {
    let readonly = meta.permissions().readonly();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o7777;
        let executable = (mode & 0o111) != 0;
        let _ = path;
        PortablePermissions {
            readonly,
            unix_mode: Some(mode),
            executable,
        }
    }
    #[cfg(not(unix))]
    {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let executable = matches!(ext.as_str(), "sh" | "py" | "pl" | "rb" | "command");
        PortablePermissions {
            readonly,
            unix_mode: Some(if executable { 0o755 } else { 0o644 }),
            executable,
        }
    }
}

fn detect_sensitive_file(
    path: &Path,
    logical_rel: &str,
    findings: &mut Vec<SensitiveFinding>,
) -> bool {
    let fname = path
        .file_name()
        .map(|f| f.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if fname == ".env"
        || fname.starts_with(".env.")
        || fname.ends_with(".pem")
        || fname.ends_with(".key")
        || fname.ends_with(".p12")
        || fname.ends_with(".pfx")
        || fname == "id_rsa"
        || fname == "id_ed25519"
    {
        findings.push(SensitiveFinding {
            relative_path: logical_rel.to_string(),
            finding_kind: "SensitiveFilePattern".to_string(),
            description: format!(
                "File '{}' matches a credential/certificate/environment pattern (.env / key).",
                fname
            ),
            selected_by_user: false,
        });
        return true;
    }

    false
}

fn inspect_config_toml_for_secrets_and_plugins(
    raw_cfg: &str,
    findings: &mut Vec<SensitiveFinding>,
    plugin_count: &mut usize,
) {
    let Ok(doc) = raw_cfg.parse::<toml_edit::DocumentMut>() else {
        return;
    };

    if let Some(plugins_tbl) = doc.get("plugins").and_then(|v| v.as_table()) {
        *plugin_count = plugins_tbl.len();
    }

    if let Some(mcp_tbl) = doc.get("mcp_servers").and_then(|v| v.as_table()) {
        for (srv_name, srv_val) in mcp_tbl.iter() {
            if let Some(tbl) = srv_val.as_table() {
                if tbl.contains_key("env")
                    || tbl.contains_key("bearer_token")
                    || tbl.contains_key("api_key")
                {
                    findings.push(SensitiveFinding {
                        relative_path: format!("config.toml#mcp_servers.{srv_name}"),
                        finding_kind: "McpServerCredential".to_string(),
                        description: format!(
                            "MCP server '{srv_name}' in config.toml defines environment variables or tokens."
                        ),
                        selected_by_user: false,
                    });
                }
            }
        }
    }
}

fn inspect_skill_file_for_platform_deps(
    path: &Path,
    logical_rel: &str,
    warnings: &mut Vec<SkillDependencyWarning>,
) {
    let fname = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let skill_name = logical_rel.split('/').nth(1).unwrap_or(&fname).to_string();

    if matches!(
        ext.as_str(),
        "exe" | "dll" | "dylib" | "so" | "ps1" | "bat" | "cmd"
    ) {
        warnings.push(SkillDependencyWarning {
            skill_name,
            relative_path: logical_rel.to_string(),
            warning_kind: "PlatformSpecificExecutableOrScript".to_string(),
            detail: format!(
                "Contains platform-specific binary or script '{}'. It may require reinstallation or adaptation on the target OS.",
                fname
            ),
        });
        return;
    }

    if matches!(
        ext.as_str(),
        "md" | "py" | "sh" | "js" | "ts" | "toml" | "json"
    ) {
        if let Ok(meta) = path.metadata() {
            if meta.len() <= 256 * 1024 {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if content.contains("C:\\Users\\")
                        || content.contains("/Users/")
                        || content.contains("powershell")
                        || content.contains("/opt/homebrew")
                    {
                        warnings.push(SkillDependencyWarning {
                            skill_name,
                            relative_path: logical_rel.to_string(),
                            warning_kind: "HardcodedPathOrInterpreter".to_string(),
                            detail: "References local user path or OS-specific interpreter (e.g., C:\\Users\\, /Users/, PowerShell, Homebrew).".to_string(),
                        });
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_projects_inventory(
    raw_projects: &[CodexProjectSummary],
    include_project_files: bool,
    excluded_project_dirs: &[String],
    compute_sha256: bool,
    limits: &SecurityLimits,
    files: &mut Vec<ManifestFileEntry>,
    sensitive_findings: &mut Vec<SensitiveFinding>,
    skill_warnings: &mut Vec<SkillDependencyWarning>,
    general_warnings: &mut Vec<String>,
) -> AppResult<Vec<ProjectInventoryEntry>> {
    let mut out = Vec::new();

    // Detect nested project roots so we never package the same directory twice
    let mut canonical_roots: Vec<(String, PathBuf)> = Vec::new();
    for proj in raw_projects {
        if let Some(first_root) = proj.root_paths.first() {
            let p = PathBuf::from(first_root);
            if let Ok(canon) = p.canonicalize() {
                canonical_roots.push((proj.id.clone(), canon));
            }
        }
    }

    for proj in raw_projects {
        let first_root_opt = proj.root_paths.first().map(PathBuf::from);
        let exists_on_disk = first_root_opt.as_ref().map(|p| p.is_dir()).unwrap_or(false);

        let mut nested_under = None;
        if let Some(ref root_p) = first_root_opt {
            if let Ok(canon_self) = root_p.canonicalize() {
                for (other_id, other_canon) in &canonical_roots {
                    if other_id != &proj.id
                        && canon_self.starts_with(other_canon)
                        && canon_self != *other_canon
                    {
                        nested_under = Some(other_id.clone());
                        break;
                    }
                }
            }
        }

        let mut git_status = GitRepoStatus::NoGit;
        let mut proj_files_count = 0usize;
        let mut proj_bytes = 0u64;
        let mut proj_sensitive = Vec::new();

        if let Some(ref root_p) = first_root_opt {
            if exists_on_disk {
                git_status = inspect_git_repository_status(root_p);
                if let GitRepoStatus::ExternalWorktree { ref gitdir_pointer } = git_status {
                    general_warnings.push(format!(
                        "Project '{}' is a Git worktree pointing to external gitdir '{}'. Its standalone .git history lives outside this folder.",
                        proj.name, gitdir_pointer
                    ));
                }

                if include_project_files && nested_under.is_none() {
                    let before_len = files.len();
                    let before_sens = sensitive_findings.len();
                    let archive_prefix = format!("projects/{}", proj.id);

                    scan_directory_into_manifest(
                        root_p,
                        root_p,
                        &archive_prefix,
                        ContentCategory::ProjectFiles,
                        MigrationDisposition::DirectCopy,
                        Some(&proj.id),
                        excluded_project_dirs,
                        compute_sha256,
                        limits,
                        files,
                        sensitive_findings,
                        skill_warnings,
                    )?;

                    for f in &files[before_len..] {
                        proj_files_count += 1;
                        proj_bytes += f.size_bytes;
                    }
                    proj_sensitive.extend_from_slice(&sensitive_findings[before_sens..]);
                }
            }
        }

        let has_git = !matches!(git_status, GitRepoStatus::NoGit);
        out.push(ProjectInventoryEntry {
            project_id: proj.id.clone(),
            name: proj.name.clone(),
            source_root_paths: proj.root_paths.clone(),
            exists_on_disk,
            include_project_files: include_project_files
                && exists_on_disk
                && nested_under.is_none(),
            git_status,
            has_uncommitted_changes: has_git,
            has_untracked_files: exists_on_disk,
            total_files: proj_files_count,
            total_bytes: proj_bytes,
            sensitive_findings: proj_sensitive,
            nested_under_project_id: nested_under,
        });
    }

    Ok(out)
}

/// Inspects `.git` in `project_root` without executing external git shell commands:
/// - If `.git` is a directory, checks `.gitmodules` for submodules.
/// - If `.git` is a file (`gitdir: ...`), identifies it as a Git worktree or submodule pointer.
pub fn inspect_git_repository_status(project_root: &Path) -> GitRepoStatus {
    let dot_git = project_root.join(".git");
    if dot_git.is_file() {
        if let Ok(content) = std::fs::read_to_string(&dot_git) {
            let pointer = content
                .lines()
                .find_map(|l| l.strip_prefix("gitdir:"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| content.trim().to_string());
            return GitRepoStatus::ExternalWorktree {
                gitdir_pointer: pointer,
            };
        }
    } else if dot_git.is_dir() {
        let gitmodules = project_root.join(".gitmodules");
        if gitmodules.is_file() {
            if let Ok(content) = std::fs::read_to_string(&gitmodules) {
                let count = content
                    .lines()
                    .filter(|l| l.trim().starts_with("[submodule"))
                    .count();
                if count > 0 {
                    return GitRepoStatus::HasSubmodules { count };
                }
            }
        }
        return GitRepoStatus::StandardRepo;
    }
    GitRepoStatus::NoGit
}

fn build_category_summaries(
    files: &[ManifestFileEntry],
    plugin_count: usize,
) -> Vec<CategorySummary> {
    let all_categories = [
        (
            ContentCategory::ActiveAndArchivedChats,
            MigrationDisposition::NeedsPathMapping,
            true,
            "Active and archived conversation transcripts (.jsonl / .jsonl.zst)",
        ),
        (
            ContentCategory::ChatMetadataAndIndex,
            MigrationDisposition::NeedsPathMapping,
            true,
            "Thread titles, project bindings, and fast lookup indexes",
        ),
        (
            ContentCategory::UserSkills,
            MigrationDisposition::DirectCopy,
            true,
            "Custom skills inside .codex/skills",
        ),
        (
            ContentCategory::SharedAgentSkills,
            MigrationDisposition::DirectCopy,
            true,
            "Cross-agent shared skills inside .agents/skills",
        ),
        (
            ContentCategory::InstructionFiles,
            MigrationDisposition::DirectCopy,
            true,
            "Global AGENTS.md and AGENTS.override.md instructions",
        ),
        (
            ContentCategory::CoreConfig,
            MigrationDisposition::NeedsPathMapping,
            true,
            "Preferences, model settings, and project trust levels in config.toml",
        ),
        (
            ContentCategory::ExtensionsMemoriesRulesHooks,
            MigrationDisposition::NeedsPathMapping,
            true,
            "Memories, goals, hooks, and automations (restored in paused state)",
        ),
        (
            ContentCategory::ChatAttachments,
            MigrationDisposition::DirectCopy,
            true,
            "Chat attachments and generated media",
        ),
        (
            ContentCategory::ProjectFiles,
            MigrationDisposition::DirectCopy,
            true,
            "Project source files, uncommitted changes, untracked files, and .git history",
        ),
        (
            ContentCategory::PluginManifests,
            MigrationDisposition::NeedsReinstallOnTarget,
            true,
            "Enabled plugin declarations (native binaries excluded; reinstall on target)",
        ),
    ];

    let mut counts: HashMap<ContentCategory, (usize, u64)> = HashMap::new();
    for f in files {
        let entry = counts.entry(f.category).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += f.size_bytes;
    }

    all_categories
        .into_iter()
        .map(|(cat, disp, sel, desc)| {
            let (mut item_count, total_bytes) = counts.get(&cat).copied().unwrap_or((0, 0));
            if cat == ContentCategory::PluginManifests && item_count == 0 {
                item_count = plugin_count;
            }
            CategorySummary {
                category: cat,
                disposition: disp,
                item_count,
                total_bytes,
                selected_by_default: sel,
                notes: vec![desc.to_string()],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_scan_inventory_identifies_worktree_and_sensitive_env() {
        let dir = tempdir().unwrap();
        let codex_home = dir.path().join(".codex");
        let proj_dir = dir.path().join("my-worktree-proj");
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::create_dir_all(&proj_dir).unwrap();

        // Create auth.json (must be excluded)
        std::fs::write(codex_home.join("auth.json"), r#"{"token":"secret"}"#).unwrap();
        // Create AGENTS.md
        std::fs::write(codex_home.join("AGENTS.md"), "# Global Instructions").unwrap();
        // Create config.toml referencing project
        let cfg = format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            proj_dir.to_string_lossy().replace('\\', "\\\\")
        );
        std::fs::write(codex_home.join("config.toml"), cfg).unwrap();

        // Simulate git worktree pointer file and .env file inside project
        std::fs::write(
            proj_dir.join(".git"),
            "gitdir: /external/repo/.git/worktrees/wt1",
        )
        .unwrap();
        std::fs::write(proj_dir.join(".env"), "OPENAI_API_KEY=sk-1234567890abcdef").unwrap();
        std::fs::write(proj_dir.join("main.rs"), "fn main() {}").unwrap();

        let opts = ScanOptions {
            codex_home: codex_home.to_string_lossy().to_string(),
            shared_skills_dir: None,
            include_project_files: true,
            excluded_project_dirs: default_project_excluded_dirs(),
            compute_sha256: true,
        };

        let manifest = scan_codex_inventory(&opts, &SecurityLimits::default()).unwrap();

        // Verify auth.json is in excluded_items and NOT in files
        assert!(manifest
            .excluded_items
            .iter()
            .any(|e| e.logical_path == "auth.json"));
        assert!(!manifest
            .files
            .iter()
            .any(|f| f.logical_rel_path.contains("auth.json")));

        // Verify worktree detection
        assert_eq!(manifest.projects.len(), 1);
        assert!(matches!(
            manifest.projects[0].git_status,
            GitRepoStatus::ExternalWorktree { .. }
        ));

        // Verify .env sensitive finding
        assert!(!manifest.sensitive_findings.is_empty());
    }
}
