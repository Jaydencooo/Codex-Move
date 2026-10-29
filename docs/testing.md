# 测试与验收策略 (Testing & Verification Strategy)

## 1. 测试铁律

1. **严禁触碰开发者真实 `.codex`**：所有单元测试、集成测试与故障注入测试均在 `tempfile::TempDir` 创建的隔离沙盒目录中执行，通过显式传入 `CodexHomeOverride` 注入脱敏 Fixture，绝不读取或修改宿主机的 `%USERPROFILE%\.codex` 或 `~/.codex`。
2. **测试真实行为而非复述实现**：每个测试必须验证实际生成的 SQLite 行记录、转换后的 JSONL/TOML 文件内容、磁盘字节哈希、回滚后的字节级一致性以及网络套接字通信结果。

---

## 2. 测试覆盖矩阵

| 测试领域 | 覆盖场景 | 对应测试模块 |
| :--- | :--- | :--- |
| **跨平台路径映射** | Windows (`C:\Users\Alice\...`, UNC `\\nas\share\...`) ↔ macOS (`/Users/bob/...`)；最长路径前缀匹配；严格路径组件边界（防止 `/Users/alice/app` 错误替换 `/Users/alice/app-v2`） | `paths::tests`, `tests/integration/path_mapping_test.rs` |
| **特殊字符与文件系统约束** | 中文、空格、Emoji（`🚀`）、超长路径（>260 字符）、Windows 保留名（`CON`, `NUL`, `COM1`）、非法字符（`: * ? " < > |`）、尾随点/空格、大小写冲突（`Readme.md` vs `README.md`）、Unicode NFC/NFD 冲突 | `paths::tests` |
| **Git 与符号链接** | 标准 `.git` 仓库、未提交修改、未跟踪文件、重要被忽略文件（`.env`）、内部相对 Symlink、越界外部 Symlink 阻断、失效 Symlink、Git Worktree（外部 `.git` 文件指针识别与告警）、Git Submodule | `inventory::tests`, `archive::tests` |
| **Codex 多版本与异常数据** | `state_4.sqlite` 与 `state_5.sqlite` 兼容读取；未知 `state_99.sqlite` 拒绝盲目写入；损坏或截断的 `rollout-*.jsonl` 隔离处理；SQLite WAL 模式未归档日志检查 | `adapters::codex::tests` |
| **合并、去重与分叉保护** | 目标端已有 `.codex` 数据时的增量合并；完全相同会话自动去重；同 ID 但内容分叉会话（两边都有新消息）保留双方数据并生成分叉副本，绝不按时间戳丢弃；`config.toml` 字段级合并与目标机凭据保护 | `planner::tests`, `migration::tests` |
| **局域网配对与安全传输** | 双实例回环/局域网发现、TLS 证书指纹固定（拒绝伪造证书）、过期/错误配对码拦截与退避、中途断网后基于 `(migration_id, manifest_hash)` 的分块断点续传、拒绝混用不同快照 | `pairing::tests`, `transport::tests` |
| **故障注入与崩溃恢复** | 模拟磁盘空间不足、目标文件只读权限错误、用户中途取消、Apply 阶段写完 SQLite 后但在写完项目文件前崩溃；验证重启后自动识别未完成事务日志并完整回滚恢复目标原始字节 | `recovery::tests`, `tests/failure-injection/` |
| **恶意输入安全防御** | 包含 `../../evil` 的恶意清单、绝对路径注入、超出声明大小的解压流（Decompression Bomb）、包含 SQL 注入载荷的会话标题与项目名 | `archive::tests`, `adapters::codex::tests` |
| **大文件有界内存流式处理** | 使用 4 MiB 固定分块流式传输与哈希计算，验证多百 MB / GB 级大文件流式处理时不发生整包加载内存膨胀 | `transport::tests`, `archive::tests` |

---

## 3. 执行命令

### Rust 后端单元测试与集成测试
```bash
cd src-tauri
cargo test --all-targets
```

### 前端严格类型检查与组件测试
```bash
npm run typecheck
npm test
```
