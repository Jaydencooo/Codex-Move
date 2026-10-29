# Codex Move (Codex 搬家)

**English** | [简体中文](./README.zh-CN.md)

**Codex Move** is a lightweight, production-grade cross-platform desktop application that directly migrates your **Codex chat history, Skills, global instructions, configuration, and project folders** between two computers (**Windows ↔ macOS**, **Windows ↔ Windows**, **macOS ↔ macOS**) over your local network (LAN) — with zero command-line setup and zero cloud intermediaries.

> **Non-Affiliation Notice**: Codex Move is an independent open-source community project and is not affiliated with, endorsed by, or sponsored by OpenAI. No official OpenAI logos or trademarks are used in this application.

---

## 1. Which Installer Should I Download?

Download and open Codex Move on **both** your old computer and your new computer from the **Releases** page:

| Operating System | Architecture | Installer to Download | Notes |
| :--- | :--- | :--- | :--- |
| **Windows 10 / 11** | 64-bit (`x86_64`) | `Codex-Move_x.y.z_x64-setup.exe` or `.msi` | Windows 10 (1803+) and Windows 11 include the Microsoft Edge WebView2 runtime by default. On stripped-down offline systems without WebView2, the installer displays clear guidance. |
| **macOS (Apple Silicon)** | M1 / M2 / M3 / M4 (`aarch64`) | `Codex-Move_x.y.z_aarch64.dmg` | For all Macs powered by Apple Silicon |
| **macOS (Intel)** | Intel (`x86_64`) | `Codex-Move_x.y.z_x64.dmg` | For Intel-based Macs |

---

## 2. How Two Computers Connect and Migrate

The entire migration takes 6 guided steps:

1. **Choose Role on Each Computer**:
   - On your **old computer**, click **"Send from this computer"** (strictly read-only — your old computer's files are never deleted or modified).
   - On your **new computer**, click **"Migrate to this computer"**.
2. **Pair and Verify Identity**:
   - Your new computer displays a one-time **QR / Secure Connection Code** and a **6-character pairing code** protected by Argon2id key stretching and bound to the ephemeral TLS certificate's SHA-256 fingerprint.
   - Your old computer automatically discovers the new computer on the same LAN. Select it, enter the 6-character code (or paste the connection string), and click **"Approve & Allow Transfer"** on the new computer.
3. **Select Content to Migrate**:
   - Choose which categories to include (Active & Archived Chats, Skills, `AGENTS.md`, Core Config, Memories/Automations, Project Folders, etc.).
4. **Choose Project Locations on the New Computer**:
   - By default, projects are placed cleanly under a unified folder on the new computer (e.g., `~/CodexProjects/<project-name>`), or you can customize individual project paths or bind chat history to an existing project directory.
   - Structured workspace paths (`cwd`, `workspace_roots`, project roots) are remapped across Windows and macOS without mutating your chat message text.
5. **Review Summary & Start Migration**:
   - Inspect disk space pre-checks, path conflict checks, and chat merge plans. If the new computer already has Codex data, identical transcripts are deduplicated by verified content fingerprint, and divergent chats sharing an ID are preserved side-by-side under a new valid UUID.
6. **Review Verification Report**:
   - Automatically verifies target SQLite database integrity (`PRAGMA integrity_check`), chat rollout `.jsonl` parity, target project folders, and zero unmapped old paths.

---

## 3. Why Both Computers Need to Be on the Same LAN

- **Privacy & Zero Cloud Relay**: Your chat transcripts and source code repositories stay strictly between your two machines. Data is streamed directly peer-to-peer over TLS with pinned certificate fingerprints, authenticated pairing tokens, and per-chunk SHA-256 verification — no accounts, no cloud uploads, and no third-party servers.
- **High Speed & Resumable Streaming**: Direct LAN transfer saturates local Wi-Fi or Gigabit Ethernet and supports bounded 4 MiB chunked streaming with automatic byte-offset resume if Wi-Fi momentarily drops.

---

## 4. What Migrates Automatically vs. What Needs Re-Login or Reinstall

Codex Move categorizes all detected items and clearly labels their disposition before transfer:

| Category | Disposition Badge | Behavior |
| :--- | :--- | :--- |
| **Chat History (Active & Archived)** | `Needs Path Mapping` | Migrates `sessions/` and `archived_sessions/` `.jsonl` transcripts; preserves message bodies verbatim while remapping structured `cwd` and `workspace_roots` fields |
| **Chat Metadata & State Databases** | `Needs Path Mapping` | Structurally merges `state_5.sqlite` (and `state_4.sqlite`), `thread_history_1.sqlite`, `session_index.jsonl`, and `.codex-global-state.json` |
| **User Skills (`~/.codex/skills`)** | `Direct Migration` | Copies custom Skills (excluding internal `.system` cache) and flags external runtime dependencies (`python`, `node`, `uv`, `brew`) |
| **Shared Skills (`~/.agents/skills`)** | `Direct Migration` | Copies shared cross-agent Skills |
| **Global Instructions (`AGENTS.md`)** | `Direct Migration` | Copies `AGENTS.md` and `AGENTS.override.md` |
| **Core Config (`config.toml`)** | `Needs Path Mapping` | Merges model preferences, approval policies, project trust entries, and MCP server definitions with mapped paths |
| **Memories, Rules, Hooks & Automations** | `Needs Path Mapping` | Migrates `memories/`, `rules/`, `hooks.json`, `automations/`, and `goals_1.sqlite`. **All scheduled automations and active goals are imported in `Paused` state** so they never execute unverified commands on the new OS before your review |
| **Chat Attachments (`attachments/`)** | `Direct Migration` | Copies attached images and artifacts |
| **Project Folders** | `Needs Path Mapping` | Copies git repositories (including uncommitted changes and untracked files) while excluding rebuildable caches (`node_modules`, `target`, `dist`, `.venv`) by default; sensitive files (`.env`, private keys) require explicit opt-in |
| **Account Auth & Native Plugin Binaries** | `Re-login / Reinstall on Target` | **`auth.json` and OS keychain credentials are never copied across computers** (sign in via the official Codex login flow on the new computer); platform-specific `.sandbox-bin` and plugin binaries are reinstalled on target |

---

## 5. Troubleshooting Connection Failures

If automatic discovery does not list your new computer:

1. **Check Guest Wi-Fi / AP Isolation**: Ensure both computers are connected to the same local network and not isolated by a Guest Wi-Fi network or router AP isolation.
2. **Check OS Firewall Prompts**: Allow `codex-move` through Windows Defender Firewall or macOS Application Firewall when prompted on Step 1.
3. **Use Manual Connection Code**: Copy the full `cdxmove://...` connection string displayed on the new computer (which embeds the LAN IP, port, and TLS certificate fingerprint) and paste it directly into the old computer's input field.
4. **Offline Fallback — Encrypted Migration Bundle (`.cdxmove`)**:
   - If the two computers cannot reach each other over the network at all, expand **"Codex Directory & Advanced Fallback Options"** on the home screen.
   - Export an Argon2id + ChaCha20-Poly1305 encrypted `.cdxmove` bundle to an external USB drive on the old computer, and import it on the new computer using the same guided path-mapping and verification wizard.

---

## 6. How to Roll Back

Codex Move never overwrites `.codex` directly without a verified backup:

- **Automatic Rollback on Mid-Import Failure**: If an unexpected error or power loss occurs while applying changes, Codex Move uses its persistent `RecoveryJournal` to automatically restore modified files from SHA-256 verified backups and remove newly created files.
- **One-Click Manual Rollback After Migration**:
  1. Open Codex Move on your new computer.
  2. Under **"Recovery & Rollback Previous Migrations"** on the home screen, click **"Roll Back to Pre-Migration State"**.
  3. Confirm in the dialog to restore your pre-migration `.codex` files and verify every restored file's SHA-256 digest.

---

## 7. Verified Codex Versions & Supported Platforms

See [`docs/compatibility.md`](./docs/compatibility.md) for the full compatibility matrix:

- **Verified Codex Storage Layout**:
  - Primary State Databases: `state_5.sqlite` (migrations `v1`–`v38`) and `state_4.sqlite`.
  - Auxiliary Databases: `thread_history_1.sqlite`, `memories_1.sqlite`, `goals_1.sqlite`.
  - Transcript & Config Files: `sessions/**/*.jsonl`, `archived_sessions/**/*.jsonl`, `session_index.jsonl`, `.codex-global-state.json`, `config.toml`, `AGENTS.md`, `skills/`, `automations/`.
- **Forward-Compatibility Guard**:
  - If a future Codex release introduces an unknown database schema (e.g., `state_6.sqlite`), Codex Move detects it during inspection and blocks direct SQLite mutation rather than risking corruption.
- **Supported Platforms**:
  - Windows 10 (1803+) / Windows 11 (`x86_64`)
  - macOS 12 Monterey and newer (`aarch64` Apple Silicon & `x86_64` Intel)

---

## 8. Documentation & License

- [Architecture Overview (`docs/architecture.md`)](./docs/architecture.md)
- [Migration Format & Wire Protocol (`docs/migration-format.md`)](./docs/migration-format.md)
- [Compatibility Matrix (`docs/compatibility.md`)](./docs/compatibility.md)
- [Threat Model & Security Design (`docs/threat-model.md`)](./docs/threat-model.md)
- [Testing & Fault Injection Guide (`docs/testing.md`)](./docs/testing.md)
- [Release & Signing Guide (`docs/release.md`)](./docs/release.md)
- [Contributing (`CONTRIBUTING.md`)](./CONTRIBUTING.md) · [Security Policy (`SECURITY.md`)](./SECURITY.md)

Licensed under the [Apache License 2.0](./LICENSE). See [NOTICE](./NOTICE) for protocol attribution details.
