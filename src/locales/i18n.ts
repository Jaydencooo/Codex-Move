export type LocaleCode = "zh-CN" | "en";

export interface Translations {
  appName: string;
  appSubtitle: string;
  communityDisclaimer: string;
  sendFromThisComputer: string;
  sendFromThisComputerDesc: string;
  receiveOnThisComputer: string;
  receiveOnThisComputerDesc: string;
  stepConnect: string;
  stepSelect: string;
  stepMapping: string;
  stepSummary: string;
  stepTransfer: string;
  stepResult: string;
  backButton: string;
  nextButton: string;
  cancelButton: string;
  startMigrationButton: string;
  codexRunningWarningTitle: string;
  codexRunningWarningBody: string;
  recheckProcessButton: string;
  lanPairingTitle: string;
  lanPairingDescSender: string;
  lanPairingDescReceiver: string;
  discoveredDevicesTitle: string;
  noDevicesFoundHint: string;
  manualConnectLabel: string;
  connectButton: string;
  approveSenderTitle: string;
  approveSenderDesc: string;
  approveButton: string;
  rejectButton: string;
  selectContentTitle: string;
  selectContentDesc: string;
  sensitiveContentWarningTitle: string;
  sensitiveContentWarningBody: string;
  includeSensitiveLabel: string;
  projectMappingTitle: string;
  projectMappingDesc: string;
  targetProjectsRootLabel: string;
  applyDefaultRootButton: string;
  summaryTitle: string;
  summaryDesc: string;
  transferProgressTitle: string;
  importProgressTitle: string;
  resultSucceededTitle: string;
  resultPartialTitle: string;
  resultRolledBackTitle: string;
  resultFailedPartialTitle: string;
  automatedChecksTitle: string;
  manualConfirmationsTitle: string;
  pendingDependenciesTitle: string;
  rollbackSectionTitle: string;
  rollbackButton: string;
  rollbackConfirmDialogTitle: string;
  rollbackConfirmDialogBody: string;
  diagnosticsSummaryLabel: string;
  advancedOfflineBundleToggle: string;
  dispositionDirect: string;
  dispositionPathMap: string;
  dispositionReinstall: string;
  dispositionReauth: string;
  dispositionUnsupported: string;
  categoryNames: Record<string, string>;
}

export const LOCALES: Record<LocaleCode, Translations> = {
  "zh-CN": {
    appName: "Codex Move · Codex 搬家",
    appSubtitle: "在两台电脑之间，直接迁移 Codex 聊天记录、Skills、配置与项目文件夹",
    communityDisclaimer:
      "独立开源社区工具，与 OpenAI 无隶属或官方背书关系。全程局域网点对点加密传输，不经过任何云服务器。",
    sendFromThisComputer: "从这台电脑发送",
    sendFromThisComputerDesc:
      "这是我的旧电脑。扫描本机 Codex 聊天记录、Skills 与项目文件夹，并通过局域网直接发送到新电脑。",
    receiveOnThisComputer: "迁移到这台电脑",
    receiveOnThisComputerDesc:
      "这是我的新电脑。展示安全连接码并等待旧电脑连接，确认项目存放位置后自动完成合并与验证。",
    stepConnect: "1. 连接电脑",
    stepSelect: "2. 选择迁移内容",
    stepMapping: "3. 选择项目存放位置",
    stepSummary: "4. 迁移摘要",
    stepTransfer: "5. 开始迁移",
    stepResult: "6. 查看结果",
    backButton: "返回上一步",
    nextButton: "下一步",
    cancelButton: "取消",
    startMigrationButton: "确认并开始迁移",
    codexRunningWarningTitle: "检测到 Codex 正在运行",
    codexRunningWarningBody:
      "为了保证所有聊天记录和状态文件完整一致，请先手动关闭 Codex 桌面端或命令行窗口（本工具不会强制终止您的进程）。",
    recheckProcessButton: "我已关闭，重新检查",
    lanPairingTitle: "局域网安全连接与配对",
    lanPairingDescSender:
      "请确保两台电脑连接在同一个局域网（同一 Wi-Fi 或路由器，且未开启访客网络隔离）。您可以直接选择下方发现的新电脑，或粘贴新电脑上显示的安全连接码。",
    lanPairingDescReceiver:
      "请在旧电脑上打开 Codex Move 并选择「从这台电脑发送」。核对下方 6 位安全配对码或直接复制连接码到旧电脑。",
    discoveredDevicesTitle: "同一局域网内发现的电脑",
    noDevicesFoundHint:
      "暂未自动发现设备。如果路由器开启了 AP 隔离或系统防火墙拦截了 UDP 53317 端口，请直接粘贴新电脑上的「安全连接码」手动连接。",
    manualConnectLabel: "粘贴安全连接码（cdxmove://...）或输入 6 位配对码",
    connectButton: "请求安全配对",
    approveSenderTitle: "收到来自旧电脑的迁移请求",
    approveSenderDesc:
      "请确认对方设备名称与迁移范围。只有在您点击「允许接收」后，对方才能开始传输数据。",
    approveButton: "确认身份并允许接收",
    rejectButton: "拒绝并断开",
    selectContentTitle: "选择要搬家的 Codex 内容",
    selectContentDesc:
      "登录凭据（auth.json）默认不迁移，迁移后只需在新电脑重新登录。插件和本地运行环境会迁移清单，按需在新电脑重新安装。",
    sensitiveContentWarningTitle: "敏感配置与密钥检测提示",
    sensitiveContentWarningBody:
      "自动检测发现了可能包含环境变量或密钥的文件（如 .env 或 MCP 服务配置）。自动检测仅作辅助，无法保证识别所有秘密。默认不勾选敏感文件。",
    includeSensitiveLabel: "我已知晓风险，包含已标记的 .env 与配置凭据",
    projectMappingTitle: "选择新电脑上的项目存放位置",
    projectMappingDesc:
      "请选择一个项目根目录，Codex Move 会自动为每个项目生成新路径，并将聊天记录自动关联到新位置。您也可以逐个修改。",
    targetProjectsRootLabel: "新电脑默认项目根目录",
    applyDefaultRootButton: "按根目录重新生成路径",
    summaryTitle: "迁移前最终确认摘要",
    summaryDesc:
      "目标电脑如已有 Codex 聊天记录，将默认安全合并（相同内容自动去重，同 ID 分叉聊天保留双方副本，绝不自动丢弃）。",
    transferProgressTitle: "第一阶段：局域网加密传输进度",
    importProgressTitle: "第二阶段：本地备份、路径转换与合并进度",
    resultSucceededTitle: "搬家完成，所有自动校验均已通过",
    resultPartialTitle: "搬家已完成（含待确认提示）",
    resultRolledBackTitle: "迁移中断，已自动完整回滚至原始状态",
    resultFailedPartialTitle: "迁移失败且部分回滚受阻（已保留完整备份材料）",
    automatedChecksTitle: "自动验证结果",
    manualConfirmationsTitle: "需要您在 Codex 中确认的事项",
    pendingDependenciesTitle: "需要重新登录或按需安装的依赖",
    rollbackSectionTitle: "安全备份与一键回滚",
    rollbackButton: "回滚本次迁移（恢复修改前状态）",
    rollbackConfirmDialogTitle: "确认回滚本次迁移？",
    rollbackConfirmDialogBody:
      "回滚将使用迁移前经过校验的本地备份恢复所有被修改的文件，并移除本次新导入的文件。您的原始数据不会被破坏。",
    diagnosticsSummaryLabel: "展开技术诊断信息（数据库版本 / 索引 / 脱敏日志）",
    advancedOfflineBundleToggle: "高级备用功能：导出/导入加密离线迁移包 (.cdxmove)",
    dispositionDirect: "可直接迁移",
    dispositionPathMap: "自动转换路径",
    dispositionReinstall: "需在目标电脑重装",
    dispositionReauth: "需重新登录认证",
    dispositionUnsupported: "当前版本暂不支持",
    categoryNames: {
      ActiveAndArchivedChats: "聊天记录（活跃与归档会话）",
      ChatMetadataAndIndex: "聊天标题、项目关联与分组索引",
      UserSkills: "用户级 Skills（.codex/skills）",
      SharedAgentSkills: "共享 Skills（.agents/skills）",
      InstructionFiles: "全局指令文件（AGENTS.md）",
      CoreConfig: "核心偏好与模型配置（config.toml）",
      ExtensionsMemoriesRulesHooks: "记忆、目标、Hooks 与自动化（默认暂停态）",
      ChatAttachments: "聊天附件与生成图片",
      ProjectFiles: "项目文件夹（源码、未提交修改、未跟踪文件与 .git）",
      PluginManifests: "插件启用清单（不含跨平台二进制）",
    },
  },
  en: {
    appName: "Codex Move",
    appSubtitle:
      "Directly migrate Codex chats, Skills, config, and project folders between two computers",
    communityDisclaimer:
      "Independent open-source community tool, not affiliated with or endorsed by OpenAI. Peer-to-peer LAN encrypted transfer with zero cloud servers.",
    sendFromThisComputer: "Send from this computer",
    sendFromThisComputerDesc:
      "This is my old computer. Scan local Codex chats, Skills, and projects, then send them directly over LAN.",
    receiveOnThisComputer: "Migrate to this computer",
    receiveOnThisComputerDesc:
      "This is my new computer. Display a secure pairing code, choose where projects should live, and merge safely.",
    stepConnect: "1. Connect",
    stepSelect: "2. Select Content",
    stepMapping: "3. Project Locations",
    stepSummary: "4. Summary",
    stepTransfer: "5. Migrate",
    stepResult: "6. Results",
    backButton: "Back",
    nextButton: "Continue",
    cancelButton: "Cancel",
    startMigrationButton: "Confirm & Start Migration",
    codexRunningWarningTitle: "Codex is currently running",
    codexRunningWarningBody:
      "To guarantee a consistent cross-file snapshot, please close the Codex app or CLI before proceeding (Codex Move will never force-kill your processes).",
    recheckProcessButton: "I closed Codex, re-check",
    lanPairingTitle: "Secure LAN Connection & Pairing",
    lanPairingDescSender:
      "Make sure both computers are on the same local network. Select a discovered computer below or paste the secure connection code from the new computer.",
    lanPairingDescReceiver:
      "Open Codex Move on your old computer and choose 'Send from this computer'. Verify the 6-digit code or copy the connection string.",
    discoveredDevicesTitle: "Computers Discovered on Local Network",
    noDevicesFoundHint:
      "No devices auto-discovered yet. If guest Wi-Fi isolation or a firewall blocks UDP port 53317, paste the Connection Code manually.",
    manualConnectLabel: "Paste Connection Code (cdxmove://...) or 6-char Pairing Code",
    connectButton: "Request Secure Pairing",
    approveSenderTitle: "Incoming Migration Request",
    approveSenderDesc:
      "Verify the sender device name and migration scope. Data transfer cannot begin until you click Approve.",
    approveButton: "Approve & Allow Transfer",
    rejectButton: "Reject & Disconnect",
    selectContentTitle: "Select Codex Content to Migrate",
    selectContentDesc:
      "Login credentials (auth.json) are excluded by default—simply sign in on the new computer. Plugins migrate as manifests for clean reinstallation.",
    sensitiveContentWarningTitle: "Sensitive Config & Secret Scan Notice",
    sensitiveContentWarningBody:
      "Automated detection found files that may contain secrets (.env or MCP config). Automated detection is assistive only and cannot guarantee finding every secret.",
    includeSensitiveLabel: "Include flagged .env and sensitive configuration entries",
    projectMappingTitle: "Choose Project Locations on New Computer",
    projectMappingDesc:
      "Pick a root folder for your projects. Codex Move automatically generates new paths and re-binds your chat history.",
    targetProjectsRootLabel: "Default Projects Root Folder",
    applyDefaultRootButton: "Regenerate Paths Under Root",
    summaryTitle: "Pre-Migration Summary",
    summaryDesc:
      "Existing Codex data on the target computer is safely merged by default (identical chats deduplicated, divergent chats preserved side-by-side).",
    transferProgressTitle: "Stage 1: Encrypted LAN Transfer Progress",
    importProgressTitle: "Stage 2: Local Backup, Path Remapping & Merge Progress",
    resultSucceededTitle: "Migration Complete — All Automated Checks Passed",
    resultPartialTitle: "Migration Completed with Review Items",
    resultRolledBackTitle: "Migration Interrupted — Automatically Rolled Back",
    resultFailedPartialTitle: "Migration Failed with Partial Rollback (Backup Retained)",
    automatedChecksTitle: "Automated Verification Results",
    manualConfirmationsTitle: "Items to Confirm Inside Codex",
    pendingDependenciesTitle: "Sign-in & Reinstallation Reminders",
    rollbackSectionTitle: "Verified Backup & One-Click Rollback",
    rollbackButton: "Roll Back This Migration",
    rollbackConfirmDialogTitle: "Roll back this migration?",
    rollbackConfirmDialogBody:
      "Rolling back restores all modified files from verified pre-migration backups and removes newly imported files.",
    diagnosticsSummaryLabel: "Technical Diagnostics (DB Schema / Indexes / Redacted Logs)",
    advancedOfflineBundleToggle: "Advanced Fallback: Encrypted Offline Bundle (.cdxmove)",
    dispositionDirect: "Direct Transfer",
    dispositionPathMap: "Auto Path Mapping",
    dispositionReinstall: "Reinstall on Target",
    dispositionReauth: "Re-authenticate on Target",
    dispositionUnsupported: "Unsupported",
    categoryNames: {
      ActiveAndArchivedChats: "Chat History (Active & Archived Sessions)",
      ChatMetadataAndIndex: "Chat Titles, Project Bindings & Indexes",
      UserSkills: "User Skills (.codex/skills)",
      SharedAgentSkills: "Shared Skills (.agents/skills)",
      InstructionFiles: "Global Instructions (AGENTS.md)",
      CoreConfig: "Core Preferences & Model Config (config.toml)",
      ExtensionsMemoriesRulesHooks: "Memories, Goals, Hooks & Automations (Paused)",
      ChatAttachments: "Chat Attachments & Generated Media",
      ProjectFiles: "Project Folders (Source, Uncommitted, Untracked & .git)",
      PluginManifests: "Plugin Manifests (Excludes Native Binaries)",
    },
  },
};

export function detectSystemLocale(): LocaleCode {
  if (typeof navigator !== "undefined" && navigator.language) {
    return navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en";
  }
  return "zh-CN";
}

export function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const idx = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1
  );
  const val = bytes / Math.pow(1024, idx);
  return `${val.toFixed(idx === 0 ? 0 : 1)} ${units[idx]}`;
}
