/**
 * Typed wrappers around the Rust commands.
 *
 * Every GitHub call happens in Rust; this module is the entire network-ish
 * surface the webview has, and it only ever crosses the IPC boundary.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppInfo,
  AuthStatus,
  Config,
  DeviceCodeInfo,
  DeviceFlowEvent,
  ImportEvent,
  ImportRequest,
  RunReport,
  TaskIssueStatus,
  Workspace,
} from "./types";

/** Event the importer streams progress on. Mirrors `commands::PROGRESS_EVENT`. */
export const PROGRESS_EVENT = "import://progress";
/** Event the device-flow poller streams status on. */
export const AUTH_EVENT = "auth://device";

export const api = {
  // ---- Workspace ---------------------------------------------------------
  appInfo: () => invoke<AppInfo>("app_info"),
  loadWorkspace: () => invoke<Workspace>("load_workspace"),
  loadTasksFromPath: (path: string) =>
    invoke<Workspace>("load_tasks_from_path", { path }),
  revalidate: () => invoke<Workspace>("revalidate"),
  saveConfig: (config: Config) => invoke<Workspace>("save_config", { config }),
  workspacePaths: () => invoke<Record<string, string | boolean>>("workspace_paths"),
  revealWorkspace: () => invoke<void>("reveal_workspace"),
  taskStatuses: () => invoke<string[]>("task_statuses"),

  // ---- Authentication ----------------------------------------------------
  authStatus: () => invoke<AuthStatus>("auth_status"),
  setClientId: (clientId: string) =>
    invoke<AuthStatus>("set_client_id", { clientId }),
  startDeviceFlow: () => invoke<DeviceCodeInfo>("auth_start_device_flow"),
  pollDeviceFlow: () => invoke<AuthStatus>("auth_poll_device_flow"),
  cancelDeviceFlow: () => invoke<void>("auth_cancel_device_flow"),
  setPat: (token: string) => invoke<AuthStatus>("auth_set_pat", { token }),
  importFromGh: () => invoke<AuthStatus>("auth_import_gh"),
  setRemember: (remember: boolean) =>
    invoke<AuthStatus>("auth_set_remember", { remember }),
  signOut: () => invoke<AuthStatus>("auth_sign_out"),

  // ---- Import ------------------------------------------------------------
  startImport: (request: ImportRequest) =>
    invoke<void>("import_start", { request }),
  cancelImport: () => invoke<void>("import_cancel"),
  dryRunPreview: () => invoke<RunReport>("import_dry_run_preview"),
  importStatuses: () => invoke<TaskIssueStatus[]>("import_statuses"),
};

/** Subscribe to import progress. Returns the unlisten function. */
export function onImportProgress(
  handler: (event: ImportEvent) => void,
): Promise<UnlistenFn> {
  return listen<ImportEvent>(PROGRESS_EVENT, (e) => handler(e.payload));
}

/** Subscribe to device-flow status. Returns the unlisten function. */
export function onDeviceFlow(
  handler: (event: DeviceFlowEvent) => void,
): Promise<UnlistenFn> {
  return listen<DeviceFlowEvent>(AUTH_EVENT, (e) => handler(e.payload));
}

/**
 * Tauri rejects with a plain string from `Result::Err(String)`, but a thrown
 * JS error can arrive as an `Error`. Normalise both to a displayable message.
 */
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) {
    return String((e as { message: unknown }).message);
  }
  return String(e);
}
