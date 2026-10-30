# Codex Move（Codex 搬家）

[English](./README.md) | **简体中文**

> **Alpha 开发状态**：当前为开发源码，尚未验证为稳定可用版本。前端构建及 4 个模拟流程／语言测试通过；本机缺少 MSVC 链接器，Rust 测试和安装包尚未验证。Windows／macOS 双机迁移、恢复和配对安全仍待验证。请仅使用可丢弃的数据副本测试，勿作为唯一备份。以下安装包名称为计划产物，不代表已有可下载版本。

**Codex Move（Codex 搬家）** 是一款处于实验阶段的轻量跨平台桌面工具，帮助普通用户在两台电脑之间（Windows ↔ macOS、Windows ↔ Windows、macOS ↔ macOS）通过局域网直接迁移 **Codex 聊天记录、Skills、全局指令、配置与项目文件夹**，无需使用命令行，也不依赖任何云端或中转服务器。

> **非官方项目声明**：Codex Move 是独立的开源社区工具，与 OpenAI 没有任何隶属、合作或官方背书关系。应用内不使用 OpenAI 官方标志。

---

## 1. 应该下载哪个安装包？

请前往本仓库的 **Releases** 页面，根据每台电脑的操作系统下载对应安装包（旧电脑与新电脑均需安装并打开 Codex Move）：

| 电脑系统 | 处理器架构 | 推荐下载文件 | 说明 |
| :--- | :--- | :--- | :--- |
| **Windows 10 / 11** | 64 位 (`x86_64`) | `Codex-Move_x.y.z_x64-setup.exe` 或 `.msi` | Windows 10 (1803+) / 11 默认自带 Microsoft Edge WebView2 运行时；如在极端精简版离线系统缺少 WebView2，应用会明确提示下载指引 |
| **macOS (Apple Silicon)** | M1 / M2 / M3 / M4 (`aarch64`) | `Codex-Move_x.y.z_aarch64.dmg` | 适用于所有搭载 Apple M 系列芯片的 Mac |
| **macOS (Intel)** | Intel (`x86_64`) | `Codex-Move_x.y.z_x64.dmg` | 适用于搭载 Intel 处理器的 Mac |

---

## 2. 两台电脑如何连接与迁移？

整个迁移过程共 6 步，全程图形化向导引导：

1. **打开应用并选择角色**：
   - 在**旧电脑**上点击 **「从这台电脑发送」**（应用只会以只读方式扫描并打包快照，绝不删除或改动旧电脑任何文件）。
   - 在**新电脑**上点击 **「迁移到这台电脑」**。
2. **连接两台电脑并确认身份**：
   - 新电脑界面会展示一次性 **连接二维码 / 安全连接码** 以及 **6 位配对码**（受 Argon2id 密钥派生与 TLS 证书 SHA-256 指纹绑定保护）。
   - 旧电脑会自动发现同一局域网内的新电脑；点击选择后输入 6 位配对码（或粘贴完整安全连接码），并在新电脑上点击 **「确认身份并允许接收」**。
3. **选择要迁移的内容**：
   - 勾选需要迁移的类别（聊天记录、Skills、全局指令 `AGENTS.md`、核心配置、扩展记忆与自动化任务、项目文件夹等）。
4. **选择新电脑上的项目存放位置**：
   - 默认在新电脑统一目录下按项目名自动创建文件夹（如 `~/CodexProjects/<项目名>`），也支持为单个项目自定义路径或绑定到新电脑已有同名项目目录。
   - 应用会自动修复跨平台路径（如将 `C:\Users\Alice\...` 映射为 `/Users/bob/...`），绝不对聊天正文做盲目字符串替换。
5. **查看迁移摘要并开始迁移**：
   - 展示空间预检、路径冲突检查与合并策略（新电脑已有 Codex 数据时自动按会话内容指纹去重；同 ID 但内容分叉的聊天自动生成新 UUID 并存保留，绝不互相覆盖）。
6. **查看验证结果**：
   - 自动检验新电脑 SQLite 数据库完整性（`PRAGMA integrity_check`）、聊天记录与 `.jsonl` 会话文件一致性、目标项目目录及旧路径残留情况。

---

## 3. 为什么需要两台电脑在同一局域网？

- **隐私与零云端中转**：你的聊天记录、代码仓库和配置可能包含敏感商业代码或私人对话。Codex Move 采用点对点（Peer-to-Peer）直连设计，所有数据仅在你的旧电脑与新电脑之间通过端到端加密的 HTTPS（TLS 证书指纹固定 + 256 位会话令牌 + 每分块 SHA-256 校验）直接传输，不经过任何第三方服务器，也无需注册账号。
- **高速传输与断点续传**：局域网直连可跑满本地 Wi-Fi 或千兆有线网络带宽，支持 4 MiB 流式分块与网线/Wi-Fi 闪断后从精确字节偏移量断点续传。

---

## 4. 哪些内容会自动迁移？哪些需要重新登录或重新安装？

Codex Move 将数据分为 10 个清晰类别，并在界面中明确标注每类数据的处理方式：

| 迁移类别 | 处理策略标签 | 详细说明 |
| :--- | :--- | :--- |
| **聊天记录（活动与归档）** | `需要路径映射` | 完整迁移 `sessions/` 与 `archived_sessions/` 下的 `.jsonl` 会话文件；保持对话正文、命令与输出原样，仅更新结构化工作区路径（`cwd`、`workspace_roots`） |
| **会话索引与状态数据库** | `需要路径映射` | 结构化合并 `state_5.sqlite`（及 `state_4.sqlite`）、`thread_history_1.sqlite`、`session_index.jsonl` 与 `.codex-global-state.json` |
| **用户 Skills (`~/.codex/skills`)** | `直接迁移` | 完整复制自定义 Skills（自动跳过内部 `.system` 缓存），并提示脚本所需的外部运行时（如 Python/Node/Brew） |
| **共享 Skills (`~/.agents/skills`)** | `直接迁移` | 完整迁移跨 Agent 共享 Skills 目录 |
| **全局指令 (`AGENTS.md`)** | `直接迁移` | 迁移 `AGENTS.md` 与 `AGENTS.override.md` |
| **核心配置 (`config.toml`)** | `需要路径映射` | 迁移模型偏好、审批策略、项目信任级别及 MCP Server 定义，自动映射项目路径 |
| **记忆、规则、Hooks 与自动化** | `需要路径映射` | 迁移 `memories/`、`rules/`、`hooks.json`、`automations/`、`goals_1.sqlite`；**为安全起见，所有自动化与定时任务导入后默认设为「已暂停 (Paused)」**，需用户检查路径后手动开启 |
| **会话附件 (`attachments/`)** | `直接迁移` | 迁移聊天关联的图片与本地附件 |
| **项目文件夹** | `需要路径映射` | 迁移工作区源码（包含未提交修改与未跟踪文件），默认排除 `node_modules`、`target`、`dist`、`.venv` 等可重建缓存；敏感文件（如 `.env`、私钥）默认不勾选，需用户显式确认 |
| **登录凭据与平台二进制插件** | `需在新电脑重新登录 / 重新安装` | **`auth.json` 与系统钥匙串凭据默认绝不跨机复制**（请在迁移后通过 Codex 官方登录流程重新登录）；`.sandbox-bin` 与平台原生插件二进制缓存会在新电脑按清单重新安装 |

---

## 5. 连接失败时如何处理？

如果旧电脑未能自动发现新电脑，或局域网连接受阻，请按以下顺序排查：

1. **检查 Wi-Fi / 路由器 AP 隔离**：确认两台电脑连接在同一个路由器网段下，且未开启“访客网络（Guest Wi-Fi）”或“AP 客户端隔离”。
2. **检查系统防火墙弹窗**：首次进入连接模式时，Windows Defender 防火墙或 macOS 防火墙可能会询问是否允许 `codex-move` 进行局域网通信，请勾选允许“专用网络/局域网”。
3. **使用手动 IP + 连接码直连**：即使 UDP 多播发现被路由器屏蔽，你也可以直接复制新电脑界面显示的 `cdxmove://...` 完整连接码（内含新电脑局域网 IP、端口与 TLS 指纹）粘贴到旧电脑输入框直接连接。
4. **终极兜底：加密迁移包导入/导出 (`.cdxmove`)**：
   - 如果两台电脑处于完全隔离的网络（如企业内网 VLAN 隔离），可在首页展开 **「Codex 数据目录与高级备用选项」**。
   - 在旧电脑设置口令（采用 Argon2id + ChaCha20-Poly1305 AEAD 认证加密），导出 `.cdxmove` 加密迁移包到移动硬盘/U 盘，再在新电脑输入口令解密并进入同一个路径映射与导入向导。

---

## 6. 如何回滚（恢复到迁移前状态）？

Codex Move 采用 **先验证 staging、先备份目标、再事务性应用** 的设计：

- **导入中途出错自动回滚**：如果写入新电脑期间发生断电、磁盘满或校验失败，应用会自动根据持久化恢复日志（`RecoveryJournal`）恢复已修改文件并清理新增文件。
- **导入完成后手动一键回滚**：
  1. 在新电脑打开 Codex Move 首页。
  2. 在 **「恢复与回滚历史迁移」** 区域找到对应的迁移记录，点击 **「回滚到迁移前状态」**。
  3. 确认后，应用会校验备份文件的 SHA-256 哈希，将新电脑 `.codex` 目录和关联文件精确还原到本次迁移前的状态。

---

## 7. 当前已验证的 Codex 版本与系统范围

详细的存储结构与兼容矩阵请参阅 [`docs/compatibility.md`](./docs/compatibility.md)：

- **已验证的 Codex 存储结构**：
  - 状态数据库：`state_5.sqlite`（含迁移版本 `v1`–`v38`）及 `state_4.sqlite`（向后兼容）。
  - 辅助数据库：`thread_history_1.sqlite`、`memories_1.sqlite`、`goals_1.sqlite`。
  - 会话与配置文件：`sessions/**/*.jsonl`、`archived_sessions/**/*.jsonl`、`session_index.jsonl`、`.codex-global-state.json`、`config.toml`、`AGENTS.md`、`skills/`、`automations/`。
- **未知未来版本保护策略**：
  - 若未来新版 Codex 引入未知的 `state_6.sqlite` 或不兼容主表结构，Codex Move 会自动识别并禁止直接改写数据库，仅提供安全的只读原始备份，绝不静默损坏用户数据。
- **支持平台**：
  - Windows 10 (1803+) / Windows 11 (`x86_64`)
  - macOS 12 Monterey 及以上 (`aarch64` Apple Silicon & `x86_64` Intel)

---

## 8. 开发者文档与开源许可

- [系统架构说明 (`docs/architecture.md`)](./docs/architecture.md)
- [迁移包与协议格式 (`docs/migration-format.md`)](./docs/migration-format.md)
- [Codex 版本兼容矩阵 (`docs/compatibility.md`)](./docs/compatibility.md)
- [安全与威胁模型 (`docs/threat-model.md`)](./docs/threat-model.md)
- [测试与故障注入指南 (`docs/testing.md`)](./docs/testing.md)
- [构建、签名与发布流程 (`docs/release.md`)](./docs/release.md)
- [贡献指南 (`CONTRIBUTING.md`)](./CONTRIBUTING.md) · [安全漏洞报告 (`SECURITY.md`)](./SECURITY.md)

本项目基于 [Apache License 2.0](./LICENSE) 开源，协议设计参考声明详见 [NOTICE](./NOTICE)。
