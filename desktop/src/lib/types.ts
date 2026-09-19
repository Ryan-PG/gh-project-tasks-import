/**
 * TypeScript mirrors of the Rust DTOs in `src-tauri/src/commands.rs`.
 *
 * Field names are camelCase because every DTO the frontend receives is
 * serialised with `#[serde(rename_all = "camelCase")]`. The one exception is
 * `Config`, which is also the on-disk `config.json` format and therefore keeps
 * the CLI's snake_case `project_owner` key.
 */

/** `config.json`, shared byte-for-byte with the Python CLI. */
export interface Config {
  repo: string;
  project: string;
  /** The user or organisation owning the Project — not necessarily the repo owner. */
  project_owner: string;
}

export interface AppInfo {
  version: string;
  /** True when a `portable.txt` marker sits beside the executable. */
  portable: boolean;
  baseDir: string;
  configPath: string;
  tasksPath: string;
  statePath: string;
  stateExists: boolean;
  /** Client id compiled into this build, if any. */
  builtinClientId: string | null;
  ghCliAvailable: boolean;
}

export interface ValidationReport {
  ok: boolean;
  errors: string[];
  taskCount: number;
}

export type RelationshipErrors = "strict" | "ignore";

export interface Settings {
  relationshipErrors: RelationshipErrors;
  relationshipErrorsSource: string;
  skipRelationships: boolean;
  skipRelationshipsSource: string;
  /** One line per setting whose value was rejected; the CLI prints these to stderr. */
  warnings: string[];
}

export interface PreviewRow {
  id: string;
  priority: string;
  module: string;
  title: string;
  labels: string[];
  dependsOn: string[];
  parent: string | null;
}

export interface Workspace {
  config: Config | null;
  configError: string | null;
  tasksSource: string | null;
  tasksError: string | null;
  taskCount: number;
  validation: ValidationReport | null;
  preview: PreviewRow[];
  settings: Settings;
  settingsBanner: string;
}

export interface AuthStatus {
  signedIn: boolean;
  login: string | null;
  name: string | null;
  method: string | null;
  scopes: string[];
  /** Empty for fine-grained tokens, which do not report scopes. */
  missingScopes: string[];
  clientId: string;
  rememberToken: boolean;
  vaultError: string | null;
}

export interface DeviceCodeInfo {
  /** The 8-character code to type at `verificationUri`. */
  userCode: string;
  verificationUri: string;
  /** Seconds until the code expires; GitHub issues 15-minute codes. */
  expiresIn: number;
  interval: number;
  clientId: string;
}

export interface ImportRequest {
  dryRun: boolean;
  skipRelationships: boolean;
  skipProject: boolean;
}

export interface ImportSummary {
  created: number;
  skipped: number;
  tracked: number;
  labelsCreated: string[];
  projectItemsAdded: number;
  relationshipsApplied: number;
  relationshipsAlready: number;
  relationshipsFailed: number;
  alreadyIds: string[];
  failedIds: string[];
  warnings: string[];
  dryRun: boolean;
}

export type Phase =
  | "preflight"
  | "labels"
  | "issues"
  | "project"
  | "relationships"
  | "finished";

export type TaskStatus =
  | "created"
  | "skipped"
  | "added"
  | "already_set"
  | "failed"
  | "would_create"
  | "would_add";

/** Streamed on the `import://progress` event. */
export type ImportEvent =
  | { kind: "phase"; phase: Phase; message: string }
  | {
      kind: "task";
      phase: Phase;
      index: number;
      total: number;
      id: string;
      status: TaskStatus;
      detail: string | null;
    }
  | { kind: "log"; message: string }
  | { kind: "done"; summary: ImportSummary }
  | { kind: "failed"; message: string };

/** Streamed on the `auth://device` event while a device flow is pending. */
export interface DeviceFlowEvent {
  status: "waiting" | "approved" | "expired" | "failed";
  secondsRemaining?: number;
  message?: string;
}

export interface RunReport {
  summary: ImportSummary;
  cliOutput: string;
}

export interface TaskIssueStatus {
  id: string;
  number: number;
  onProject: boolean;
}

export const PHASE_LABELS: Record<Phase, string> = {
  preflight: "Checking access",
  labels: "Creating labels",
  issues: "Creating issues",
  project: "Adding to the project board",
  relationships: "Applying relationships",
  finished: "Finished",
};

export const STATUS_LABELS: Record<TaskStatus, string> = {
  created: "Created",
  skipped: "Already exists",
  added: "Applied",
  already_set: "Already set",
  failed: "Failed",
  would_create: "Would create",
  would_add: "Would apply",
};
