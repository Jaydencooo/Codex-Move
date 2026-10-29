use serde::{Deserialize, Serialize};

pub const CURRENT_FORMAT_VERSION: &str = "1.0.0";
pub const CURRENT_PROTOCOL_VERSION: &str = "1.0.0";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_DISCOVERY_PORT: u16 = 53317;
pub const DEFAULT_MULTICAST_ADDR: &str = "224.0.0.167";

/// Centralized security and resource bounds for untrusted archive and network inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityLimits {
    /// Maximum size of `manifest.json` in bytes (default: 32 MiB)
    pub max_manifest_bytes: u64,
    /// Maximum number of file entries in a single migration (default: 250,000)
    pub max_file_count: usize,
    /// Maximum relative path length in characters (default: 1024)
    pub max_path_length: usize,
    /// Maximum size of a single config/metadata file in bytes (default: 64 MiB)
    pub max_metadata_file_bytes: u64,
    /// Maximum size of a single transfer chunk in bytes (default: 4 MiB)
    pub stream_chunk_bytes: usize,
    /// Maximum concurrent HTTP chunk requests (default: 4)
    pub max_concurrent_requests: usize,
    /// Pairing code validity duration in seconds (default: 300s / 5 min)
    pub pairing_ttl_seconds: u64,
    /// Maximum failed pairing attempts before lockout (default: 5)
    pub max_pairing_attempts: u32,
}

impl Default for SecurityLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 32 * 1024 * 1024,
            max_file_count: 250_000,
            max_path_length: 1024,
            max_metadata_file_bytes: 64 * 1024 * 1024,
            stream_chunk_bytes: 4 * 1024 * 1024,
            max_concurrent_requests: 4,
            pairing_ttl_seconds: 300,
            max_pairing_attempts: 5,
        }
    }
}

/// Default directory exclusions for project file packaging (user-adjustable in UI).
pub fn default_project_excluded_dirs() -> Vec<String> {
    vec![
        "node_modules".to_string(),
        "target".to_string(),
        "dist".to_string(),
        "build".to_string(),
        ".next".to_string(),
        ".nuxt".to_string(),
        ".svelte-kit".to_string(),
        "__pycache__".to_string(),
        ".pytest_cache".to_string(),
        ".mypy_cache".to_string(),
        ".ruff_cache".to_string(),
        ".venv".to_string(),
        "venv".to_string(),
        ".gradle".to_string(),
        "Pods".to_string(),
        "DerivedData".to_string(),
    ]
}
