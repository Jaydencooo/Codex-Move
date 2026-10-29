# 威胁模型与安全设计 (Threat Model & Security Architecture)

## 1. 信任边界与核心假设

1. **局域网环境不可信 (Untrusted LAN)**：
   - 用户可能处于公共 Wi-Fi、共享办公网或存在恶意节点的局域网中。
   - **对策**：UDP 组播发现包绝不包含用户名、项目路径、聊天标题或任何令牌；组播发现结果仅作为“候选网络地址”，绝不直接视为可信身份。
2. **对端输入完全不可信 (Untrusted Peer Input)**：
   - 即使完成了配对，接收端也必须将发送端发来的 `MigrationManifest`、文件流、SQLite 数据库、JSONL 和 TOML 视为潜在恶意或损坏输入。
   - **对策**：全链路实施边界检查、路径净化、解压炸弹防御、参数化 SQL 查询与禁止代码执行策略。

---

## 2. 威胁分类与防御措施 (STRIDE 分析)

### 2.1 身份伪造与中间人攻击 (Spoofing & MitM)
- **风险**：局域网攻击者伪造接收端响应，或拦截自签名 HTTPS 流量窃取用户项目源码与密钥。
- **防御**：
  - 每次启动传输服务时使用 `rcgen` 生成全新 2048/4096-bit 或 Ed25519/ECDSA P-256 临时自签名证书。
  - **禁止忽略 TLS 证书错误**：客户端使用自定义 `rustls::client::danger::ServerCertVerifier`，严格校验服务端证书的 SHA-256 指纹必须与配对阶段绑定的 `peer_cert_fingerprint` 完全一致（Certificate Pinning）。
  - **配对认证与防爆破**：
    - 模式 A（二维码 / 高熵连接码）：包含 256-bit 随机令牌 + 服务端证书 SHA-256 指纹 + 局域网地址。
    - 模式 B（短配对码机制）：结合 X25519 临时公钥交换与 Argon2id/HKDF-SHA256 密钥派生，绑定双方 TLS 证书指纹生成挑战认证标签，绝不将短数字直接作为对称密钥。
    - 强制 5 分钟过期时间、最多 5 次尝试限制及指数退避（Exponential Backoff），并在接收端界面强制要求人工点击“确认接收来自 [设备名] 的迁移请求”。

### 2.2 路径穿越与越界文件写入 (Path Traversal & Arbitrary File Write)
- **风险**：恶意构造的归档清单中包含 `../../.ssh/authorized_keys`、`C:\Windows\System32\...` 或指向项目外部的符号链接，企图覆盖系统关键文件。
- **防御**：
  - `paths::sanitize_relative_path` 拒绝任何包含 `..`（ParentDir）、根路径（`/` 或 `\`)、Windows 盘符前缀（`C:`）或 UNC 前缀（`\\`）的相对路径。
  - 所有接收文件先写入与正式目录隔离的暂存目录（Staging Area），校验全部通过后才按白名单映射表写入目标 `.codex` 或用户选定的目标项目根目录。
  - **符号链接防御**：扫描阶段将符号链接分类为 `InternalRelative`（指向同一项目内部）、`ExternalTarget`（指向项目外）和 `Broken`（失效）。默认严禁跟随外部符号链接复制目录外数据，解压时也严禁创建指向目标根目录之外的符号链接。

### 2.3 解压炸弹与资源耗尽 (Decompression Bomb & DoS)
- **风险**：发送端发送极小但解压后达数百 GB 的 `.jsonl.zst` 文件，或发送包含数百万个微小文件的清单耗尽内存与句柄。
- **防御**：
  - 集中式安全限制配置 (`SecurityLimits`)：
    - 最大清单 JSON 大小：`32 MiB`
    - 单次迁移最大文件数：`250,000`
    - 单个配置文件/元数据文件最大解压体积：`64 MiB`
    - 流式分块大小：`4 MiB`（内存有界缓冲，禁止整包读入内存）
    - 最大并发请求数：`4`
  - 解压流使用 `std::io::Read::take(max_allowed_bytes)` 包装，一旦实际解压字节数超过清单声明大小或安全上限，立即中断并清理暂存区。

### 2.4 SQL 注入与恶意配置执行 (SQL Injection & RCE via Config/Hooks)
- **风险**：
  1. 恶意会话标题或路径中包含 SQL 语句，企图在合并 `state_5.sqlite` 时执行任意 SQL。
  2. 迁移的 `hooks.json`、`config.toml`（`[mcp_servers]` / `notify`）、`automations/*.toml` 或 Skills 脚本包含恶意命令，在导入时自动执行。
- **防御**：
  - 所有 `rusqlite` 操作 100% 使用参数化预编译语句（`params![...]`），绝不拼接 SQL 字符串；表名/列名检查均通过静态白名单验证。
  - **零自动执行原则**：Codex Move 在扫描、传输、导入与验证全过程中**绝不调用任何迁移包内的脚本、二进制、Hook 或 MCP 命令**。
  - 导入 `automations/*.toml` 时强制将状态改写为 `paused`；导入 `hooks.json` 与 `config.toml` 中的 `[mcp_servers]` 时，默认将其隔离或标记为“待用户人工审查后启用”，并在完成页醒目列出其中的命令行内容。

### 2.5 凭据泄露与日志污染 (Credential Leakage)
- **风险**：用户 `auth.json`、`.env`、`config.toml` 中的 API Key 或私钥在迁移日志、错误提示或诊断报告中明文泄露。
- **防御**：
  - 默认排除 `auth.json`、`installation_id`、`cap_sid`、`.sandbox-secrets`。
  - 内置敏感内容扫描器（识别 `.env*`、`*.pem`、`*.key`、`id_rsa`、`id_ed25519` 以及 `sk-*`、`OPENAI_API_KEY`、`ANTHROPIC_API_KEY`、`GITHUB_TOKEN` 等高危模式），在界面上单独标记并由用户决定是否包含。
  - 日志与诊断报告导出器内置 `redact_sensitive_text()`，自动遮蔽所有路径中的用户名、所有疑似 Token/Key 字符串，且绝不记录任何会话聊天正文。
