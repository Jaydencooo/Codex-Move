use crate::errors::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathStyle {
    Windows,
    Posix,
}

impl PathStyle {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Posix
        }
    }

    pub fn separator(self) -> char {
        match self {
            Self::Windows => '\\',
            Self::Posix => '/',
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathMappingRule {
    pub project_id: Option<String>,
    pub source_prefix: String,
    pub target_prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PathValidationIssueKind {
    CaseCollisionOnTarget,
    UnicodeNormalizationCollision,
    WindowsReservedDeviceName,
    IllegalWindowsCharacters,
    TrailingDotOrSpace,
    PathTooLong,
    TargetDirectoryAlreadyExists,
    OverlappingTargetRoots,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathValidationIssue {
    pub kind: PathValidationIssueKind,
    pub relative_or_target_path: String,
    pub conflicting_with: Option<String>,
    pub blocking: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SymlinkClassification {
    InternalRelative { target_rel: String },
    ExternalTarget { raw_target: String },
    Broken { raw_target: String },
}

/// Validates and normalizes an archive-relative path.
/// Strictly blocks `..`, leading `/` or `\`, Windows drive prefixes (`C:`), and UNC prefixes (`\\`).
pub fn sanitize_relative_path(raw: &str, max_len: usize) -> AppResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::new(
            ErrorCode::InvalidRelativePath,
            "Relative path cannot be empty.",
        ));
    }
    if trimmed.len() > max_len {
        return Err(AppError::new(
            ErrorCode::InvalidRelativePath,
            format!("Relative path exceeds maximum length of {max_len} characters."),
        ));
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(AppError::new(
            ErrorCode::PathTraversalDetected,
            format!("Absolute path injection blocked: {trimmed}"),
        ));
    }
    if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
        return Err(AppError::new(
            ErrorCode::PathTraversalDetected,
            format!("Windows drive prefix injection blocked: {trimmed}"),
        ));
    }

    let unified = trimmed.replace('\\', "/");
    let mut clean_parts = Vec::new();
    for segment in unified.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(AppError::new(
                ErrorCode::PathTraversalDetected,
                format!("Parent directory traversal (..) blocked in path: {trimmed}"),
            ));
        }
        if segment.contains('\0') {
            return Err(AppError::new(
                ErrorCode::InvalidRelativePath,
                "Null byte in path is forbidden.",
            ));
        }
        clean_parts.push(segment);
    }

    if clean_parts.is_empty() {
        return Err(AppError::new(
            ErrorCode::InvalidRelativePath,
            "Relative path resolved to empty.",
        ));
    }

    Ok(clean_parts.join("/"))
}

/// Converts a sanitized `/` relative path into a safe child `PathBuf` under `base_dir`.
pub fn resolve_safe_child_path(
    base_dir: &Path,
    raw_rel: &str,
    max_len: usize,
) -> AppResult<PathBuf> {
    let clean = sanitize_relative_path(raw_rel, max_len)?;
    let mut out = base_dir.to_path_buf();
    for part in clean.split('/') {
        out.push(part);
    }
    Ok(out)
}

/// Normalizes a path into canonical segments for boundary-safe prefix matching.
/// Preserves drive letters (`c:`) or UNC roots (`//server/share`) in normalized form.
fn split_normalized_components(path_str: &str) -> (Vec<String>, Vec<String>, PathStyle) {
    let trimmed = path_str.trim();
    let is_windows_style = trimmed.contains('\\')
        || (trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':')
        || trimmed.starts_with("\\\\");

    let style = if is_windows_style {
        PathStyle::Windows
    } else {
        PathStyle::Posix
    };

    let unified = trimmed.replace('\\', "/");
    let raw_segments: Vec<String> = unified
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_string())
        .collect();

    let compare_segments: Vec<String> = raw_segments
        .iter()
        .enumerate()
        .map(|(idx, seg)| {
            if is_windows_style {
                seg.nfc().collect::<String>().to_lowercase()
            } else if idx == 0 && seg.ends_with(':') {
                seg.to_lowercase()
            } else {
                seg.nfc().collect::<String>()
            }
        })
        .collect();

    (raw_segments, compare_segments, style)
}

/// Rewrites `input_path` using the longest matching `PathMappingRule` strictly respecting
/// path component boundaries (e.g. `/Users/alice/proj` will NOT match `/Users/alice/proj-2`).
/// Also handles `local:C:\Users\...` host-prefixed keys used in `.codex-global-state.json`.
pub fn rewrite_path_with_rules(
    input_path: &str,
    rules: &[PathMappingRule],
    target_style: PathStyle,
) -> Option<String> {
    if let Some(stripped) = input_path.strip_prefix("local:") {
        return rewrite_path_with_rules(stripped, rules, target_style)
            .map(|mapped| format!("local:{mapped}"));
    }

    let (input_raw, input_cmp, _) = split_normalized_components(input_path);
    if input_cmp.is_empty() {
        return None;
    }

    let mut best_match: Option<(&PathMappingRule, usize)> = None;

    for rule in rules {
        let (_, rule_cmp, _) = split_normalized_components(&rule.source_prefix);
        if rule_cmp.is_empty() || rule_cmp.len() > input_cmp.len() {
            continue;
        }
        // Component boundary check
        let matches = input_cmp[..rule_cmp.len()]
            .iter()
            .zip(rule_cmp.iter())
            .all(|(a, b)| a == b || a.eq_ignore_ascii_case(b));

        if matches {
            let match_len = rule_cmp.len();
            if best_match.map_or(true, |(_, prev_len)| match_len > prev_len) {
                best_match = Some((rule, match_len));
            }
        }
    }

    let (winning_rule, matched_components) = best_match?;
    let remaining_segments = &input_raw[matched_components..];

    let sep = target_style.separator();
    let mut base = winning_rule
        .target_prefix
        .trim_end_matches(&['/', '\\'][..])
        .to_string();

    if target_style == PathStyle::Windows {
        base = base.replace('/', "\\");
    } else {
        base = base.replace('\\', "/");
    }

    if remaining_segments.is_empty() {
        return Some(base);
    }

    let mut out = base;
    for seg in remaining_segments {
        if !out.ends_with(sep) {
            out.push(sep);
        }
        out.push_str(seg);
    }
    Some(out)
}

const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Validates a list of relative file paths against target OS filesystem constraints:
/// - Case-insensitivity collisions (for Windows NTFS and macOS default APFS)
/// - Unicode NFC/NFD normalization collisions
/// - Windows reserved device names (`CON`, `NUL`, `AUX`, `COM1`..`9`, `LPT1`..`9`)
/// - Windows illegal characters (`< > : " | ? *`) and trailing dots/spaces
pub fn validate_relative_paths_for_target(
    relative_paths: &[String],
    target_style: PathStyle,
    target_case_insensitive: bool,
) -> Vec<PathValidationIssue> {
    let mut issues = Vec::new();
    let mut seen_case_folded: HashMap<String, String> = HashMap::new();
    let mut seen_nfc_folded: HashMap<String, String> = HashMap::new();

    for rel in relative_paths {
        let nfc_rel: String = rel.nfkc().collect();
        if let Some(existing) = seen_nfc_folded.get(&nfc_rel) {
            if existing != rel {
                issues.push(PathValidationIssue {
                    kind: PathValidationIssueKind::UnicodeNormalizationCollision,
                    relative_or_target_path: rel.clone(),
                    conflicting_with: Some(existing.clone()),
                    blocking: true,
                    description: format!(
                        "Unicode NFC/NFD normalization conflict between '{}' and '{}'",
                        rel, existing
                    ),
                });
            }
        } else {
            seen_nfc_folded.insert(nfc_rel.clone(), rel.clone());
        }

        if target_case_insensitive {
            let lower_key = nfc_rel.to_lowercase();
            if let Some(existing) = seen_case_folded.get(&lower_key) {
                if existing != rel {
                    issues.push(PathValidationIssue {
                        kind: PathValidationIssueKind::CaseCollisionOnTarget,
                        relative_or_target_path: rel.clone(),
                        conflicting_with: Some(existing.clone()),
                        blocking: true,
                        description: format!(
                            "Case-insensitive filename collision between '{}' and '{}'",
                            rel, existing
                        ),
                    });
                }
            } else {
                seen_case_folded.insert(lower_key, rel.clone());
            }
        }

        if target_style == PathStyle::Windows {
            if rel.len() > 240 {
                issues.push(PathValidationIssue {
                    kind: PathValidationIssueKind::PathTooLong,
                    relative_or_target_path: rel.clone(),
                    conflicting_with: None,
                    blocking: false,
                    description: "Path exceeds classic 260-char Windows MAX_PATH threshold."
                        .to_string(),
                });
            }

            for segment in rel.split('/') {
                if segment.ends_with('.') || segment.ends_with(' ') {
                    issues.push(PathValidationIssue {
                        kind: PathValidationIssueKind::TrailingDotOrSpace,
                        relative_or_target_path: rel.clone(),
                        conflicting_with: None,
                        blocking: true,
                        description: format!(
                            "Segment '{}' ends with a dot or space, which Windows strips/forbids.",
                            segment
                        ),
                    });
                }

                if segment
                    .chars()
                    .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
                {
                    issues.push(PathValidationIssue {
                        kind: PathValidationIssueKind::IllegalWindowsCharacters,
                        relative_or_target_path: rel.clone(),
                        conflicting_with: None,
                        blocking: true,
                        description: format!(
                            "Segment '{}' contains illegal Windows filename characters.",
                            segment
                        ),
                    });
                }

                let stem = segment
                    .split('.')
                    .next()
                    .unwrap_or(segment)
                    .to_ascii_uppercase();
                if WINDOWS_RESERVED_NAMES.contains(&stem.as_str()) {
                    issues.push(PathValidationIssue {
                        kind: PathValidationIssueKind::WindowsReservedDeviceName,
                        relative_or_target_path: rel.clone(),
                        conflicting_with: None,
                        blocking: true,
                        description: format!(
                            "Segment '{}' uses a Windows reserved device name ({}).",
                            segment, stem
                        ),
                    });
                }
            }
        }
    }

    issues
}

/// Validates target project root mappings to prevent overlapping directories or duplicate names.
pub fn validate_project_target_roots(
    project_targets: &[(String, String, bool)], // (project_id, target_path, bind_existing_allowed)
    target_style: PathStyle,
) -> Vec<PathValidationIssue> {
    let mut issues = Vec::new();
    let mut normalized_roots: Vec<(String, String, Vec<String>)> = Vec::new();
    let mut seen_exact = HashSet::new();

    for (proj_id, target_path, bind_existing_allowed) in project_targets {
        let (_, cmp_parts, _) = split_normalized_components(target_path);
        let key = cmp_parts.join("/");

        if !seen_exact.insert(key.clone()) {
            issues.push(PathValidationIssue {
                kind: PathValidationIssueKind::OverlappingTargetRoots,
                relative_or_target_path: target_path.clone(),
                conflicting_with: Some(proj_id.clone()),
                blocking: true,
                description: format!(
                    "Multiple projects are mapped to the exact same target folder: {}",
                    target_path
                ),
            });
        }

        let p = Path::new(target_path);
        if p.exists() && !*bind_existing_allowed {
            issues.push(PathValidationIssue {
                kind: PathValidationIssueKind::TargetDirectoryAlreadyExists,
                relative_or_target_path: target_path.clone(),
                conflicting_with: None,
                blocking: false,
                description: format!(
                    "Target folder '{}' already exists. Choose a new folder or explicitly bind to the existing project.",
                    target_path
                ),
            });
        }

        normalized_roots.push((proj_id.clone(), target_path.clone(), cmp_parts));
    }

    // Check if one project's target directory is nested inside another's
    for i in 0..normalized_roots.len() {
        for j in (i + 1)..normalized_roots.len() {
            let (_, path_a, parts_a) = &normalized_roots[i];
            let (_, path_b, parts_b) = &normalized_roots[j];
            if parts_a.is_empty() || parts_b.is_empty() || parts_a == parts_b {
                continue;
            }
            let a_in_b = parts_b.len() < parts_a.len() && parts_a[..parts_b.len()] == *parts_b;
            let b_in_a = parts_a.len() < parts_b.len() && parts_b[..parts_a.len()] == *parts_a;
            if a_in_b || b_in_a {
                issues.push(PathValidationIssue {
                    kind: PathValidationIssueKind::OverlappingTargetRoots,
                    relative_or_target_path: path_a.clone(),
                    conflicting_with: Some(path_b.clone()),
                    blocking: true,
                    description: format!(
                        "Target project paths overlap (nested directories): '{}' and '{}'",
                        path_a, path_b
                    ),
                });
            }
        }
    }

    let _ = target_style;
    issues
}

/// Classifies a symbolic link at `link_path` inside `project_root`.
/// Never follows external symlinks to copy outside data.
pub fn classify_symlink(project_root: &Path, link_path: &Path) -> SymlinkClassification {
    let Ok(raw_target) = std::fs::read_link(link_path) else {
        return SymlinkClassification::Broken {
            raw_target: String::new(),
        };
    };
    let raw_target_str = raw_target.to_string_lossy().to_string();

    let resolved = if raw_target.is_absolute() {
        raw_target.clone()
    } else {
        link_path.parent().unwrap_or(project_root).join(&raw_target)
    };

    let Ok(canon_root) = project_root.canonicalize() else {
        return SymlinkClassification::Broken {
            raw_target: raw_target_str,
        };
    };

    match resolved.canonicalize() {
        Ok(canon_target) => {
            if canon_target.starts_with(&canon_root) {
                if let Ok(rel) = canon_target.strip_prefix(&canon_root) {
                    let clean_rel = rel.to_string_lossy().replace('\\', "/");
                    SymlinkClassification::InternalRelative {
                        target_rel: clean_rel,
                    }
                } else {
                    SymlinkClassification::ExternalTarget {
                        raw_target: raw_target_str,
                    }
                }
            } else {
                SymlinkClassification::ExternalTarget {
                    raw_target: raw_target_str,
                }
            }
        }
        Err(_) => SymlinkClassification::Broken {
            raw_target: raw_target_str,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_to_macos_path_mapping_and_boundaries() {
        let rules = vec![
            PathMappingRule {
                project_id: Some("p1".to_string()),
                source_prefix: r"C:\Users\standardsoftware\Documents\ChatGPT\anysms".to_string(),
                target_prefix: "/Users/alice/Projects/anysms".to_string(),
            },
            PathMappingRule {
                project_id: None,
                source_prefix: r"C:\Users\standardsoftware\.codex".to_string(),
                target_prefix: "/Users/alice/.codex".to_string(),
            },
        ];

        // Exact match
        assert_eq!(
            rewrite_path_with_rules(
                r"C:\Users\standardsoftware\Documents\ChatGPT\anysms",
                &rules,
                PathStyle::Posix
            ),
            Some("/Users/alice/Projects/anysms".to_string())
        );

        // Child path with Chinese, spaces, and emoji
        assert_eq!(
            rewrite_path_with_rules(
                r"c:\users\standardsoftware\Documents\ChatGPT\anysms\子目录 🚀\file name.rs",
                &rules,
                PathStyle::Posix
            ),
            Some("/Users/alice/Projects/anysms/子目录 🚀/file name.rs".to_string())
        );

        // Component boundary protection: `anysms_backend` must NOT match `anysms`
        assert_eq!(
            rewrite_path_with_rules(
                r"C:\Users\standardsoftware\Documents\ChatGPT\anysms_backend\main.py",
                &rules,
                PathStyle::Posix
            ),
            None
        );

        // Host-prefixed global state key
        assert_eq!(
            rewrite_path_with_rules(
                r"local:C:\Users\standardsoftware\.codex",
                &rules,
                PathStyle::Posix
            ),
            Some("local:/Users/alice/.codex".to_string())
        );
    }

    #[test]
    fn test_macos_to_windows_path_mapping() {
        let rules = vec![PathMappingRule {
            project_id: Some("p2".to_string()),
            source_prefix: "/Users/bob/workspace/app".to_string(),
            target_prefix: r"D:\Dev\app".to_string(),
        }];

        assert_eq!(
            rewrite_path_with_rules(
                "/Users/bob/workspace/app/src/lib.rs",
                &rules,
                PathStyle::Windows
            ),
            Some(r"D:\Dev\app\src\lib.rs".to_string())
        );
    }

    #[test]
    fn test_sanitize_relative_path_blocks_traversal_and_drive_injection() {
        assert!(sanitize_relative_path("../../etc/passwd", 1024).is_err());
        assert!(sanitize_relative_path(r"..\..\Windows\System32", 1024).is_err());
        assert!(sanitize_relative_path(r"C:\Windows\win.ini", 1024).is_err());
        assert!(sanitize_relative_path("/etc/shadow", 1024).is_err());
        assert_eq!(
            sanitize_relative_path(r"sessions\2026\10\rollout.jsonl", 1024).unwrap(),
            "sessions/2026/10/rollout.jsonl"
        );
    }

    #[test]
    fn test_validate_relative_paths_detects_case_and_windows_reserved_names() {
        let paths = vec![
            "src/Readme.md".to_string(),
            "src/README.md".to_string(),
            "docs/aux.txt".to_string(),
            "scripts/bad:name?.sh".to_string(),
        ];

        let issues = validate_relative_paths_for_target(&paths, PathStyle::Windows, true);
        assert!(issues
            .iter()
            .any(|i| i.kind == PathValidationIssueKind::CaseCollisionOnTarget));
        assert!(issues
            .iter()
            .any(|i| i.kind == PathValidationIssueKind::WindowsReservedDeviceName));
        assert!(issues
            .iter()
            .any(|i| i.kind == PathValidationIssueKind::IllegalWindowsCharacters));
    }
}
