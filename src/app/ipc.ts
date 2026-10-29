import { invoke } from "@tauri-apps/api/core";

export type ContentCategory =
  | "ActiveAndArchivedChats"
  | "ChatMetadataAndIndex"
  | "UserSkills"
  | "SharedAgentSkills"
  | "InstructionFiles"
  | "CoreConfig"
  | "ExtensionsMemoriesRulesHooks"
  | "ChatAttachments"
  | "ProjectFiles"
  | "PluginManifests";

export type MigrationDisposition =
  | "DirectCopy"
  | "NeedsPathMapping"
  | "NeedsReinstallOnTarget"
  | "NeedsReauthOnTarget"
  | "UnsupportedInCurrentVersion";

export interface ActiveCodexProcess {
  pid: number;
  name: string;
}

export interface SystemOverview {
  os: string;
  arch: string;
  hostname_display: string;
  default_codex_home: string | null;
  candidate_codex_homes: string[];
  shared_agents_skills_dir: string | null;
  active_codex_processes: ActiveCodexProcess[];
  webview2_runtime_version: string | null;
  available_disk_bytes: number;
  supports_symlinks: boolean;
}

export interface CategorySummary {
  category: ContentCategory;
  disposition: MigrationDisposition;
  item_count: number;
  total_bytes: number;
  selected_by_default: boolean;
  notes: string[];
}

export interface SensitiveFinding {
  relative_path: string;
  finding_kind: string;
  description: string;
  selected_by_user: boolean;
}

export interface SkillDependencyWarning {
  skill_name: string;
  relative_path: string;
  warning_kind: string;
  detail: string;
}

export interface ProjectInventoryEntry {
  project_id: string;
  name: string;
  source_root_paths: string[];
  exists_on_disk: boolean;
  include_project_files: boolean;
  git_status:
    | "NoGit"
    | "StandardRepo"
    | { ExternalWorktree: { gitdir_pointer: string } }
    | { HasSubmodules: { count: number } };
  has_uncommitted_changes: boolean;
  has_untracked_files: boolean;
  total_files: number;
  total_bytes: number;
  sensitive_findings: SensitiveFinding[];
  nested_under_project_id: string | null;
}

export interface SqliteSchemaReport {
  state_db_filename: string | null;
  state_schema_version: number | null;
  state_migration_version: number | null;
  thread_history_db_filename: string | null;
  thread_history_migration_version: number | null;
  memories_db_filename: string | null;
  goals_db_filename: string | null;
  has_active_wal: boolean;
  unknown_state_databases: string[];
}

export interface AdapterCapabilities {
  adapter_id: string;
  can_read_threads: boolean;
  can_write_threads: boolean;
  can_map_paths: boolean;
  supported_state_versions: number[];
  unknown_tables: string[];
  compatibility_notes: string[];
}

export interface MigrationManifest {
  format_version: string;
  migration_id: string;
  app_version: string;
  created_at: string;
  source_env: {
    os: string;
    arch: string;
    codex_home: string;
    shared_skills_dir: string | null;
    sqlite_schemas: SqliteSchemaReport;
  };
  adapter_capabilities: AdapterCapabilities;
  categories: CategorySummary[];
  threads: Array<{
    id: string;
    title: string;
    cwd: string;
    rollout_path: string;
    updated_at: number;
    archived: boolean;
  }>;
  projects: ProjectInventoryEntry[];
  excluded_items: Array<{
    logical_path: string;
    reason: string;
    disposition: MigrationDisposition;
  }>;
  skill_warnings: SkillDependencyWarning[];
  sensitive_findings: SensitiveFinding[];
  general_warnings: string[];
}

export interface ProjectPathMappingConfig {
  project_id: string;
  project_name: string;
  source_root: string;
  target_root: string;
  copy_project_files: boolean;
  bind_to_existing_target_dir: boolean;
}

export interface PathValidationIssue {
  kind: string;
  relative_or_target_path: string;
  conflicting_with: string | null;
  blocking: boolean;
  description: string;
}

export interface ImportPlan {
  migration_id: string;
  target_codex_home: string;
  target_shared_skills_dir: string | null;
  target_has_existing_data: boolean;
  selected_categories: ContentCategory[];
  include_sensitive_files: boolean;
  project_mappings: ProjectPathMappingConfig[];
  thread_actions: Array<{
    source_thread_id: string;
    target_thread_id: string;
    title: string;
    resolution: "DeduplicateIdentical" | "ImportNew" | "KeepBothForked";
    mapped_cwd: string;
    mapped_rollout_path: string;
  }>;
  config_conflicts: Array<{
    field_path: string;
    source_summary: string;
    target_summary: string;
    resolution: string;
  }>;
  validation_issues: PathValidationIssue[];
  space_report: {
    staging_bytes_needed: number;
    backup_bytes_needed: number;
    final_write_bytes_needed: number;
    total_required_bytes: number;
    available_bytes: number;
    sufficient: boolean;
  };
  can_proceed: boolean;
  blocking_reasons: string[];
}

export interface ReceiverPairingTicket {
  session_id: string;
  host_ip: string;
  port: number;
  cert_fingerprint_sha256: string;
  high_entropy_token_b64: string;
  short_pake_code: string;
  pake_salt_hex: string;
  connection_code_string: string;
  qr_svg: string;
  expires_in_seconds: number;
}

export interface DiscoveredPeer {
  session_id: string;
  device_alias: string;
  os: string;
  arch: string;
  ip_address: string;
  https_port: number;
  cert_fingerprint_prefix: string;
  full_cert_fingerprint: string;
  pake_salt_hex: string;
  last_seen_ms_ago: number;
}

export interface PairingRequestPayload {
  sender_device_name: string;
  sender_os: string;
  sender_arch: string;
  summary_threads_count: number;
  summary_projects_count: number;
  summary_total_bytes: number;
}

export interface ReceiverSessionSnapshot {
  pairing_approved: boolean;
  pending_sender_request: PairingRequestPayload | null;
  sas_verification_code?: string | null;
  active_migration_id: string | null;
  active_manifest_hash: string | null;
  received_files_count: number;
  total_files_count: number;
  received_bytes: number;
  total_bytes: number;
  transfer_completed: boolean;
  staging_dir: string;
}

export interface VerificationCheckItem {
  check_name: string;
  passed: boolean;
  detail: string;
}

export interface PostImportVerificationReport {
  all_automated_checks_passed: boolean;
  automated_checks: VerificationCheckItem[];
  residual_old_paths_found: string[];
  manual_codex_confirmations_needed: string[];
  pending_dependencies_and_reauth: string[];
}

export interface RollbackReport {
  migration_id: string;
  fully_restored: boolean;
  restored_files_count: number;
  removed_new_files_count: number;
  failed_paths: string[];
  retained_backup_dir: string;
}

export interface MigrationExecutionResult {
  migration_id: string;
  outcome:
    | "Succeeded"
    | "PartiallyCompletedWithWarnings"
    | "FailedAndRolledBack"
    | "FailedPartialRollback";
  applied_files_count: number;
  merged_threads_count: number;
  deduplicated_threads_count: number;
  forked_threads_count: number;
  warnings: string[];
  verification: PostImportVerificationReport | null;
  rollback_report: RollbackReport | null;
  backup_dir: string;
}

export interface RecoveryJournal {
  migration_id: string;
  started_at: string;
  updated_at: string;
  target_codex_home: string;
  backup_dir: string;
  status:
    | "BackupCreated"
    | "ApplyingChanges"
    | "Completed"
    | "RolledBack"
    | "RollbackPartialFailure";
  current_checkpoint: string;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function apiGetSystemOverview(): Promise<SystemOverview> {
  if (!isTauriRuntime()) {
    return {
      os: "windows",
      arch: "x86_64",
      hostname_display: "Local-Computer",
      default_codex_home: "C:\\Users\\user\\.codex",
      candidate_codex_homes: ["C:\\Users\\user\\.codex"],
      shared_agents_skills_dir: "C:\\Users\\user\\.agents\\skills",
      active_codex_processes: [],
      webview2_runtime_version: "Microsoft Edge WebView2 (Evergreen)",
      available_disk_bytes: 64 * 1024 * 1024 * 1024,
      supports_symlinks: true,
    };
  }
  return invoke<SystemOverview>("cmd_get_system_overview");
}

export async function apiScanCodexInventory(
  codexHome: string,
  includeProjectFiles: boolean
): Promise<MigrationManifest> {
  return invoke<MigrationManifest>("cmd_scan_codex_inventory", {
    options: {
      codex_home: codexHome,
      shared_skills_dir: null,
      include_project_files: includeProjectFiles,
      excluded_project_dirs: [
        "node_modules",
        "target",
        "dist",
        "build",
        ".next",
        "__pycache__",
        ".venv",
        "venv",
      ],
      compute_sha256: false,
    },
  });
}

export async function apiGetCachedManifest(): Promise<MigrationManifest> {
  return invoke<MigrationManifest>("cmd_get_cached_manifest");
}

export async function apiProposeProjectMappings(
  defaultProjectsRoot: string
): Promise<ProjectPathMappingConfig[]> {
  return invoke<ProjectPathMappingConfig[]>("cmd_propose_project_mappings", {
    defaultProjectsRoot,
  });
}

export async function apiBuildImportPlan(params: {
  targetCodexHome: string;
  targetSharedSkillsDir: string | null;
  selectedCategories: ContentCategory[];
  projectMappings: ProjectPathMappingConfig[];
  includeSensitiveFiles: boolean;
  stagingAlreadyComplete: boolean;
}): Promise<ImportPlan> {
  return invoke<ImportPlan>("cmd_build_import_plan", params);
}

export async function apiStartReceiverMode(
  deviceAlias: string,
  autoApprovePairing = false
): Promise<{ ticket: ReceiverPairingTicket; staging_dir: string }> {
  return invoke("cmd_start_receiver_mode", {
    deviceAlias,
    autoApprovePairing,
  });
}

export async function apiStartSenderDiscovery(): Promise<void> {
  if (!isTauriRuntime()) return;
  return invoke("cmd_start_sender_discovery");
}

export async function apiListDiscoveredPeers(): Promise<DiscoveredPeer[]> {
  if (!isTauriRuntime()) return [];
  return invoke<DiscoveredPeer[]>("cmd_list_discovered_peers");
}

export async function apiStopNetworkServices(): Promise<void> {
  if (!isTauriRuntime()) return;
  return invoke("cmd_stop_network_services");
}

export async function apiGetReceiverSessionSnapshot(): Promise<ReceiverSessionSnapshot | null> {
  if (!isTauriRuntime()) return null;
  return invoke<ReceiverSessionSnapshot | null>(
    "cmd_get_receiver_session_snapshot"
  );
}

export async function apiApproveReceiverPairing(): Promise<string> {
  return invoke<string>("cmd_approve_receiver_pairing");
}

export async function apiRejectReceiverPairing(): Promise<void> {
  return invoke("cmd_reject_receiver_pairing");
}

export async function apiSenderPairWithReceiver(req: {
  connection_code_or_short_code: string;
  manual_host_ip?: string | null;
  manual_port?: number | null;
  peer_cert_fingerprint?: string | null;
  peer_pake_salt_hex?: string | null;
  sender_device_name: string;
}): Promise<{
  receiver_base_url: string;
  receiver_cert_fingerprint: string;
  approved: boolean;
  poll_token: string | null;
  session_bearer_token: string | null;
  sas_verification_code?: string | null;
  message: string;
}> {
  return invoke("cmd_sender_pair_with_receiver", { req });
}

export async function apiSenderPollApproval(
  receiverBaseUrl: string,
  receiverCertFingerprint: string,
  pollToken: string
): Promise<{
  approved: boolean;
  poll_token: string | null;
  session_bearer_token: string | null;
  receiver_cert_fingerprint: string;
  sas_verification_code?: string | null;
  message: string;
}> {
  return invoke("cmd_sender_poll_approval", {
    receiverBaseUrl,
    receiverCertFingerprint,
    pollToken,
  });
}

export async function apiSenderExecuteTransfer(params: {
  receiverBaseUrl: string;
  receiverCertFingerprint: string;
  bearerToken: string;
  selectedCategories: ContentCategory[];
  includeSensitiveFiles: boolean;
}): Promise<string> {
  return invoke<string>("cmd_sender_execute_transfer", params);
}

export async function apiApplyMigration(
  stagingDir: string,
  plan: ImportPlan
): Promise<MigrationExecutionResult> {
  return invoke<MigrationExecutionResult>("cmd_apply_migration", {
    stagingDir,
    plan,
  });
}

export async function apiDiscoverRecoveryJournals(
  targetCodexHome: string
): Promise<RecoveryJournal[]> {
  if (!isTauriRuntime()) return [];
  return invoke<RecoveryJournal[]>("cmd_discover_recovery_journals", {
    targetCodexHome,
  });
}

export async function apiExecuteRollback(
  targetCodexHome: string,
  migrationId: string
): Promise<RollbackReport> {
  return invoke<RollbackReport>("cmd_execute_rollback", {
    targetCodexHome,
    migrationId,
  });
}

export async function apiExportEncryptedBundle(
  outputFile: string,
  passphrase: string
): Promise<string> {
  return invoke<string>("cmd_export_encrypted_bundle", {
    outputFile,
    passphrase,
  });
}

export async function apiImportEncryptedBundle(
  bundleFile: string,
  passphrase: string
): Promise<[MigrationManifest, string]> {
  return invoke<[MigrationManifest, string]>("cmd_import_encrypted_bundle", {
    bundleFile,
    passphrase,
  });
}
