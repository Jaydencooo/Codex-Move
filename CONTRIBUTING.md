# Contributing to Codex Move (贡献指南)

Thank you for your interest in contributing to **Codex Move (Codex 搬家)**!

## Core Engineering Rules (核心开发准则)

1. **Never Touch Real `.codex` in Tests**:
   - All unit, integration, and fault-injection tests must use `tempfile::TempDir` sandboxes and synthetic/redacted fixtures.
   - Never read or mutate the developer's actual `~/.codex` or `%USERPROFILE%\.codex` directory in automated tests.
2. **No Silent Data Loss or Blind Overwrites**:
   - Never overwrite a target `.codex` folder wholesale.
   - When merging threads with the same ID that have diverged on both computers, preserve both branches rather than discarding one by timestamp.
3. **Never Guess Unknown SQLite Schemas**:
   - If a future Codex version introduces an unrecognized `state_N.sqlite` schema, the adapter must fall back to read-only inspection and raw backup rather than guessing SQL writes.
4. **Zero Automatic Code Execution**:
   - Never execute scripts, binaries, hooks, or MCP servers contained inside a migration package.

## Pull Request Checklist

```bash
# 1. Frontend type check and tests
npm run typecheck
npm test

# 2. Rust formatting, linting, and full test suite
cd src-tauri
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```
