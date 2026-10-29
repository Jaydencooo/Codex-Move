use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

const MAX_DIAGNOSTIC_ENTRIES: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticLogEntry {
    pub timestamp: String,
    pub level: String,
    pub module: String,
    pub message: String,
}

static DIAGNOSTIC_BUFFER: OnceLock<Mutex<VecDeque<DiagnosticLogEntry>>> = OnceLock::new();

fn buffer() -> &'static Mutex<VecDeque<DiagnosticLogEntry>> {
    DIAGNOSTIC_BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_DIAGNOSTIC_ENTRIES)))
}

pub fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .try_init();
}

pub fn record_diagnostic(level: &str, module: &str, message: &str) {
    let redacted = redact_sensitive_text(message);
    if let Ok(mut guard) = buffer().lock() {
        if guard.len() >= MAX_DIAGNOSTIC_ENTRIES {
            guard.pop_front();
        }
        guard.push_back(DiagnosticLogEntry {
            timestamp: Utc::now().to_rfc3339(),
            level: level.to_string(),
            module: module.to_string(),
            message: redacted,
        });
    }
}

pub fn export_redacted_diagnostics() -> Vec<DiagnosticLogEntry> {
    if let Ok(guard) = buffer().lock() {
        guard.iter().cloned().collect()
    } else {
        Vec::new()
    }
}

/// Redacts API keys, bearer tokens, private keys, and sensitive credential strings
/// from logs, error messages, and diagnostic reports.
pub fn redact_sensitive_text(input: &str) -> String {
    let mut out = input.to_string();

    // Redact sk-... tokens (OpenAI / Anthropic style keys)
    out = redact_prefixed_token(&out, "sk-");
    out = redact_prefixed_token(&out, "ghp_");
    out = redact_prefixed_token(&out, "github_pat_");
    out = redact_prefixed_token(&out, "xoxb-");
    out = redact_prefixed_token(&out, "eyJ"); // JWTs

    // Redact key=value or key: "value" for known secret field names
    let secret_keys = [
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "id_token",
        "client_secret",
        "password",
        "secret",
        "authorization",
        "bearer",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
    ];

    for key in secret_keys {
        out = redact_key_value_assignments(&out, key);
    }

    if out.contains("-----BEGIN ") && out.contains("PRIVATE KEY-----") {
        return "[REDACTED_PRIVATE_KEY_BLOCK]".to_string();
    }

    out
}

fn redact_prefixed_token(text: &str, prefix: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find(prefix) {
        result.push_str(&rest[..idx]);
        let after = &rest[idx + prefix.len()..];
        let token_len = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .map(|c| c.len_utf8())
            .sum::<usize>();
        if token_len >= 8 {
            result.push_str(prefix);
            result.push_str("[REDACTED]");
            rest = &after[token_len..];
        } else {
            result.push_str(prefix);
            rest = after;
        }
    }
    result.push_str(rest);
    result
}

fn redact_key_value_assignments(text: &str, key: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = key.to_ascii_lowercase();
    if !lower.contains(&needle) {
        return text.to_string();
    }

    let mut lines = Vec::new();
    for line in text.lines() {
        let line_lower = line.to_ascii_lowercase();
        if line_lower.contains(&needle) && (line.contains('=') || line.contains(':')) {
            if let Some(eq_idx) = line.find('=') {
                lines.push(format!("{}= \"[REDACTED]\"", &line[..eq_idx].trim_end()));
                continue;
            } else if let Some(col_idx) = line.find(':') {
                lines.push(format!("{}: \"[REDACTED]\"", &line[..col_idx].trim_end()));
                continue;
            }
        }
        lines.push(line.to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redacts_api_keys_and_jwts() {
        let raw = "Failed with key sk-proj-1234567890abcdefABCDEF and token eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payload";
        let redacted = redact_sensitive_text(raw);
        assert!(!redacted.contains("1234567890abcdefABCDEF"));
        assert!(!redacted.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
        assert!(redacted.contains("sk-[REDACTED]"));
        assert!(redacted.contains("eyJ[REDACTED]"));
    }

    #[test]
    fn test_redacts_key_value_lines() {
        let raw = "model = \"gpt-5\"\nOPENAI_API_KEY = \"super-secret-value\"\n";
        let redacted = redact_sensitive_text(raw);
        assert!(redacted.contains("model = \"gpt-5\""));
        assert!(!redacted.contains("super-secret-value"));
        assert!(redacted.contains("[REDACTED]"));
    }
}
