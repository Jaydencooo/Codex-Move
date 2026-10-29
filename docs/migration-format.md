# 迁移数据格式与网络协议规范 (Migration Format & Network Protocol)

## 1. 版本化原则

Codex Move 将 **迁移清单与归档格式 (`format_version`)** 与 **局域网传输协议 (`protocol_version`)** 严格解耦并独立版本化：
- 当前迁移数据格式版本：`format_version = "1.0.0"`
- 当前局域网传输协议版本：`protocol_version = "1.0.0"`

在建立连接后、传输任何实质文件之前，发送端与接收端必须通过 `/api/codex-move/v1/capabilities` 交换能力声明。若版本不兼容或目标端 Codex 适配器不支持源端数据 schema，立即中止并给出明确升级或兼容提示。

---

## 2. 迁移清单结构 (`MigrationManifest`)

清单文件名为 `manifest.json`，采用 UTF-8 编码，结构如下：

```json
{
  "format_version": "1.0.0",
  "migration_id": "019a30f0-8b2c-7d10-9a11-4f829012abcd",
  "app_version": "0.1.0",
  "created_at": "2026-10-29T08:50:00Z",
  "source_env": {
    "os": "windows",
    "arch": "x86_64",
    "hostname_redacted": "DESKTOP-SOURCE",
    "codex_home": "C:\\Users\\alice\\.codex",
    "detected_codex_version": "0.130.0",
    "sqlite_schemas": {
      "state_db_filename": "state_5.sqlite",
      "state_migration_version": 38,
      "thread_history_db_filename": "thread_history_1.sqlite",
      "thread_history_migration_version": 4,
      "memories_db_filename": "memories_1.sqlite",
      "goals_db_filename": "goals_1.sqlite"
    }
  },
  "adapter_capabilities": {
    "adapter_id": "codex-v1-state5",
    "can_read_threads": true,
    "can_write_threads": true,
    "can_map_paths": true,
    "supported_state_versions": [4, 5],
    "unknown_tables": []
  },
  "categories": [
    {
      "category": "ActiveAndArchivedChats",
      "disposition": "NeedsPathMapping",
      "item_count": 42,
      "total_bytes": 18452910
    }
  ],
  "projects": [
    {
      "project_id": "5a5011b1-92bd-4752-9ca6-1232290a40d7",
      "name": "my-service",
      "source_root_paths": ["C:\\Users\\alice\\projects\\my-service"],
      "include_project_files": true,
      "git_status": "StandardRepo",
      "has_uncommitted_changes": true,
      "has_untracked_files": true,
      "sensitive_files": [".env"]
    }
  ],
  "files": [
    {
      "entry_id": "f-0001",
      "category": "ActiveAndArchivedChats",
      "archive_rel_path": "codex_home/sessions/2026/10/29/rollout-2026-10-29T08-00-00-019a.jsonl",
      "logical_rel_path": "sessions/2026/10/29/rollout-2026-10-29T08-00-00-019a.jsonl",
      "project_id": "5a5011b1-92bd-4752-9ca6-1232290a40d7",
      "size_bytes": 45120,
      "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "permissions": {
        "readonly": false,
        "unix_mode": 420,
        "executable": false
      },
      "symlink": null,
      "disposition": "NeedsPathMapping"
    }
  ],
  "excluded_items": [
    {
      "logical_path": "auth.json",
      "reason": "CredentialExcludedByDefault"
    },
    {
      "logical_path": "logs_2.sqlite",
      "reason": "RuntimeDiagnosticCacheExcluded"
    }
  ],
  "warnings": []
}
```

### 2.1 十类迁移内容分类枚举 (`ContentCategory`)
1. `ActiveAndArchivedChats`：活跃会话（`sessions/**/*.jsonl`）与归档会话（`archived_sessions/**/*.jsonl.zst`）。
2. `ChatMetadataAndIndex`：`state_*.sqlite` 中的 `threads`/`projects`/`project_roots`/`thread_sections`、`thread_history_*.sqlite`、`session_index.jsonl` 及 `.codex-global-state.json` 中的项目与会话绑定关系。
3. `UserSkills`：`~/.codex/skills/` 下的用户自定义 Skills（默认跳过 `.system` 内置缓存，并对含二进制或平台脚本的 Skill 标记依赖提示）。
4. `SharedAgentSkills`：`~/.agents/skills/` 下的跨 Agent 共享 Skills。
5. `InstructionFiles`：全局 `~/.codex/AGENTS.md`、`AGENTS.override.md` 及项目级指令文件。
6. `CoreConfig`：`config.toml`（结构化解析，分离普通设置、模型配置、项目信任级别与敏感字段）。
7. `ExtensionsMemoriesRulesHooks`：`memories/`、`memories_*.sqlite`、`goals_*.sqlite`、`hooks.json`、`automations/`（定时任务导入后强制置为 `paused`/`disabled` 状态）。
8. `ChatAttachments`：`attachments/` 与 `generated_images/`。
9. `ProjectFiles`：用户勾选迁移的项目源码、文档、隐藏配置、未提交修改、未跟踪文件及 `.git` 仓库。
10. `PluginManifests`：`config.toml` 中的 `[plugins]` 与 `[marketplaces]` 声明清单（仅迁移声明以便目标端按需重装，绝不跨平台复制 `.plugin-appserver/*.exe` 等原生二进制）。

### 2.2 迁移处置标签 (`MigrationDisposition`)
- `DirectCopy`：可直接字节级迁移。
- `NeedsPathMapping`：需要结构化转换跨平台路径。
- `NeedsReinstallOnTarget`：需要在目标系统重新安装（如插件、MCP 本地服务、虚拟环境依赖）。
- `NeedsReauthOnTarget`：需要在目标系统重新登录认证（如 `auth.json`、OAuth 令牌）。
- `UnsupportedInCurrentVersion`：当前版本暂不支持（如未知的数据库大版本或外部绝对路径 Git worktree）。

---

## 3. 局域网发现与安全传输协议

### 3.1 灵感来源与协议差异声明（对比 LocalSend）
- **借鉴部分**：参考了 LocalSend Protocol v2（MIT License）使用的 UDP 组播地址 `224.0.0.167:53317`、设备发现后通过 HTTPS REST 端点握手确认以及利用 TLS 证书 SHA-256 指纹识别设备身份的思路。
- **独立设计与不兼容说明**：
  1. Codex Move 专为 Codex 结构化迁移设计，REST 路径前缀为 `/api/codex-move/v1/*`，**不与普通 LocalSend 客户端互通**。
  2. LocalSend 默认允许无 PIN 直接请求接收并忽略自签名证书校验；Codex Move **强制要求配对认证**（通过二维码一次性 256-bit 令牌或基于 X25519 + HKDF-SHA256 的安全连接码握手机制），且严格校验对端 TLS 证书 SHA-256 指纹（Certificate Pinning），绝不采用“忽略任意 TLS 证书错误”的策略。

### 3.2 协议端点与交互时序
1. **`GET /api/codex-move/v1/ping`**：返回接收端公开标识（随机生成的设备显示名、OS、架构、协议版本、TLS 证书指纹前缀）。
2. **`POST /api/codex-move/v1/pair/request`**：
   - 发送端提交经配对密钥（QR Token 或连接码派生密钥）签名的挑战响应（HMAC-SHA256 绑定双方 TLS 证书指纹 + 随机数 Nonce）与发送端设备信息。
   - 速率限制：最多允许 5 次失败尝试，每次失败触发指数退避（1s → 2s → 4s → 8s → 锁定重新生成配对码），有效期 5 分钟。
   - 接收端用户在界面看到发送端设备名、系统与将要迁移的摘要范围，点击“确认允许接收”后颁发会话 `session_token`。
3. **`POST /api/codex-move/v1/capabilities`**：交换 `CapabilitiesHandshake`（网络协议版本、迁移格式版本、源/目标 Codex 适配器版本、支持压缩算法 Zstd、最大分块大小、目标可用磁盘空间与文件系统大小写敏感度）。
4. **`POST /api/codex-move/v1/manifest`**：上传并校验 `MigrationManifest`，接收端记录 `(migration_id, manifest_sha256)`。
5. **`GET /api/codex-move/v1/transfer/status?migration_id=...&manifest_hash=...`**：查询已成功接收并校验通过的文件与分块偏移，用于断网重连后的断点续传；若 `migration_id` 或 `manifest_hash` 不匹配，立即拒绝混用快照。
6. **`PUT /api/codex-move/v1/transfer/chunk`**：流式上传文件分块（默认 4 MiB 有界缓冲块），携带 `X-Migration-Id`、`X-Entry-Id`、`X-Chunk-Offset`、`X-Chunk-Sha256`，接收端边收边校验并写入独立暂存区（Staging Area）。
7. **`POST /api/codex-move/v1/transfer/complete`**：通知接收端所有文件块传输完毕，接收端执行整包清单二次哈希核验，随后关闭局域网监听端口并进入本地导入阶段。
