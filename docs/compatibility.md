# Codex 兼容性说明与非官方接口声明 (Compatibility Matrix & Disclaimer)

> [!IMPORTANT]
> **独立社区工具声明**：Codex Move 是独立的开源社区工具，与 OpenAI 没有任何隶属、合作或官方背书关系。
> 本工具依赖的 `.codex/` 本地目录结构、SQLite 表结构（如 `state_5.sqlite`）和 JSONL 会话日志属于 OpenAI Codex 客户端的内部持久化实现，并非官方承诺永久不变的公共 API。因此，Codex Move 采用严格的**版本化适配器（CodexAdapter）+ Schema 签名校验**机制：遇到未验证的新版本数据库结构时，默认只提供只读扫描与原始备份，绝不盲目猜测写入。

---

## 1. 已验证与受支持的平台及数据格式

### 1.1 操作系统矩阵
| 迁移方向 | 路径转换支持 | 大小写与保留名检查 | 符号链接与权限处理 | 验证状态 |
| :--- | :--- | :--- | :--- | :--- |
| **Windows → Windows** | 支持（不同用户名、不同盘符 `C:\` → `D:\`、UNC 路径） | NTFS 大小写不敏感冲突、Windows 保留设备名检查 | 检测 Junction/Symlink，无权限时支持安全降级选项 | 已通过本地集成测试与端到端引擎验证 |
| **Windows → macOS** | 支持（`C:\Users\Alice\...` → `/Users/bob/...`，分隔符与盘符归一化） | NTFS → APFS（默认大小写不敏感但保留大小写，支持 NFC/NFD 归一化检测） | 恢复 POSIX 权限位，映射内部相对链接 | 已通过跨平台 Fixture 集成测试验证（跨物理机 GUI 待双机实测） |
| **macOS → Windows** | 支持（`/Users/alice/...` → `D:\Projects\...`） | 检测 POSIX 下合法但在 Windows 下非法的字符（`: * ? " < > |`）、尾随点/空格及大小写同名文件冲突 | 外部链接阻断，内部链接按 Windows 开发者模式能力提示转换 | 已通过跨平台 Fixture 集成测试验证（跨物理机 GUI 待双机实测） |
| **macOS → macOS** | 支持（不同用户名或自定义项目根目录） | APFS NFC/NFD Unicode 归一化冲突检测 | 保留 `unix_mode` 与可执行位（`0755`/`0644`） | 已通过跨平台 Fixture 集成测试验证 |

---

## 2. Codex 存储组件兼容性矩阵

基于对 OpenAI Codex（`codex-rs` / Codex CLI `0.110.x ~ 0.130.x+` 及 Codex Desktop 2026 版）实际本地目录结构的核实：

| 组件 / 文件 | 识别规则 | 迁移与路径转换策略 | 未知 Schema 降级行为 |
| :--- | :--- | :--- | :--- |
| **`state_4.sqlite` / `state_5.sqlite`** | 动态扫描 `state_*.sqlite`（及 `sqlite_home` 自定义位置），检查 `_sqlx_migrations` 与 `threads`、`projects`、`project_roots`、`thread_sections`、`thread_attachments`、`thread_spawn_edges` 表结构 | 合并写入目标库；重写 `threads.rollout_path`、`threads.cwd`、`project_roots.path` 及 `thread_attachments.payload`（`root`/`workspaceRoot`/`sourceCwd`）；同 ID 分叉会话自动生成带分支后缀的新会话副本并保留双方记录 | 若发现 `state_6.sqlite` 或缺失核心列，拒绝直接 SQL 篡改，仅允许原始文件备份并导出脱敏诊断报告 |
| **`thread_history_1.sqlite`** | 检查 `thread_turns`、`thread_items`、`thread_history_projection_state` | 按已迁移的 `thread_id` 增量合并行记录；保持 `item_json` 原始对话与工具输出内容不变 | 若表结构不匹配，跳过投影库写入（Codex 启动时可从 `sessions/*.jsonl` 重建或回退读取）并明确提示 |
| **`memories_1.sqlite` & `memories/`** | 检查 `stage1_outputs` 表及 `memories/MEMORY.md`、`raw_memories.md`、`rollout_summaries/` | 增量合并 `stage1_outputs`（按 `thread_id` 去重），合并 Markdown 记忆文件 | 未知表结构时仅迁移 Markdown 记忆文件并保留原库备份 |
| **`goals_1.sqlite`** | 检查 `thread_goals`、`thread_goal_continuation_deferrals` | 按 `thread_id` 合并目标记录；为防止换机后后台目标任务意外自动续跑，将 `active` 状态安全标记为 `paused` | 未知版本时只读跳过并提示 |
| **`sessions/**/*.jsonl` & `archived_sessions/**/*.jsonl.zst`** | 解析首行 `type == "session_meta"` | 保持对话历史正文、命令记录与 `response_item` 原文不变；结构化更新首行 `session_meta.payload.cwd` 与 `runtime_workspace_roots`，以及 `turn_context.cwd` / `workspace_roots` | 若首行非合法 JSON 或截断，标记为“损坏或截断的会话日志”，隔离跳过或按原样只读归档，不中断其余会话迁移 |
| **`session_index.jsonl`** | 逐行解析 `{"id", "thread_name", "updated_at"}` | 合并追加至目标 `session_index.jsonl`，保留最新自定义会话标题 | 格式异常行隔离跳过 |
| **`.codex-global-state.json`** | 解析 `local-projects`、`thread-project-assignments`、`thread-writable-roots`、`sidebar-project-thread-orders`、`pinned-project-ids`、`pinned-thread-ids` | 合并项目定义与会话绑定关系，重写项目 `rootPaths` 与 `thread-writable-roots`；**排除**窗口坐标（`electron-main-window-bounds`）、设备安装 ID（`electron-local-remote-control-installation-id`）等本机专属状态 | 仅更新已识别的安全字段，保留目标机其余字段不变 |
| **`config.toml`** | `toml_edit` 无损 AST 解析 | 合并常规模型与偏好设置；将 `[projects."<old_path>"]` 键名转换为新机器项目路径；保护目标机认证设置；检测 `[mcp_servers]` 中的本机命令路径与环境变量密钥并提示用户 | 语法损坏时阻断配置合并并展示具体行号 |
| **`automations/**/*.toml`** | 解析 `version`, `id`, `status`, `cwds`, `target` | 转换 `cwds` 路径；**强制将 `status` 设为 `"paused"` / `"disabled"`**，防止未审核的定时任务在新电脑自动执行 | 原样复制但移除自动触发配置 |
| **`skills/` & `~/.agents/skills/`** | 扫描 `SKILL.md` 及附属脚本/二进制 | 完整迁移纯文本指令与脚本，扫描硬编码绝对路径、Python/Node/PowerShell 解释器依赖及平台二进制，在完成页生成“待检查依赖清单” | 不自动执行任何脚本 |
| **`plugins/`** | 读取 `config.toml` 中的 `[plugins]` 与 `[marketplaces]` | 仅迁移插件启用声明与源清单；**排除** `.plugin-appserver/*.exe` 等平台原生二进制与 `plugins/cache` 临时缓存，提示用户在 Codex 中自动重装 | 不复制二进制 |
| **默认排除项** | `auth.json`, `installation_id`, `cap_sid`, `logs_*.sqlite`, `queue_*.sqlite`, `.sandbox*`, `*-wal`, `*-shm`, `*.lock`, `*.tmp*` | 严格排除，绝不覆盖目标机登录态或拷贝数 GB 的运行日志与临时锁文件 | N/A |
