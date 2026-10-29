# Security Policy (安全策略)

## Supported Versions (受支持版本)

| Version | Supported |
| :--- | :--- |
| `0.1.x` | Yes |

## Security Design Highlights (核心安全机制)

1. **Zero Cloud / Peer-to-Peer LAN Only**: Codex Move never uploads your chat history, `.env` files, or source code to any cloud server.
2. **Mandatory Certificate Fingerprint Pinning & Pairing**:
   - Every receiver session generates an ephemeral X.509 certificate and SHA-256 fingerprint.
   - Connections require either a 256-bit high-entropy token (via QR/Connection Code) or a 6-character pairing code stretched via **Argon2id** bound to the certificate fingerprint and a 128-bit salt, with strict rate limiting (5 attempts max) and exponential backoff.
   - Codex Move **never** uses "ignore TLS certificate errors" as a trust policy.
3. **Untrusted Peer Input Hardening**:
   - Path traversal (`..`), drive injection (`C:`), and external symbolic links are strictly blocked.
   - SQLite operations use parameterized queries exclusively.
   - Imported hooks, MCP servers, Skill scripts, and Automations are **never** executed automatically; Automations are restored in `paused` state.
4. **Credential Redaction**:
   - `auth.json` and device installation IDs are excluded by default.
   - Logs and diagnostic exports automatically redact API keys (`sk-*`, `ghp_*`, JWTs) and never log chat contents.

## Reporting a Vulnerability (报告安全漏洞)

Please do **not** open a public GitHub issue for sensitive security vulnerabilities. Instead, use GitHub Private Vulnerability Reporting on the repository's **Security -> Advisories** tab, including reproduction steps and affected OS versions.
