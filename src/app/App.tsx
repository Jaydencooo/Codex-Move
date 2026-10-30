import React, { useEffect, useId, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  apiApplyMigration,
  apiApproveReceiverPairing,
  apiBuildImportPlan,
  apiDiscoverRecoveryJournals,
  apiExecuteRollback,
  apiExportEncryptedBundle,
  apiGetCachedManifest,
  apiGetReceiverSessionSnapshot,
  apiGetSystemOverview,
  apiImportEncryptedBundle,
  apiListDiscoveredPeers,
  apiProposeProjectMappings,
  apiRejectReceiverPairing,
  apiScanCodexInventory,
  apiSenderExecuteTransfer,
  apiSenderPairWithReceiver,
  apiSenderPollApproval,
  apiStartReceiverMode,
  apiStartSenderDiscovery,
  apiStopNetworkServices,
  ContentCategory,
  DiscoveredPeer,
  ImportPlan,
  MigrationDisposition,
  MigrationExecutionResult,
  MigrationManifest,
  ProjectPathMappingConfig,
  ReceiverPairingTicket,
  ReceiverSessionSnapshot,
  RecoveryJournal,
  RollbackReport,
  SystemOverview,
} from "./ipc";
import {
  detectSystemLocale,
  formatBytes,
  LocaleCode,
  LOCALES,
} from "../locales/i18n";

import logoUrl from "../assets/logo.svg";

type RoleMode = "home" | "sender" | "receiver";
type WizardStep = 1 | 2 | 3 | 4 | 5 | 6;

const ALL_DEFAULT_CATEGORIES: ContentCategory[] = [
  "ActiveAndArchivedChats",
  "ChatMetadataAndIndex",
  "UserSkills",
  "SharedAgentSkills",
  "InstructionFiles",
  "CoreConfig",
  "ExtensionsMemoriesRulesHooks",
  "ChatAttachments",
  "ProjectFiles",
  "PluginManifests",
];

export function App(): React.ReactElement {
  const [locale, setLocale] = useState<LocaleCode>(detectSystemLocale);
  const t = LOCALES[locale];
  const [theme, setTheme] = useState<"light" | "dark">(() => {
    try { return localStorage.getItem("codex-move-theme") === "dark" ? "dark" : "light"; }
    catch { return "light"; }
  });
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.lang = locale;
    try { localStorage.setItem("codex-move-theme", theme); } catch { /* Theme still applies when storage is unavailable. */ }
  }, [theme, locale]);

  const [role, setRole] = useState<RoleMode>("home");
  const [step, setStep] = useState<WizardStep>(1);
  const [overview, setOverview] = useState<SystemOverview | null>(null);
  const [codexHomePath, setCodexHomePath] = useState<string>("");
  const [recoveryJournals, setRecoveryJournals] = useState<RecoveryJournal[]>(
    []
  );

  // Error & busy state
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [busyMessage, setBusyMessage] = useState<string | null>(null);

  // Discovery & Pairing state
  const [discoveredPeers, setDiscoveredPeers] = useState<DiscoveredPeer[]>([]);
  const [selectedPeer, setSelectedPeer] = useState<DiscoveredPeer | null>(null);
  const [connectionInput, setConnectionInput] = useState<string>("");
  const [manualHostIp, setManualHostIp] = useState<string>("");
  const [receiverTicket, setReceiverTicket] =
    useState<ReceiverPairingTicket | null>(null);
  const [receiverStagingDir, setReceiverStagingDir] = useState<string>("");
  const [receiverSnapshot, setReceiverSnapshot] =
    useState<ReceiverSessionSnapshot | null>(null);

  // Sender established connection state
  const [senderTargetUrl, setSenderTargetUrl] = useState<string>("");
  const [senderTargetFingerprint, setSenderTargetFingerprint] =
    useState<string>("");
  const [senderPollToken, setSenderPollToken] = useState<string | null>(null);
  const [senderSasCode, setSenderSasCode] = useState<string | null>(null);
  const [senderBearerToken, setSenderBearerToken] = useState<string | null>(
    null
  );
  const [waitingReceiverApproval, setWaitingReceiverApproval] =
    useState<boolean>(false);

  // Content Selection & Manifest state
  const [manifest, setManifest] = useState<MigrationManifest | null>(null);
  const [selectedCategories, setSelectedCategories] = useState<
    ContentCategory[]
  >(ALL_DEFAULT_CATEGORIES);
  const [includeSensitive, setIncludeSensitive] = useState<boolean>(false);

  // Mapping & Import Plan state
  const [defaultProjectsRoot, setDefaultProjectsRoot] = useState<string>("");
  const [projectMappings, setProjectMappings] = useState<
    ProjectPathMappingConfig[]
  >([]);
  const [importPlan, setImportPlan] = useState<ImportPlan | null>(null);

  // Progress & Execution Result state
  const [transferProgress, setTransferProgress] = useState<{
    processedFiles: number;
    totalFiles: number;
    processedBytes: number;
    totalBytes: number;
    currentItem: string;
  } | null>(null);
  const [importProgress, setImportProgress] = useState<{
    phase: string;
    processedFiles: number;
    totalFiles: number;
    processedBytes: number;
    totalBytes: number;
    currentItem: string;
  } | null>(null);
  const [executionResult, setExecutionResult] =
    useState<MigrationExecutionResult | null>(null);
  const [rollbackReport, setRollbackReport] = useState<RollbackReport | null>(
    null
  );

  // Advanced Offline Bundle fallback state
  const [bundlePathInput, setBundlePathInput] = useState<string>("");
  const [bundlePassphrase, setBundlePassphrase] = useState<string>("");
  const [bundleStatusMessage, setBundleStatusMessage] = useState<
    string | null
  >(null);

  // Native <dialog> ref for rollback confirmation (follows modern-web-guidance `dialog` rule)
  const rollbackDialogRef = useRef<HTMLDialogElement | null>(null);
  const rollbackDialogTitleId = useId();
  const [pendingRollbackMigrationId, setPendingRollbackMigrationId] = useState<
    string | null
  >(null);

  // Initial system overview load
  useEffect(() => {
    void refreshSystemOverview();
  }, []);

  async function refreshSystemOverview() {
    try {
      const sys = await apiGetSystemOverview();
      setOverview(sys);
      const defaultHome = sys.default_codex_home ?? "";
      if (!codexHomePath && defaultHome) {
        setCodexHomePath(defaultHome);
      }
      if (!defaultProjectsRoot && defaultHome) {
        const sep = sys.os === "windows" ? "\\" : "/";
        const parentDir = defaultHome.replace(/[\\/]\.codex$/i, "");
        setDefaultProjectsRoot(`${parentDir}${sep}CodexProjects`);
      }
      if (defaultHome) {
        const journals = await apiDiscoverRecoveryJournals(defaultHome);
        setRecoveryJournals(journals);
      }
    } catch (err) {
      setErrorMessage(extractErrorText(err));
    }
  }

  // Subscribe to Tauri real-time transfer & migration progress events
  useEffect(() => {
    let unlistenTransfer: (() => void) | undefined;
    let unlistenMigration: (() => void) | undefined;

    if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
      void listen<{
        processed_files: number;
        total_files: number;
        processed_bytes: number;
        total_bytes: number;
        current_item: string;
      }>("transfer-progress", (ev) => {
        setTransferProgress({
          processedFiles: ev.payload.processed_files,
          totalFiles: ev.payload.total_files,
          processedBytes: ev.payload.processed_bytes,
          totalBytes: ev.payload.total_bytes,
          currentItem: ev.payload.current_item,
        });
      }).then((fn) => {
        unlistenTransfer = fn;
      });

      void listen<{
        phase: string;
        processed_files: number;
        total_files: number;
        processed_bytes: number;
        total_bytes: number;
        current_item: string;
      }>("migration-progress", (ev) => {
        setImportProgress({
          phase: ev.payload.phase,
          processedFiles: ev.payload.processed_files,
          totalFiles: ev.payload.total_files,
          processedBytes: ev.payload.processed_bytes,
          totalBytes: ev.payload.total_bytes,
          currentItem: ev.payload.current_item,
        });
      }).then((fn) => {
        unlistenMigration = fn;
      });
    }

    return () => {
      if (unlistenTransfer) unlistenTransfer();
      if (unlistenMigration) unlistenMigration();
    };
  }, []);

  // Poll discovery or receiver status while in active network steps
  useEffect(() => {
    if (role === "home") return;

    const timer = window.setInterval(async () => {
      try {
        if (role === "sender" && step === 1) {
          const peers = await apiListDiscoveredPeers();
          setDiscoveredPeers(peers);

          if (
            waitingReceiverApproval &&
            senderTargetUrl &&
            senderTargetFingerprint &&
            senderPollToken
          ) {
            const poll = await apiSenderPollApproval(
              senderTargetUrl,
              senderTargetFingerprint,
              senderPollToken
            );
            if (poll.approved && poll.session_bearer_token) {
              setSenderBearerToken(poll.session_bearer_token);
              setWaitingReceiverApproval(false);
              setStep(2);
            }
          }
        } else if (role === "receiver" && (step === 1 || step === 2)) {
          const snap = await apiGetReceiverSessionSnapshot();
          if (snap) {
            setReceiverSnapshot(snap);
            if (snap.transfer_completed && step === 1) {
              // Receiver received full staging snapshot -> load manifest & move to step 2 (Select Content)
              const receivedManifest = await apiGetCachedManifest();
              setManifest(receivedManifest);
              const defaultCats = receivedManifest.categories
                .filter((c) => c.selected_by_default && c.item_count > 0)
                .map((c) => c.category);
              if (defaultCats.length > 0) {
                setSelectedCategories(defaultCats);
              }
              await apiStopNetworkServices();
              setStep(2);
            }
          }
        }
      } catch {
        // Ignore transient poll errors
      }
    }, 1200);

    return () => window.clearInterval(timer);
  }, [
    role,
    step,
    waitingReceiverApproval,
    senderTargetUrl,
    senderTargetFingerprint,
    senderPollToken,
  ]);

  function extractErrorText(err: unknown): string {
    if (typeof err === "string") return err;
    if (err && typeof err === "object") {
      const maybeObj = err as Record<string, unknown>;
      if (typeof maybeObj.message === "string") {
        return maybeObj.detail
          ? `${maybeObj.message} (${String(maybeObj.detail)})`
          : maybeObj.message;
      }
    }
    return String(err);
  }

  async function handleStartSenderFlow() {
    setErrorMessage(null);
    setBusyMessage(
      locale === "zh-CN"
        ? "正在扫描本机 Codex 数据与开启局域网发现..."
        : "Scanning local Codex data & starting LAN discovery..."
    );
    try {
      await refreshSystemOverview();
      const targetHome =
        codexHomePath || overview?.default_codex_home || "~/.codex";
      const scanned = await apiScanCodexInventory(targetHome, true);
      setManifest(scanned);
      await apiStartSenderDiscovery();
      setRole("sender");
      setStep(1);
    } catch (err) {
      setErrorMessage(extractErrorText(err));
    } finally {
      setBusyMessage(null);
    }
  }

  async function handleStartReceiverFlow() {
    setErrorMessage(null);
    setBusyMessage(
      locale === "zh-CN"
        ? "正在生成一次性 TLS 证书与局域网安全连接码..."
        : "Generating ephemeral TLS certificate & pairing ticket..."
    );
    try {
      await refreshSystemOverview();
      const alias = overview?.hostname_display || "Codex-New-Computer";
      const res = await apiStartReceiverMode(alias, false);
      setReceiverTicket(res.ticket);
      setReceiverStagingDir(res.staging_dir);
      setRole("receiver");
      setStep(1);
    } catch (err) {
      setErrorMessage(extractErrorText(err));
    } finally {
      setBusyMessage(null);
    }
  }

  async function handleReturnHome() {
    await apiStopNetworkServices();
    setRole("home");
    setStep(1);
    setErrorMessage(null);
    setWaitingReceiverApproval(false);
    void refreshSystemOverview();
  }

  async function handleSenderRequestPairing() {
    setErrorMessage(null);
    setBusyMessage(
      locale === "zh-CN"
        ? "正在验证对端 TLS 证书指纹并发送配对请求..."
        : "Verifying peer TLS fingerprint & requesting pairing..."
    );
    try {
      const senderName = overview?.hostname_display || "Codex-Old-Computer";
      const res = await apiSenderPairWithReceiver({
        connection_code_or_short_code: connectionInput,
        manual_host_ip: selectedPeer?.ip_address || manualHostIp || null,
        manual_port: selectedPeer?.https_port || 53317,
        peer_cert_fingerprint: selectedPeer?.full_cert_fingerprint || null,
        peer_pake_salt_hex: selectedPeer?.pake_salt_hex || null,
        sender_device_name: senderName,
      });

      setSenderTargetUrl(res.receiver_base_url);
      setSenderTargetFingerprint(res.receiver_cert_fingerprint);
      setSenderPollToken(res.poll_token);
      setSenderSasCode(res.sas_verification_code ?? null);

      if (res.approved && res.session_bearer_token) {
        setSenderBearerToken(res.session_bearer_token);
        setWaitingReceiverApproval(false);
        setStep(2);
      } else {
        setWaitingReceiverApproval(true);
      }
    } catch (err) {
      setErrorMessage(extractErrorText(err));
    } finally {
      setBusyMessage(null);
    }
  }

  async function handleReceiverApproveSender() {
    setErrorMessage(null);
    try {
      await apiApproveReceiverPairing();
      const snap = await apiGetReceiverSessionSnapshot();
      setReceiverSnapshot(snap);
    } catch (err) {
      setErrorMessage(extractErrorText(err));
    }
  }

  async function handleReceiverRejectSender() {
    await apiRejectReceiverPairing();
    const snap = await apiGetReceiverSessionSnapshot();
    setReceiverSnapshot(snap);
  }

  async function handleSenderStartStreamingTransfer() {
    if (!senderBearerToken) return;
    setErrorMessage(null);
    setStep(5);
    try {
      await apiSenderExecuteTransfer({
        receiverBaseUrl: senderTargetUrl,
        receiverCertFingerprint: senderTargetFingerprint,
        bearerToken: senderBearerToken,
        selectedCategories,
        includeSensitiveFiles: includeSensitive,
      });
      await apiStopNetworkServices();
      setStep(6);
    } catch (err) {
      setErrorMessage(extractErrorText(err));
    }
  }

  async function handlePrepareProjectMappingsStep() {
    setErrorMessage(null);
    try {
      const proposed = await apiProposeProjectMappings(defaultProjectsRoot);
      setProjectMappings(proposed);
      setStep(3);
    } catch (err) {
      setErrorMessage(extractErrorText(err));
    }
  }

  async function handlePrepareSummaryStep() {
    setErrorMessage(null);
    setBusyMessage(
      locale === "zh-CN"
        ? "正在校验路径冲突、可用空间与合并计划..."
        : "Validating path conflicts, disk space, and merge plan..."
    );
    try {
      const plan = await apiBuildImportPlan({
        targetCodexHome: codexHomePath,
        targetSharedSkillsDir: overview?.shared_agents_skills_dir ?? null,
        selectedCategories,
        projectMappings,
        includeSensitiveFiles: includeSensitive,
        stagingAlreadyComplete: true,
      });
      setImportPlan(plan);
      setStep(4);
    } catch (err) {
      setErrorMessage(extractErrorText(err));
    } finally {
      setBusyMessage(null);
    }
  }

  async function handleReceiverExecuteApply() {
    if (!importPlan || !receiverStagingDir) return;
    setErrorMessage(null);
    setStep(5);
    try {
      const result = await apiApplyMigration(receiverStagingDir, importPlan);
      setExecutionResult(result);
      await refreshSystemOverview();
      setStep(6);
    } catch (err) {
      setErrorMessage(extractErrorText(err));
    }
  }

  function openRollbackModal(migrationId: string) {
    setPendingRollbackMigrationId(migrationId);
    const dlg = rollbackDialogRef.current;
    if (dlg && !dlg.open) {
      dlg.showModal();
    }
  }

  async function handleRollbackDialogClose() {
    const dlg = rollbackDialogRef.current;
    if (!dlg) return;
    const action = dlg.returnValue;
    // Reset returnValue immediately per modern-web-guidance dialog rule
    dlg.returnValue = "";

    if (action === "confirm" && pendingRollbackMigrationId) {
      setErrorMessage(null);
      setBusyMessage(
        locale === "zh-CN"
          ? "正在从已校验备份回滚并验证文件 SHA-256..."
          : "Rolling back from verified backup and checking SHA-256..."
      );
      try {
        const rb = await apiExecuteRollback(
          codexHomePath,
          pendingRollbackMigrationId
        );
        setRollbackReport(rb);
        await refreshSystemOverview();
      } catch (err) {
        setErrorMessage(extractErrorText(err));
      } finally {
        setBusyMessage(null);
      }
    }
  }

  function renderDispositionBadge(disp: MigrationDisposition) {
    switch (disp) {
      case "DirectCopy":
        return <span className="badge success">{t.dispositionDirect}</span>;
      case "NeedsPathMapping":
        return <span className="badge success">{t.dispositionPathMap}</span>;
      case "NeedsReinstallOnTarget":
        return <span className="badge warn">{t.dispositionReinstall}</span>;
      case "NeedsReauthOnTarget":
        return <span className="badge warn">{t.dispositionReauth}</span>;
      case "UnsupportedInCurrentVersion":
        return <span className="badge error">{t.dispositionUnsupported}</span>;
    }
  }

  function toggleCategory(cat: ContentCategory) {
    setSelectedCategories((prev) =>
      prev.includes(cat) ? prev.filter((c) => c !== cat) : [...prev, cat]
    );
  }

  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand-group">
          <img className="brand-mark" src={logoUrl} alt="" />
          <div>
            <h1 className="brand-title">Codex Move</h1>
            <span className="brand-subtitle">{locale === "zh-CN" ? "Codex 搬家 · 让工作继续" : "Your workspace, carried forward"}</span>
          </div>
        </div>
        <div className="header-actions">
          <button type="button" className="btn" onClick={() => setTheme(theme === "light" ? "dark" : "light")}
            aria-label={locale === "zh-CN" ? "切换黑白主题" : "Toggle light and dark theme"}>
            {locale === "zh-CN" ? (theme === "light" ? "深色" : "浅色") : (theme === "light" ? "Dark" : "Light")}
          </button>
          {role !== "home" && (
            <button
              type="button"
              className="btn"
              onClick={() => void handleReturnHome()}
            >
              {locale === "zh-CN" ? "返回首页" : "Home"}
            </button>
          )}
          <button
            type="button"
            className="btn"
            onClick={() => setLocale(locale === "zh-CN" ? "en" : "zh-CN")}
            aria-label="Switch Language"
          >
            {locale === "zh-CN" ? "English" : "简体中文"}
          </button>
        </div>
      </header>

      {/* Polite live region for status announcements without stealing focus */}
      <div role="status" aria-live="polite">
        {busyMessage && (
          <div className="alert-banner">
            <strong>{busyMessage}</strong>
            <div style={{ marginTop: 8 }}>
              <progress aria-label={busyMessage} />
            </div>
          </div>
        )}
        {errorMessage && (
          <div className="alert-banner error">
            <strong>
              {locale === "zh-CN" ? "操作提示：" : "Notice: "}
            </strong>
            <span>{errorMessage}</span>
          </div>
        )}
      </div>

      {/* Active Codex process warning banner */}
      {overview && overview.active_codex_processes.length > 0 && (
        <div className="alert-banner warn">
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
              gap: 12,
            }}
          >
            <div>
              <strong>{t.codexRunningWarningTitle}</strong>
              <p style={{ margin: "4px 0 0" }}>{t.codexRunningWarningBody}</p>
            </div>
            <button
              type="button"
              className="btn"
              onClick={() => void refreshSystemOverview()}
            >
              {t.recheckProcessButton}
            </button>
          </div>
        </div>
      )}

      {/* Step navigation pills when inside Sender or Receiver wizard */}
      {role !== "home" && (
        <nav className="step-nav" aria-label="Migration Steps">
          {[
            { num: 1, label: t.stepConnect },
            { num: 2, label: t.stepSelect },
            ...(role === "receiver"
              ? [
                  { num: 3, label: t.stepMapping },
                  { num: 4, label: t.stepSummary },
                ]
              : []),
            { num: 5, label: t.stepTransfer },
            { num: 6, label: t.stepResult },
          ].map((s) => (
            <span
              key={s.num}
              className={`step-pill ${
                step === s.num ? "active" : step > s.num ? "done" : ""
              }`}
            >
              {s.label}
            </span>
          ))}
        </nav>
      )}

      {/* ==================== HOME VIEW ==================== */}
      {role === "home" && (
        <main>
          <section className="home-intro">
            <p className="home-eyebrow">CODEX MOVE / LOCAL TRANSFER</p>
            <h2>{locale === "zh-CN" ? "换一台电脑，接着做。" : "New computer. Same momentum."}</h2>
            <p>{locale === "zh-CN" ? "把聊天、Skills 和项目带到新电脑。两台电脑连接同一网络，即可开始。" : "Bring your chats, skills and projects along. Connect both computers to the same network to begin."}</p>
          </section>
          <div className="primary-actions-grid">
            <button
              type="button"
              className="hero-action-card"
              onClick={() => void handleStartSenderFlow()}
            >
              <span className="badge" style={{ marginBottom: 12 }}>
                {locale === "zh-CN" ? "01 / 旧电脑" : "01 / OLD COMPUTER"}
              </span>
              <span className="action-symbol" aria-hidden="true">↗</span>
              <h2 className="hero-action-title">{locale === "zh-CN" ? "这是旧电脑" : "Send from here"}</h2>
              <p className="hero-action-desc">{t.sendFromThisComputerDesc}</p>
              <span className="btn btn-primary">{t.sendFromThisComputer} →</span>
            </button>

            <button
              type="button"
              className="hero-action-card"
              onClick={() => void handleStartReceiverFlow()}
            >
              <span className="badge" style={{ marginBottom: 12 }}>
                {locale === "zh-CN"
                  ? "02 / 新电脑"
                  : "02 / NEW COMPUTER"}
              </span>
              <span className="action-symbol" aria-hidden="true">↙</span>
              <h2 className="hero-action-title">{locale === "zh-CN" ? "这是新电脑" : "Receive here"}</h2>
              <p className="hero-action-desc">{t.receiveOnThisComputerDesc}</p>
              <span className="btn btn-primary">
                {t.receiveOnThisComputer} →
              </span>
            </button>
          </div>

          {/* Crash Recovery / Previous Migration Rollback Notice if any */}
          {recoveryJournals.length > 0 && (
            <section className="surface-card">
              <h3 className="section-title">{t.rollbackSectionTitle}</h3>
              <div className="item-list">
                {recoveryJournals.slice(0, 3).map((j) => (
                  <div key={j.migration_id} className="item-row">
                    <div>
                      <strong>
                        {locale === "zh-CN" ? "迁移记录：" : "Migration ID: "}
                        {j.migration_id}
                      </strong>
                      <div
                        style={{ fontSize: "0.85rem", color: "var(--text-muted)" }}
                      >
                        {j.updated_at} · {j.status} ({j.current_checkpoint})
                      </div>
                    </div>
                    {j.status !== "RolledBack" && (
                      <button
                        type="button"
                        className="btn btn-danger"
                        onClick={() => openRollbackModal(j.migration_id)}
                      >
                        {t.rollbackButton}
                      </button>
                    )}
                  </div>
                ))}
              </div>
              {rollbackReport && (
                <div className="alert-banner success" style={{ marginTop: 12 }}>
                  {locale === "zh-CN"
                    ? `已成功回滚迁移 ${rollbackReport.migration_id}：恢复 ${rollbackReport.restored_files_count} 个原有文件，清理 ${rollbackReport.removed_new_files_count} 个新增文件。`
                    : `Rolled back ${rollbackReport.migration_id}: restored ${rollbackReport.restored_files_count} files and removed ${rollbackReport.removed_new_files_count} new files.`}
                </div>
              )}
            </section>
          )}

          {/* Collapsible Codex Path & Advanced Offline Bundle Options */}
          <details className="surface-card home-advanced">
            <summary style={{ cursor: "pointer", fontWeight: 600 }}>
              {locale === "zh-CN"
                ? "Codex 数据目录与高级备用选项"
                : "Codex Directory & Advanced Fallback Options"}
            </summary>
            <div style={{ marginTop: 16 }}>
              <div className="field-row">
                <label htmlFor="codex-home-input">
                  {locale === "zh-CN"
                    ? "当前使用的 CODEX_HOME 目录（支持自动发现与手动修改）："
                    : "Active CODEX_HOME Directory:"}
                </label>
                <input
                  id="codex-home-input"
                  className="field-input"
                  value={codexHomePath}
                  onChange={(e) => setCodexHomePath(e.target.value)}
                />
              </div>

              <hr
                style={{
                  border: "none",
                  borderTop: "1px solid var(--border-subtle)",
                  margin: "16px 0",
                }}
              />
              <h4 style={{ margin: "0 0 8px" }}>
                {t.advancedOfflineBundleToggle}
              </h4>
              <div className="field-row">
                <label htmlFor="bundle-path-input">
                  {locale === "zh-CN"
                    ? "加密迁移包路径 (.cdxmove)："
                    : "Encrypted Bundle File Path (.cdxmove):"}
                </label>
                <input
                  id="bundle-path-input"
                  className="field-input"
                  placeholder="C:\Users\...\backup.cdxmove"
                  value={bundlePathInput}
                  onChange={(e) => setBundlePathInput(e.target.value)}
                />
              </div>
              <div className="field-row">
                <label htmlFor="bundle-pass-input">
                  {locale === "zh-CN"
                    ? "加密口令（Argon2id + ChaCha20-Poly1305）："
                    : "Encryption Passphrase (Argon2id + ChaCha20-Poly1305):"}
                </label>
                <input
                  id="bundle-pass-input"
                  type="password"
                  className="field-input"
                  value={bundlePassphrase}
                  onChange={(e) => setBundlePassphrase(e.target.value)}
                />
              </div>
              <div style={{ display: "flex", gap: 10 }}>
                <button
                  type="button"
                  className="btn"
                  disabled={!bundlePathInput || !bundlePassphrase}
                  onClick={async () => {
                    try {
                      setErrorMessage(null);
                      await apiScanCodexInventory(codexHomePath, true);
                      const hash = await apiExportEncryptedBundle(
                        bundlePathInput,
                        bundlePassphrase
                      );
                      setBundleStatusMessage(
                        locale === "zh-CN"
                          ? `加密迁移包导出成功，清单 SHA-256: ${hash.slice(0, 16)}...`
                          : `Encrypted bundle exported (SHA-256: ${hash.slice(0, 16)}...)`
                      );
                    } catch (err) {
                      setErrorMessage(extractErrorText(err));
                    }
                  }}
                >
                  {locale === "zh-CN"
                    ? "扫描并导出加密迁移包"
                    : "Scan & Export Encrypted Bundle"}
                </button>
                <button
                  type="button"
                  className="btn"
                  disabled={!bundlePathInput || !bundlePassphrase}
                  onClick={async () => {
                    try {
                      setErrorMessage(null);
                      const [loadedManifest, stgDir] =
                        await apiImportEncryptedBundle(
                          bundlePathInput,
                          bundlePassphrase
                        );
                      setManifest(loadedManifest);
                      setReceiverStagingDir(stgDir);
                      setRole("receiver");
                      setStep(2);
                    } catch (err) {
                      setErrorMessage(extractErrorText(err));
                    }
                  }}
                >
                  {locale === "zh-CN"
                    ? "解密并进入导入向导"
                    : "Decrypt & Open Import Wizard"}
                </button>
              </div>
              {bundleStatusMessage && (
                <p style={{ marginTop: 8, color: "var(--status-success-text)" }}>
                  {bundleStatusMessage}
                </p>
              )}
            </div>
          </details>
        </main>
      )}

      {/* ==================== STEP 1: LAN CONNECT & PAIRING ==================== */}
      {role !== "home" && step === 1 && (
        <main className="surface-card">
          <h2 className="section-title">{t.lanPairingTitle}</h2>
          <p className="section-desc">
            {role === "sender"
              ? t.lanPairingDescSender
              : t.lanPairingDescReceiver}
          </p>

          {role === "receiver" && receiverTicket && (
            <div>
              <div
                style={{
                  display: "grid",
                  gridTemplateColumns: "auto 1fr",
                  gap: 24,
                  alignItems: "center",
                  marginBottom: 20,
                }}
              >
                <div
                  className="qr-box"
                  dangerouslySetInnerHTML={{ __html: receiverTicket.qr_svg }}
                />
                <div>
                  <div style={{ marginBottom: 10 }}>
                    <span className="badge success">
                      {locale === "zh-CN"
                        ? "6 位安全配对码（Argon2id 保护）"
                        : "6-Char Pairing Code (Argon2id Protected)"}
                    </span>
                    <div
                      style={{
                        fontSize: "2rem",
                        fontWeight: 800,
                        letterSpacing: "0.2em",
                        fontFamily: "var(--font-mono)",
                        marginTop: 4,
                      }}
                    >
                      {receiverTicket.short_pake_code}
                    </div>
                  </div>
                  <div style={{ fontSize: "0.9rem", marginBottom: 8 }}>
                    <strong>IP:Port:</strong> {receiverTicket.host_ip}:
                    {receiverTicket.port} ·{" "}
                    <strong>TLS SHA-256:</strong>{" "}
                    {receiverTicket.cert_fingerprint_sha256
                      .slice(0, 16)
                      .toUpperCase()}
                  </div>
                  <div className="field-row">
                    <label>
                      {locale === "zh-CN"
                        ? "或复制完整安全连接码到旧电脑："
                        : "Or copy full secure connection code to old computer:"}
                    </label>
                    <div className="mono-code">
                      {receiverTicket.connection_code_string}
                    </div>
                  </div>
                </div>
              </div>

              {/* Incoming sender confirmation card */}
              {receiverSnapshot?.pending_sender_request &&
                !receiverSnapshot.pairing_approved && (
                  <div className="alert-banner warn">
                    <h3 style={{ margin: "0 0 6px" }}>{t.approveSenderTitle}</h3>
                    <p style={{ margin: "0 0 10px" }}>{t.approveSenderDesc}</p>
                    <p style={{ margin: "0 0 12px", fontWeight: 600 }}>
                      {receiverSnapshot.pending_sender_request.sender_device_name}{" "}
                      ({receiverSnapshot.pending_sender_request.sender_os} /{" "}
                      {receiverSnapshot.pending_sender_request.sender_arch}) —{" "}
                      {
                        receiverSnapshot.pending_sender_request
                          .summary_threads_count
                      }{" "}
                      {locale === "zh-CN" ? "个会话, " : "chats, "}
                      {
                        receiverSnapshot.pending_sender_request
                          .summary_projects_count
                      }{" "}
                      {locale === "zh-CN" ? "个项目 (" : "projects ("}
                      {formatBytes(
                        receiverSnapshot.pending_sender_request
                          .summary_total_bytes
                      )}
                      )
                    </p>
                    {receiverSnapshot.sas_verification_code && (
                      <p style={{ margin: "0 0 12px", fontFamily: "var(--font-mono)" }}>
                        <strong>
                          {locale === "zh-CN"
                            ? "安全校验码 (SAS)："
                            : "Security Verification Code (SAS): "}
                        </strong>
                        <span className="badge success">
                          {receiverSnapshot.sas_verification_code}
                        </span>
                      </p>
                    )}
                    <div style={{ display: "flex", gap: 10 }}>
                      <button
                        type="button"
                        className="btn btn-primary"
                        onClick={() => void handleReceiverApproveSender()}
                      >
                        {t.approveButton}
                      </button>
                      <button
                        type="button"
                        className="btn btn-danger"
                        onClick={() => void handleReceiverRejectSender()}
                      >
                        {t.rejectButton}
                      </button>
                    </div>
                  </div>
                )}

              {receiverSnapshot?.pairing_approved && (
                <div className="alert-banner success">
                  <strong>
                    {locale === "zh-CN"
                      ? "已批准连接，正在等待或接收旧电脑加密数据流..."
                      : "Connection approved. Receiving encrypted stream from old computer..."}
                  </strong>
                  <div style={{ marginTop: 8 }}>
                    <progress
                      value={
                        receiverSnapshot.total_bytes > 0
                          ? receiverSnapshot.received_bytes
                          : undefined
                      }
                      max={
                        receiverSnapshot.total_bytes > 0
                          ? receiverSnapshot.total_bytes
                          : undefined
                      }
                    />
                    <div style={{ fontSize: "0.85rem", marginTop: 4 }}>
                      {receiverSnapshot.received_files_count} /{" "}
                      {receiverSnapshot.total_files_count}{" "}
                      {locale === "zh-CN" ? "个文件 · " : "files · "}
                      {formatBytes(receiverSnapshot.received_bytes)} /{" "}
                      {formatBytes(receiverSnapshot.total_bytes)}
                    </div>
                  </div>
                </div>
              )}
            </div>
          )}

          {role === "sender" && (
            <div>
              <h3 style={{ fontSize: "1rem", marginBottom: 10 }}>
                {t.discoveredDevicesTitle}
              </h3>
              {discoveredPeers.length === 0 ? (
                <p
                  style={{
                    fontSize: "0.9rem",
                    color: "var(--text-muted)",
                    marginBottom: 16,
                  }}
                >
                  {t.noDevicesFoundHint}
                </p>
              ) : (
                <div className="item-list" style={{ marginBottom: 16 }}>
                  {discoveredPeers.map((peer) => (
                    <div
                      key={peer.session_id}
                      className="item-row"
                      style={{
                        borderColor:
                          selectedPeer?.session_id === peer.session_id
                            ? "var(--accent-primary)"
                            : undefined,
                      }}
                    >
                      <div>
                        <strong>{peer.device_alias}</strong>{" "}
                        <span className="badge">
                          {peer.os} ({peer.arch})
                        </span>
                        <div
                          style={{
                            fontSize: "0.85rem",
                            color: "var(--text-muted)",
                          }}
                        >
                          {peer.ip_address}:{peer.https_port} · TLS:{" "}
                          {peer.cert_fingerprint_prefix}
                        </div>
                      </div>
                      <button
                        type="button"
                        className="btn"
                        onClick={() => setSelectedPeer(peer)}
                      >
                        {selectedPeer?.session_id === peer.session_id
                          ? locale === "zh-CN"
                            ? "已选中"
                            : "Selected"
                          : locale === "zh-CN"
                          ? "选择此电脑"
                          : "Select"}
                      </button>
                    </div>
                  ))}
                </div>
              )}

              <div className="field-row">
                <label htmlFor="conn-code-input">{t.manualConnectLabel}</label>
                <input
                  id="conn-code-input"
                  className="field-input"
                  placeholder="cdxmove://... 或 6 位配对码"
                  value={connectionInput}
                  onChange={(e) => setConnectionInput(e.target.value)}
                />
              </div>

              {!connectionInput.trim().startsWith("cdxmove://") &&
                !selectedPeer && (
                  <div className="field-row">
                    <label htmlFor="manual-ip-input">
                      {locale === "zh-CN"
                        ? "新电脑局域网 IP 地址（使用 6 位配对码且未自动发现时填写）："
                        : "New Computer LAN IP Address:"}
                    </label>
                    <input
                      id="manual-ip-input"
                      className="field-input"
                      placeholder="192.168.1.x"
                      value={manualHostIp}
                      onChange={(e) => setManualHostIp(e.target.value)}
                    />
                  </div>
                )}

              {waitingReceiverApproval && (
                <div className="alert-banner warn">
                  <strong>
                    {locale === "zh-CN"
                      ? "配对验证通过！正在等待新电脑点击「确认身份并允许接收」..."
                      : "Pairing authenticated! Waiting for approval on the receiving computer..."}
                  </strong>
                  {senderSasCode && (
                    <div
                      style={{
                        marginTop: 6,
                        fontFamily: "var(--font-mono)",
                        fontSize: "0.9rem",
                      }}
                    >
                      {locale === "zh-CN"
                        ? "请核对两台电脑上的安全校验码 (SAS) 一致："
                        : "Verify matching Security Code (SAS) on both computers: "}
                      <span className="badge success">{senderSasCode}</span>
                    </div>
                  )}
                  <div style={{ marginTop: 8 }}>
                    <progress />
                  </div>
                </div>
              )}

              <div className="wizard-footer">
                <button
                  type="button"
                  className="btn"
                  onClick={() => void handleReturnHome()}
                >
                  {t.cancelButton}
                </button>
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={!connectionInput.trim()}
                  onClick={() => void handleSenderRequestPairing()}
                >
                  {t.connectButton}
                </button>
              </div>
            </div>
          )}
        </main>
      )}

      {/* ==================== STEP 2: SELECT CONTENT ==================== */}
      {role !== "home" && step === 2 && manifest && (
        <main className="surface-card">
          <h2 className="section-title">{t.selectContentTitle}</h2>
          <p className="section-desc">{t.selectContentDesc}</p>

          <div className="item-list" style={{ marginBottom: 18 }}>
            {manifest.categories.map((cat) => {
              const checked = selectedCategories.includes(cat.category);
              return (
                <label
                  key={cat.category}
                  className="item-row"
                  style={{ cursor: "pointer" }}
                >
                  <div style={{ display: "flex", gap: 12, alignItems: "center" }}>
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggleCategory(cat.category)}
                    />
                    <div>
                      <strong>
                        {t.categoryNames[cat.category] ?? cat.category}
                      </strong>
                      <div
                        style={{
                          fontSize: "0.85rem",
                          color: "var(--text-muted)",
                        }}
                      >
                        {cat.item_count}{" "}
                        {locale === "zh-CN" ? "项 · " : "items · "}
                        {formatBytes(cat.total_bytes)}
                      </div>
                    </div>
                  </div>
                  {renderDispositionBadge(cat.disposition)}
                </label>
              );
            })}
          </div>

          {manifest.sensitive_findings.length > 0 && (
            <div className="alert-banner warn">
              <strong>{t.sensitiveContentWarningTitle}</strong>
              <p style={{ margin: "4px 0 8px" }}>
                {t.sensitiveContentWarningBody}
              </p>
              <ul style={{ margin: "0 0 8px", paddingLeft: 20 }}>
                {manifest.sensitive_findings.slice(0, 6).map((sf) => (
                  <li key={sf.relative_path}>
                    <code>{sf.relative_path}</code> — {sf.description}
                  </li>
                ))}
              </ul>
              <label style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <input
                  type="checkbox"
                  checked={includeSensitive}
                  onChange={(e) => setIncludeSensitive(e.target.checked)}
                />
                <span>{t.includeSensitiveLabel}</span>
              </label>
            </div>
          )}

          {/* Collapsible technical diagnostic details */}
          <details style={{ marginTop: 12 }}>
            <summary style={{ cursor: "pointer", fontSize: "0.9rem" }}>
              {t.diagnosticsSummaryLabel}
            </summary>
            <div className="mono-code" style={{ marginTop: 8 }}>
              <div>Adapter: {manifest.adapter_capabilities.adapter_id}</div>
              <div>
                State DB:{" "}
                {manifest.source_env.sqlite_schemas.state_db_filename ?? "None"}{" "}
                (migration v
                {manifest.source_env.sqlite_schemas.state_migration_version ??
                  0}
                )
              </div>
              <div>
                Threads: {manifest.threads.length} · Projects:{" "}
                {manifest.projects.length}
              </div>
            </div>
          </details>

          <div className="wizard-footer">
            <button
              type="button"
              className="btn"
              onClick={() => setStep(1)}
            >
              {t.backButton}
            </button>
            {role === "sender" ? (
              <button
                type="button"
                className="btn btn-primary"
                disabled={selectedCategories.length === 0}
                onClick={() => void handleSenderStartStreamingTransfer()}
              >
                {t.startMigrationButton}
              </button>
            ) : (
              <button
                type="button"
                className="btn btn-primary"
                disabled={selectedCategories.length === 0}
                onClick={() => void handlePrepareProjectMappingsStep()}
              >
                {t.nextButton}
              </button>
            )}
          </div>
        </main>
      )}

      {/* ==================== STEP 3: PROJECT PATH MAPPING (RECEIVER) ==================== */}
      {role === "receiver" && step === 3 && (
        <main className="surface-card">
          <h2 className="section-title">{t.projectMappingTitle}</h2>
          <p className="section-desc">{t.projectMappingDesc}</p>

          <div
            style={{
              display: "grid",
              gridTemplateColumns: "1fr auto",
              gap: 10,
              alignItems: "end",
              marginBottom: 20,
            }}
          >
            <div className="field-row" style={{ marginBottom: 0 }}>
              <label htmlFor="target-proj-root">
                {t.targetProjectsRootLabel}
              </label>
              <input
                id="target-proj-root"
                className="field-input"
                value={defaultProjectsRoot}
                onChange={(e) => setDefaultProjectsRoot(e.target.value)}
              />
            </div>
            <button
              type="button"
              className="btn"
              onClick={async () => {
                const proposed =
                  await apiProposeProjectMappings(defaultProjectsRoot);
                setProjectMappings(proposed);
              }}
            >
              {t.applyDefaultRootButton}
            </button>
          </div>

          <div className="item-list">
            {projectMappings.map((pm, idx) => (
              <div key={pm.project_id} className="item-row">
                <div style={{ flex: 1 }}>
                  <strong>{pm.project_name}</strong>
                  <div
                    style={{
                      fontSize: "0.82rem",
                      color: "var(--text-muted)",
                      marginBottom: 6,
                    }}
                  >
                    {locale === "zh-CN" ? "旧电脑路径：" : "Old Path: "}
                    {pm.source_root}
                  </div>
                  <input
                    className="field-input"
                    value={pm.target_root}
                    onChange={(e) => {
                      const val = e.target.value;
                      setProjectMappings((prev) =>
                        prev.map((item, i) =>
                          i === idx ? { ...item, target_root: val } : item
                        )
                      );
                    }}
                  />
                  <div
                    style={{
                      display: "flex",
                      gap: 16,
                      marginTop: 8,
                      fontSize: "0.88rem",
                    }}
                  >
                    <label>
                      <input
                        type="checkbox"
                        checked={pm.copy_project_files}
                        onChange={(e) => {
                          const checked = e.target.checked;
                          setProjectMappings((prev) =>
                            prev.map((item, i) =>
                              i === idx
                                ? { ...item, copy_project_files: checked }
                                : item
                            )
                          );
                        }}
                      />{" "}
                      {locale === "zh-CN"
                        ? "复制项目文件夹"
                        : "Copy project files"}
                    </label>
                    <label>
                      <input
                        type="checkbox"
                        checked={pm.bind_to_existing_target_dir}
                        onChange={(e) => {
                          const checked = e.target.checked;
                          setProjectMappings((prev) =>
                            prev.map((item, i) =>
                              i === idx
                                ? {
                                    ...item,
                                    bind_to_existing_target_dir: checked,
                                  }
                                : item
                            )
                          );
                        }}
                      />{" "}
                      {locale === "zh-CN"
                        ? "允许绑定到本机已存在的同名项目目录"
                        : "Bind chats to existing target directory"}
                    </label>
                  </div>
                </div>
              </div>
            ))}
          </div>

          <div className="wizard-footer">
            <button
              type="button"
              className="btn"
              onClick={() => setStep(2)}
            >
              {t.backButton}
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => void handlePrepareSummaryStep()}
            >
              {t.nextButton}
            </button>
          </div>
        </main>
      )}

      {/* ==================== STEP 4: PRE-MIGRATION SUMMARY (RECEIVER) ==================== */}
      {role === "receiver" && step === 4 && importPlan && (
        <main className="surface-card">
          <h2 className="section-title">{t.summaryTitle}</h2>
          <p className="section-desc">{t.summaryDesc}</p>

          <div className="item-list" style={{ marginBottom: 16 }}>
            <div className="item-row">
              <span>
                {locale === "zh-CN"
                  ? "目标 Codex 目录："
                  : "Target Codex Directory:"}
              </span>
              <code>{importPlan.target_codex_home}</code>
            </div>
            <div className="item-row">
              <span>
                {locale === "zh-CN"
                  ? "会话处理策略："
                  : "Chat Merge Plan:"}
              </span>
              <span>
                {
                  importPlan.thread_actions.filter(
                    (a) => a.resolution === "ImportNew"
                  ).length
                }{" "}
                {locale === "zh-CN" ? "新增 · " : "new · "}
                {
                  importPlan.thread_actions.filter(
                    (a) => a.resolution === "DeduplicateIdentical"
                  ).length
                }{" "}
                {locale === "zh-CN" ? "相同去重 · " : "deduplicated · "}
                {
                  importPlan.thread_actions.filter(
                    (a) => a.resolution === "KeepBothForked"
                  ).length
                }{" "}
                {locale === "zh-CN"
                  ? "同 ID 分叉并存保护"
                  : "forked chats preserved"}
              </span>
            </div>
            <div className="item-row">
              <span>
                {locale === "zh-CN"
                  ? "磁盘空间预检（含备份与最终写入）："
                  : "Disk Space Pre-check:"}
              </span>
              <span
                className={`badge ${
                  importPlan.space_report.sufficient ? "success" : "error"
                }`}
              >
                {locale === "zh-CN" ? "需 " : "Needs "}
                {formatBytes(importPlan.space_report.total_required_bytes)} /{" "}
                {locale === "zh-CN" ? "可用 " : "Available "}
                {formatBytes(importPlan.space_report.available_bytes)}
              </span>
            </div>
          </div>

          {importPlan.validation_issues.length > 0 && (
            <div className="alert-banner warn">
              <strong>
                {locale === "zh-CN"
                  ? "路径与文件系统检查发现的问题："
                  : "Path & Filesystem Validation Findings:"}
              </strong>
              <ul style={{ margin: "6px 0 0", paddingLeft: 20 }}>
                {importPlan.validation_issues.map((iss, i) => (
                  <li key={i}>
                    [{iss.blocking ? "BLOCKING" : "WARN"}] {iss.description}
                  </li>
                ))}
              </ul>
            </div>
          )}

          <div className="wizard-footer">
            <button
              type="button"
              className="btn"
              onClick={() => setStep(3)}
            >
              {t.backButton}
            </button>
            <button
              type="button"
              className="btn btn-primary"
              disabled={!importPlan.can_proceed}
              onClick={() => void handleReceiverExecuteApply()}
            >
              {t.startMigrationButton}
            </button>
          </div>
        </main>
      )}

      {/* ==================== STEP 5: PROGRESS ==================== */}
      {role !== "home" && step === 5 && (
        <main className="surface-card">
          <h2 className="section-title">
            {role === "sender"
              ? t.transferProgressTitle
              : t.importProgressTitle}
          </h2>

          {role === "sender" && (
            <div>
              <progress
                value={
                  transferProgress && transferProgress.totalBytes > 0
                    ? transferProgress.processedBytes
                    : undefined
                }
                max={
                  transferProgress && transferProgress.totalBytes > 0
                    ? transferProgress.totalBytes
                    : undefined
                }
              />
              <p style={{ marginTop: 10 }}>
                {transferProgress
                  ? `${transferProgress.processedFiles} / ${transferProgress.totalFiles} · ${formatBytes(
                      transferProgress.processedBytes
                    )} / ${formatBytes(transferProgress.totalBytes)}`
                  : locale === "zh-CN"
                  ? "正在生成一致性源快照并校验分块哈希..."
                  : "Creating consistent snapshot and computing chunk hashes..."}
              </p>
              {transferProgress?.currentItem && (
                <div className="mono-code">{transferProgress.currentItem}</div>
              )}
            </div>
          )}

          {role === "receiver" && (
            <div>
              <progress
                value={
                  importProgress && importProgress.totalBytes > 0
                    ? importProgress.processedBytes
                    : undefined
                }
                max={
                  importProgress && importProgress.totalBytes > 0
                    ? importProgress.totalBytes
                    : undefined
                }
              />
              <p style={{ marginTop: 10 }}>
                {importProgress
                  ? `[${importProgress.phase}] ${importProgress.processedFiles} / ${importProgress.totalFiles} · ${formatBytes(
                      importProgress.processedBytes
                    )} / ${formatBytes(importProgress.totalBytes)}`
                  : locale === "zh-CN"
                  ? "正在备份目标数据并执行结构化合并..."
                  : "Backing up target data and applying structured merge..."}
              </p>
              {importProgress?.currentItem && (
                <div className="mono-code">{importProgress.currentItem}</div>
              )}
            </div>
          )}
        </main>
      )}

      {/* ==================== STEP 6: VERIFICATION & RESULTS ==================== */}
      {role !== "home" && step === 6 && (
        <main className="surface-card">
          {role === "sender" ? (
            <div>
              <div className="alert-banner success">
                <h2 className="section-title" style={{ margin: 0 }}>
                  {locale === "zh-CN"
                    ? "发送完成，新电脑已完成整包 SHA-256 校验！"
                    : "Transfer Complete — Verified by Receiving Computer!"}
                </h2>
                <p style={{ margin: "6px 0 0" }}>
                  {locale === "zh-CN"
                    ? "本机原始 Codex 数据与项目文件未做任何删除或修改。现在请在新电脑上完成路径确认与导入。"
                    : "Your source Codex data on this computer remains untouched. Complete the import step on your new computer."}
                </p>
              </div>
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => void handleReturnHome()}
              >
                {locale === "zh-CN" ? "完成并返回首页" : "Done"}
              </button>
            </div>
          ) : (
            executionResult && (
              <div>
                <div
                  className={`alert-banner ${
                    executionResult.outcome === "Succeeded"
                      ? "success"
                      : executionResult.outcome ===
                        "PartiallyCompletedWithWarnings"
                      ? "warn"
                      : "error"
                  }`}
                >
                  <h2 className="section-title" style={{ margin: 0 }}>
                    {executionResult.outcome === "Succeeded"
                      ? t.resultSucceededTitle
                      : executionResult.outcome ===
                        "PartiallyCompletedWithWarnings"
                      ? t.resultPartialTitle
                      : executionResult.outcome === "FailedAndRolledBack"
                      ? t.resultRolledBackTitle
                      : t.resultFailedPartialTitle}
                  </h2>
                  <p style={{ margin: "6px 0 0" }}>
                    {locale === "zh-CN"
                      ? `已应用文件：${executionResult.applied_files_count} · 新增会话：${executionResult.merged_threads_count} · 相同去重：${executionResult.deduplicated_threads_count} · 分叉并存：${executionResult.forked_threads_count}`
                      : `Applied files: ${executionResult.applied_files_count} · Merged chats: ${executionResult.merged_threads_count} · Deduplicated: ${executionResult.deduplicated_threads_count} · Forked preserved: ${executionResult.forked_threads_count}`}
                  </p>
                </div>

                {executionResult.verification && (
                  <>
                    <h3 className="section-title">{t.automatedChecksTitle}</h3>
                    <div className="item-list" style={{ marginBottom: 16 }}>
                      {executionResult.verification.automated_checks.map(
                        (chk, idx) => (
                          <div key={idx} className="item-row">
                            <div>
                              <strong>{chk.check_name}</strong>
                              <div
                                style={{
                                  fontSize: "0.85rem",
                                  color: "var(--text-muted)",
                                }}
                              >
                                {chk.detail}
                              </div>
                            </div>
                            <span
                              className={`badge ${
                                chk.passed ? "success" : "error"
                              }`}
                            >
                              {chk.passed ? "PASS" : "WARN"}
                            </span>
                          </div>
                        )
                      )}
                    </div>

                    <h3 className="section-title">
                      {t.manualConfirmationsTitle}
                    </h3>
                    <ul style={{ marginBottom: 16 }}>
                      {executionResult.verification.manual_codex_confirmations_needed.map(
                        (item, i) => (
                          <li key={i}>{item}</li>
                        )
                      )}
                    </ul>

                    <h3 className="section-title">
                      {t.pendingDependenciesTitle}
                    </h3>
                    <ul style={{ marginBottom: 16 }}>
                      {executionResult.verification.pending_dependencies_and_reauth.map(
                        (item, i) => (
                          <li key={i}>{item}</li>
                        )
                      )}
                    </ul>
                  </>
                )}

                <div className="wizard-footer">
                  <button
                    type="button"
                    className="btn btn-danger"
                    onClick={() =>
                      openRollbackModal(executionResult.migration_id)
                    }
                  >
                    {t.rollbackButton}
                  </button>
                  <button
                    type="button"
                    className="btn btn-primary"
                    onClick={() => void handleReturnHome()}
                  >
                    {locale === "zh-CN" ? "完成" : "Finish"}
                  </button>
                </div>
              </div>
            )
          )}
        </main>
      )}

      {/* Native <dialog> modal for Rollback Confirmation (modern-web-guidance `dialog`) */}
      <dialog
        ref={rollbackDialogRef}
        aria-labelledby={rollbackDialogTitleId}
        onClose={() => void handleRollbackDialogClose()}
      >
        <h2 id={rollbackDialogTitleId} style={{ marginTop: 0 }}>
          {t.rollbackConfirmDialogTitle}
        </h2>
        <p>{t.rollbackConfirmDialogBody}</p>
        <form
          method="dialog"
          style={{
            display: "flex",
            justifyContent: "flex-end",
            gap: 10,
            marginTop: 20,
          }}
        >
          <button type="submit" value="cancel" className="btn" autoFocus>
            {t.cancelButton}
          </button>
          <button type="submit" value="confirm" className="btn btn-danger">
            {t.rollbackButton}
          </button>
        </form>
      </dialog>

      <footer className="app-footer">
        <span>{t.communityDisclaimer}</span>
        <span>Codex Move v0.1.0 · Apache-2.0</span>
      </footer>
    </div>
  );
}
