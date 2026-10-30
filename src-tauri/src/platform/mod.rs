use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use sysinfo::{Disks, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveCodexProcess {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemOverview {
    pub os: String,
    pub arch: String,
    pub hostname_display: String,
    pub default_codex_home: Option<String>,
    pub candidate_codex_homes: Vec<String>,
    pub shared_agents_skills_dir: Option<String>,
    pub active_codex_processes: Vec<ActiveCodexProcess>,
    pub webview2_runtime_version: Option<String>,
    pub available_disk_bytes: u64,
    pub supports_symlinks: bool,
}

/// Discovers `CODEX_HOME` at runtime:
/// 1. Checks `CODEX_HOME` environment variable if set.
/// 2. Checks `~/.codex` (or `%USERPROFILE%\.codex` on Windows).
/// 3. Collects any existing candidate `.codex` directories.
pub fn discover_codex_homes() -> (Option<PathBuf>, Vec<PathBuf>) {
    let mut candidates = Vec::new();

    if let Ok(env_home) = std::env::var("CODEX_HOME") {
        let trimmed = env_home.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            candidates.push(p);
        }
    }

    if let Some(home_dir) = dirs::home_dir() {
        let default_dot_codex = home_dir.join(".codex");
        if !candidates.contains(&default_dot_codex) {
            candidates.push(default_dot_codex);
        }
    }

    let primary = candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .or_else(|| candidates.first().cloned());

    (primary, candidates)
}

pub fn discover_shared_agents_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".agents").join("skills"))
}

/// Detects running Codex CLI or Codex Desktop processes without killing them.
/// Also checks for `.sqlite-maintenance.lock` or active WAL writers inside `codex_home`.
pub fn detect_active_codex_processes(codex_home: Option<&Path>) -> Vec<ActiveCodexProcess> {
    let mut sys = System::new_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut found = Vec::new();
    for (pid, process) in sys.processes() {
        let name = process.name().to_string_lossy().to_ascii_lowercase();
        // Exclude ourselves (`codex-move`)
        if name.contains("codex-move") || name.contains("codex_move") {
            continue;
        }
        if name == "codex"
            || name == "codex.exe"
            || name == "codex desktop"
            || name == "codex desktop.exe"
            || name == "codex-app-server"
            || name == "codex-app-server.exe"
        {
            found.push(ActiveCodexProcess {
                pid: pid.as_u32(),
                name: process.name().to_string_lossy().to_string(),
            });
        }
    }

    if let Some(home) = codex_home {
        let lock_file = home.join(".sqlite-maintenance.lock");
        let writer_locks = home.join("thread-writer-locks");
        if writer_locks.is_dir() {
            if let Ok(mut entries) = std::fs::read_dir(&writer_locks) {
                if entries.next().is_some() && found.is_empty() {
                    found.push(ActiveCodexProcess {
                        pid: 0,
                        name: "Codex Active Thread Writer Lock".to_string(),
                    });
                }
            }
        }
        let _ = lock_file;
    }

    found
}

/// Queries available disk space in bytes for the disk containing `target_path`.
pub fn available_disk_space(target_path: &Path) -> u64 {
    let disks = Disks::new_with_refreshed_list();
    let canonical = target_path
        .canonicalize()
        .unwrap_or_else(|_| target_path.to_path_buf());

    let mut best_match_len = 0usize;
    let mut available = 0u64;

    for disk in &disks {
        let mount = disk.mount_point();
        if canonical.starts_with(mount) {
            let len = mount.as_os_str().len();
            if len >= best_match_len {
                best_match_len = len;
                available = disk.available_space();
            }
        }
    }

    if available == 0 {
        if let Some(first) = disks.list().first() {
            available = first.available_space();
        }
    }
    available
}

/// Probes whether the current process can create symbolic links in the given directory.
pub fn probe_symlink_support(dir: &Path) -> bool {
    let Ok(temp_dir) = tempfile::tempdir_in(dir) else {
        return cfg!(unix);
    };
    let target = temp_dir.path().join("target.txt");
    let link = temp_dir.path().join("link.txt");
    if std::fs::write(&target, b"probe").is_err() {
        return false;
    }
    create_relative_file_symlink(Path::new("target.txt"), &link).is_ok()
}

pub fn create_relative_file_symlink(rel_target: &Path, link_path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(rel_target, link_path)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(rel_target, link_path)
    }
}

pub fn detect_webview2_version() -> Option<String> {
    #[cfg(windows)]
    {
        if let Ok(env_ver) = std::env::var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER") {
            return Some(format!("Custom ({env_ver})"));
        }
        Some("Microsoft Edge WebView2 (Evergreen)".to_string())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn get_system_overview() -> SystemOverview {
    let (primary, candidates) = discover_codex_homes();
    let active_procs = detect_active_codex_processes(primary.as_deref());
    let shared_skills = discover_shared_agents_skills_dir();
    let probe_dir = primary
        .as_deref()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let available_disk_bytes = available_disk_space(&probe_dir);

    let hostname = System::host_name().unwrap_or_else(|| "Codex-Computer".to_string());

    SystemOverview {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        hostname_display: hostname,
        default_codex_home: primary.map(|p| p.to_string_lossy().to_string()),
        candidate_codex_homes: candidates
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        shared_agents_skills_dir: shared_skills.map(|p| p.to_string_lossy().to_string()),
        active_codex_processes: active_procs,
        webview2_runtime_version: detect_webview2_version(),
        available_disk_bytes,
        supports_symlinks: probe_symlink_support(std::env::temp_dir().as_path()),
    }
}
