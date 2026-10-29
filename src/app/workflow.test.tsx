// @vitest-environment jsdom
import { act } from "react";
import { createRoot, Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "./ipc";
import { App } from "./App";

const mockManifest: ipc.MigrationManifest = {
  format_version: "1.0.0",
  migration_id: "mig-0196ed4c",
  app_version: "0.1.0",
  created_at: "2026-10-29T08:00:00Z",
  source_env: {
    os: "windows",
    arch: "x86_64",
    codex_home: "C:\\Users\\alice\\.codex",
    shared_skills_dir: null,
    sqlite_schemas: {
      state_db_filename: "state_5.sqlite",
      state_schema_version: 5,
      state_migration_version: 38,
      thread_history_db_filename: "thread_history_1.sqlite",
      thread_history_migration_version: 4,
      memories_db_filename: "memories_1.sqlite",
      goals_db_filename: "goals_1.sqlite",
      has_active_wal: false,
      unknown_state_databases: [],
    },
  },
  adapter_capabilities: {
    adapter_id: "codex-adapter-v1",
    can_read_threads: true,
    can_write_threads: true,
    can_map_paths: true,
    supported_state_versions: [4, 5],
    unknown_tables: [],
    compatibility_notes: ["Detected state_5.sqlite"],
  },
  categories: [
    {
      category: "ActiveAndArchivedChats",
      disposition: "NeedsPathMapping",
      item_count: 3,
      total_bytes: 65536,
      selected_by_default: true,
      notes: [],
    },
    {
      category: "ChatMetadataAndIndex",
      disposition: "NeedsPathMapping",
      item_count: 4,
      total_bytes: 131072,
      selected_by_default: true,
      notes: [],
    },
    {
      category: "ProjectFiles",
      disposition: "NeedsPathMapping",
      item_count: 12,
      total_bytes: 524288,
      selected_by_default: true,
      notes: [],
    },
  ],
  threads: [
    {
      id: "0196ed4c-1111-7000-8000-000000000001",
      title: "Cross-Platform Rust Architecture",
      cwd: "C:\\Users\\alice\\repos\\demo",
      rollout_path: "C:\\Users\\alice\\.codex\\sessions\\rollout-1.jsonl",
      updated_at: 1700000100,
      archived: false,
    },
  ],
  projects: [
    {
      project_id: "proj-1",
      name: "demo",
      source_root_paths: ["C:\\Users\\alice\\repos\\demo"],
      exists_on_disk: true,
      include_project_files: true,
      git_status: "StandardRepo",
      has_uncommitted_changes: false,
      has_untracked_files: true,
      total_files: 12,
      total_bytes: 524288,
      sensitive_findings: [],
      nested_under_project_id: null,
    },
  ],
  excluded_items: [],
  skill_warnings: [],
  sensitive_findings: [],
  general_warnings: [],
};

describe("Codex Move End-to-End Frontend Workflow State Transitions", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.useFakeTimers();
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    vi.spyOn(ipc, "apiGetSystemOverview").mockResolvedValue({
      os: "macos",
      arch: "aarch64",
      hostname_display: "MacBook-Pro",
      default_codex_home: "/Users/bob/.codex",
      candidate_codex_homes: ["/Users/bob/.codex"],
      shared_agents_skills_dir: "/Users/bob/.agents/skills",
      active_codex_processes: [],
      webview2_runtime_version: null,
      available_disk_bytes: 128 * 1024 * 1024 * 1024,
      supports_symlinks: true,
    });
    vi.spyOn(ipc, "apiDiscoverRecoveryJournals").mockResolvedValue([]);
    vi.spyOn(ipc, "apiStopNetworkServices").mockResolvedValue();
  });

  afterEach(async () => {
    await act(async () => {
      root.unmount();
    });
    container.remove();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("Receiver workflow loads cached manifest on transfer completion and completes Steps 1 -> 2 -> 3 -> 4 -> 6", async () => {
    vi.spyOn(ipc, "apiStartReceiverMode").mockResolvedValue({
      ticket: {
        session_id: "sess-recv-1",
        host_ip: "192.168.1.88",
        port: 53317,
        cert_fingerprint_sha256:
          "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        high_entropy_token_b64: "token123",
        short_pake_code: "7K9M2P",
        pake_salt_hex: "00112233",
        connection_code_string:
          "cdxmove://sess-recv-1@192.168.1.88:53317/abcdef0123456789?t=token123",
        qr_svg: "<svg></svg>",
        expires_in_seconds: 300,
      },
      staging_dir: "/tmp/codex-move-staging/sess-recv-1",
    });

    const getSnapshotSpy = vi
      .spyOn(ipc, "apiGetReceiverSessionSnapshot")
      .mockResolvedValue({
        pairing_approved: true,
        pending_sender_request: null,
        active_migration_id: "mig-0196ed4c",
        active_manifest_hash: "hash123",
        received_files_count: 19,
        total_files_count: 19,
        received_bytes: 720896,
        total_bytes: 720896,
        transfer_completed: true,
        staging_dir: "/tmp/codex-move-staging/sess-recv-1",
      });

    const getCachedManifestSpy = vi
      .spyOn(ipc, "apiGetCachedManifest")
      .mockResolvedValue(mockManifest);

    vi.spyOn(ipc, "apiProposeProjectMappings").mockResolvedValue([
      {
        project_id: "proj-1",
        project_name: "demo",
        source_root: "C:\\Users\\alice\\repos\\demo",
        target_root: "/Users/bob/CodexProjects/demo",
        copy_project_files: true,
        bind_to_existing_target_dir: false,
      },
    ]);

    vi.spyOn(ipc, "apiBuildImportPlan").mockResolvedValue({
      migration_id: "mig-0196ed4c",
      target_codex_home: "/Users/bob/.codex",
      target_shared_skills_dir: "/Users/bob/.agents/skills",
      target_has_existing_data: true,
      selected_categories: [
        "ActiveAndArchivedChats",
        "ChatMetadataAndIndex",
        "ProjectFiles",
      ],
      include_sensitive_files: false,
      project_mappings: [
        {
          project_id: "proj-1",
          project_name: "demo",
          source_root: "C:\\Users\\alice\\repos\\demo",
          target_root: "/Users/bob/CodexProjects/demo",
          copy_project_files: true,
          bind_to_existing_target_dir: false,
        },
      ],
      thread_actions: [
        {
          source_thread_id: "0196ed4c-1111-7000-8000-000000000001",
          target_thread_id: "0196ed4c-9999-7000-8000-000000000099",
          title: "Cross-Platform Rust Architecture (Migrated Copy)",
          resolution: "KeepBothForked",
          mapped_cwd: "/Users/bob/CodexProjects/demo",
          mapped_rollout_path:
            "/Users/bob/.codex/sessions/rollout-1-0196ed4c-9999-7000-8000-000000000099.jsonl",
        },
      ],
      config_conflicts: [],
      validation_issues: [],
      space_report: {
        staging_bytes_needed: 0,
        backup_bytes_needed: 102400,
        final_write_bytes_needed: 720896,
        total_required_bytes: 34379776,
        available_bytes: 68719476736,
        sufficient: true,
      },
      can_proceed: true,
      blocking_reasons: [],
    });

    vi.spyOn(ipc, "apiApplyMigration").mockResolvedValue({
      migration_id: "mig-0196ed4c",
      outcome: "Succeeded",
      applied_files_count: 19,
      merged_threads_count: 0,
      deduplicated_threads_count: 0,
      forked_threads_count: 1,
      warnings: [],
      verification: {
        all_automated_checks_passed: true,
        automated_checks: [
          {
            check_name: "SQLite Integrity (state_5.sqlite)",
            passed: true,
            detail: "ok",
          },
          {
            check_name: "Chat Rollout Transcript Parity",
            passed: true,
            detail:
              "All 1 migrated chat thread(s) link to valid transcript files with matching session IDs.",
          },
        ],
        residual_old_paths_found: [],
        manual_codex_confirmations_needed: [],
        pending_dependencies_and_reauth: [],
      },
      rollback_report: null,
      backup_dir: "/Users/bob/.codex-move-backups/mig-0196ed4c",
    });

    await act(async () => {
      root.render(<App />);
    });

    // Click Receiver card on Home screen
    const cards = Array.from(
      container.querySelectorAll("button.hero-action-card")
    );
    expect(cards.length).toBe(2);
    await act(async () => {
      (cards[1] as HTMLButtonElement).click();
    });

    // Step 1: Verify pairing code is displayed
    expect(container.textContent).toContain("7K9M2P");

    // Advance timer so receiver poll detects transfer_completed = true and loads manifest into Step 2
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });

    expect(getSnapshotSpy).toHaveBeenCalled();
    expect(getCachedManifestSpy).toHaveBeenCalled();

    // Step 2 must now be rendered with the received manifest categories!
    expect(container.querySelectorAll('input[type="checkbox"]').length).toBe(3);

    // Click Next to enter Step 3 (Project Path Mapping)
    const step2NextBtn = Array.from(
      container.querySelectorAll(".wizard-footer button.btn-primary")
    )[0] as HTMLButtonElement;
    expect(step2NextBtn).toBeTruthy();
    await act(async () => {
      step2NextBtn.click();
    });

    // Verify Step 3 shows the proposed project target root
    expect(container.textContent).toContain("C:\\Users\\alice\\repos\\demo");

    // Click Next to enter Step 4 (Pre-migration Summary)
    const step3NextBtn = Array.from(
      container.querySelectorAll(".wizard-footer button.btn-primary")
    )[0] as HTMLButtonElement;
    await act(async () => {
      step3NextBtn.click();
    });

    // Verify Step 4 shows the target_codex_home and start migration button
    expect(container.textContent).toContain("/Users/bob/.codex");
    const step4StartBtn = Array.from(
      container.querySelectorAll(".wizard-footer button.btn-primary")
    )[0] as HTMLButtonElement;
    await act(async () => {
      step4StartBtn.click();
    });

    // Verify Step 6 shows verification results
    expect(container.textContent).toContain("SQLite Integrity (state_5.sqlite)");
    expect(container.textContent).toContain("Chat Rollout Transcript Parity");
  });

  it("Sender workflow binds poll_token during pairing approval polling and executes streaming transfer", async () => {
    vi.spyOn(ipc, "apiScanCodexInventory").mockResolvedValue(mockManifest);
    vi.spyOn(ipc, "apiStartSenderDiscovery").mockResolvedValue();
    vi.spyOn(ipc, "apiListDiscoveredPeers").mockResolvedValue([]);

    const pairSpy = vi
      .spyOn(ipc, "apiSenderPairWithReceiver")
      .mockResolvedValue({
        receiver_base_url: "https://192.168.1.88:53317",
        receiver_cert_fingerprint: "abcdef0123456789",
        approved: false,
        poll_token: "authenticated-poll-secret-token-999",
        session_bearer_token: null,
        message: "Waiting for user confirmation",
      });

    const pollSpy = vi.spyOn(ipc, "apiSenderPollApproval").mockResolvedValue({
      approved: true,
      poll_token: "authenticated-poll-secret-token-999",
      session_bearer_token: "approved-bearer-token-777",
      receiver_cert_fingerprint: "abcdef0123456789",
      message: "Approved",
    });

    const transferSpy = vi
      .spyOn(ipc, "apiSenderExecuteTransfer")
      .mockResolvedValue("manifest-sha256-ok");

    await act(async () => {
      root.render(<App />);
    });

    // Click Sender card on Home screen
    const cards = Array.from(
      container.querySelectorAll("button.hero-action-card")
    );
    await act(async () => {
      (cards[0] as HTMLButtonElement).click();
    });

    // Enter connection code in #conn-code-input
    const codeInput = container.querySelector(
      "#conn-code-input"
    ) as HTMLInputElement;
    expect(codeInput).toBeTruthy();

    await act(async () => {
      const nativeSetter = Object.getOwnPropertyDescriptor(
        window.HTMLInputElement.prototype,
        "value"
      )?.set;
      nativeSetter?.call(
        codeInput,
        "cdxmove://sess-1@192.168.1.88:53317/abcdef0123456789?t=token123"
      );
      codeInput.dispatchEvent(new Event("input", { bubbles: true }));
    });

    const connectBtn = Array.from(
      container.querySelectorAll(".wizard-footer button.btn-primary")
    )[0] as HTMLButtonElement;
    await act(async () => {
      connectBtn.click();
    });

    expect(pairSpy).toHaveBeenCalled();

    // Advance timer so sender polls approval using the authenticated poll_token
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });

    expect(pollSpy).toHaveBeenCalledWith(
      "https://192.168.1.88:53317",
      "abcdef0123456789",
      "authenticated-poll-secret-token-999"
    );

    // Sender should now be on Step 2 (Select Content)
    const startTransferBtn = Array.from(
      container.querySelectorAll(".wizard-footer button.btn-primary")
    )[0] as HTMLButtonElement;
    expect(startTransferBtn).toBeTruthy();

    await act(async () => {
      startTransferBtn.click();
    });

    expect(transferSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        receiverBaseUrl: "https://192.168.1.88:53317",
        receiverCertFingerprint: "abcdef0123456789",
        bearerToken: "approved-bearer-token-777",
      })
    );
  });
});
