//! The `#[tauri::command]` surface, and the DTOs the frontend consumes.
//!
//! All GitHub traffic happens in Rust; the frontend only ever sees these
//! commands. That is what keeps a token out of the webview, and why the CSP
//! needs no remote origins.

use crate::auth::{self, Account, AuthMethod, DeviceCodeInfo, PendingDeviceFlow, SecretToken};
use crate::github::{GithubClient, RepoRef};
use crate::graphql::Project;
use crate::importer::{ImportEvent, ImportOptions, ImportSummary, Importer, ProgressSink};
use crate::secrets::SecretStore;
use crate::state::{load_json, parse_config, AppPaths, Config, ImportState};
use crate::tasks::{self, Settings, Task};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

/// Event name the importer streams progress on.
pub const PROGRESS_EVENT: &str = "import://progress";
/// Event name the device-flow poller streams status on.
pub const AUTH_EVENT: &str = "auth://device";

/// Forwards progress to the webview.
pub struct TauriSink {
    app: AppHandle,
}

impl ProgressSink for TauriSink {
    fn emit(&self, event: ImportEvent) {
        // A failed emit means the window went away mid-run; the run itself is
        // still valid, so this is not an error worth surfacing.
        let _ = self.app.emit(PROGRESS_EVENT, &event);
    }
}

/// An authenticated session. The token is wrapped so it is zeroed on drop.
pub struct Session {
    pub token: SecretToken,
    pub account: Account,
    pub method: AuthMethod,
}

/// Preferences that are not part of the workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    /// OAuth App client id. Registration is manual; see the README.
    #[serde(default)]
    pub client_id: String,
    /// Remember the token between runs. When false it is memory-only.
    #[serde(default = "default_true")]
    pub remember_token: bool,
    /// Optional passphrase for the encrypted vault.
    #[serde(default)]
    pub vault_passphrase: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Prefs {
    fn path(paths: &AppPaths) -> PathBuf {
        paths.base_dir.join("desktop-settings.json")
    }

    fn load(paths: &AppPaths) -> Self {
        let mut prefs: Prefs = std::fs::read_to_string(Self::path(paths))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if prefs.client_id.is_empty() {
            if let Some(builtin) = auth::BUILTIN_CLIENT_ID {
                prefs.client_id = builtin.to_string();
            }
        }
        prefs
    }

    fn save(&self, paths: &AppPaths) -> Result<(), String> {
        let path = Self::path(paths);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())
    }
}

/// Application state managed by Tauri.
pub struct AppState {
    pub paths: AppPaths,
    pub session: Mutex<Option<Session>>,
    pub pending_flow: Mutex<Option<PendingDeviceFlow>>,
    pub prefs: Mutex<Prefs>,
    pub vault: Box<dyn SecretStore>,
    /// Set while a run is in flight, so a second run cannot start.
    pub running: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
    /// Parsed backlog, held here so the frontend never round-trips the file.
    pub tasks: Mutex<Option<LoadedTasks>>,
}

/// The parser's view of a tasks file.
#[derive(Debug, Clone)]
pub struct LoadedTasks {
    pub source: String,
    pub raw: Vec<Value>,
    pub parsed: Vec<Task>,
    pub report: tasks::ValidationReport,
    pub settings: Settings,
}

impl AppState {
    /// The token for the current session, if any.
    fn token(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.token.expose().to_string())
    }

    /// A client bound to the current session.
    fn client(&self) -> Result<GithubClient, String> {
        let token = self.token().ok_or("Not signed in.")?;
        Ok(GithubClient::new(token, None))
    }

    fn config(&self) -> Result<Config, String> {
        let v = load_json(&self.paths.config())?;
        parse_config(&v)
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub portable: bool,
    pub base_dir: String,
    pub config_path: String,
    pub tasks_path: String,
    pub state_path: String,
    pub state_exists: bool,
    pub builtin_client_id: Option<String>,
    pub gh_cli_available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub id: String,
    pub priority: String,
    pub module: String,
    pub title: String,
    pub labels: Vec<String>,
    pub depends_on: Vec<String>,
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub config: Option<Config>,
    pub config_error: Option<String>,
    pub tasks_source: Option<String>,
    pub tasks_error: Option<String>,
    pub task_count: usize,
    pub validation: Option<tasks::ValidationReport>,
    pub preview: Vec<PreviewRow>,
    pub settings: Settings,
    pub settings_banner: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub signed_in: bool,
    pub login: Option<String>,
    pub name: Option<String>,
    pub method: Option<String>,
    pub scopes: Vec<String>,
    pub missing_scopes: Vec<String>,
    pub client_id: String,
    pub remember_token: bool,
    /// Set when a stored token existed but the vault could not be opened.
    pub vault_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub skip_relationships: bool,
    #[serde(default)]
    pub skip_project: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub summary: ImportSummary,
    pub cli_output: String,
}

// ---------------------------------------------------------------------------
// Workspace commands
// ---------------------------------------------------------------------------

fn build_workspace(state: &AppState) -> Workspace {
    let config_result = load_json(&state.paths.config()).and_then(|v| parse_config(&v));
    let (config, config_error) = match config_result {
        Ok(c) => (Some(c), None),
        Err(e) => (None, Some(e)),
    };

    let (tasks_source, tasks_error, task_count, validation, preview, settings) = {
        let guard = state.tasks.lock().unwrap();
        match guard.as_ref() {
            Some(loaded) => (
                Some(loaded.source.clone()),
                None,
                loaded.raw.len(),
                Some(loaded.report.clone()),
                loaded
                    .parsed
                    .iter()
                    .map(|t| PreviewRow {
                        id: t.id.clone(),
                        priority: t.priority.clone(),
                        module: t.module.clone(),
                        title: t.title.clone(),
                        labels: t.labels.clone(),
                        depends_on: t.depends_on.clone(),
                        parent: t.parent.clone(),
                    })
                    .collect(),
                loaded.settings.clone(),
            ),
            None => (
                None,
                None,
                0,
                None,
                Vec::new(),
                Settings::default(),
            ),
        }
    };

    Workspace {
        config,
        config_error,
        tasks_source,
        tasks_error,
        task_count,
        validation,
        preview,
        settings_banner: tasks::settings_banner(&settings),
        settings,
    }
}

/// Parse and validate a tasks file body, storing it as the active backlog.
fn load_tasks_into(state: &AppState, source: String, text: &str) -> Result<(), String> {
    let raw = tasks::parse_raw(text).map_err(|e| format!("Cannot read {source}: {e}"))?;
    let report = tasks::validate(&raw);
    let parsed: Vec<Task> = if report.ok {
        raw.iter().filter_map(tasks::to_task).collect()
    } else {
        Vec::new()
    };

    let dotenv = std::fs::read_to_string(state.paths.dotenv())
        .map(|t| tasks::parse_dotenv(&t))
        .unwrap_or_default();
    let settings = tasks::load_settings(&dotenv, |k| std::env::var(k).ok());

    *state.tasks.lock().unwrap() = Some(LoadedTasks {
        source,
        raw,
        parsed,
        report,
        settings,
    });
    Ok(())
}

/// App and workspace information, resolved once per window load.
#[tauri::command]
pub async fn app_info(state: State<'_, AppState>) -> Result<AppInfo, String> {
    let gh_cli_available = auth::gh_cli_available().await;
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        portable: state.paths.portable,
        base_dir: state.paths.base_dir.display().to_string(),
        config_path: state.paths.config().display().to_string(),
        tasks_path: state.paths.tasks().display().to_string(),
        state_path: state.paths.state().display().to_string(),
        state_exists: state.paths.state().is_file(),
        builtin_client_id: auth::BUILTIN_CLIENT_ID.map(|s| s.to_string()),
        gh_cli_available,
    })
}

/// Read `config.json` and `tasks.json` and return everything the UI shows.
#[tauri::command]
pub async fn load_workspace(state: State<'_, AppState>) -> Result<Workspace, String> {
    let tasks_path = state.paths.tasks();
    if tasks_path.is_file() {
        let text = std::fs::read_to_string(&tasks_path)
            .map_err(|e| format!("Cannot read {}: {e}", tasks_path.display()))?;
        // A malformed file is reported through the workspace, not as a hard
        // error, so the UI can still show the config form.
        if let Err(e) = load_tasks_into(&state, tasks_path.display().to_string(), &text) {
            let mut ws = build_workspace(&state);
            ws.tasks_error = Some(e);
            return Ok(ws);
        }
    }
    Ok(build_workspace(&state))
}

/// Load a backlog from an arbitrary path, as chosen in a file dialog.
#[tauri::command]
pub async fn load_tasks_from_path(
    path: String,
    state: State<'_, AppState>,
) -> Result<Workspace, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    load_tasks_into(&state, path, &text)?;
    Ok(build_workspace(&state))
}

/// Validate the in-memory backlog and refresh the preview.
#[tauri::command]
pub async fn revalidate(state: State<'_, AppState>) -> Result<Workspace, String> {
    Ok(build_workspace(&state))
}

/// Write `config.json` into the workspace.
#[tauri::command]
pub async fn save_config(
    config: Config,
    state: State<'_, AppState>,
) -> Result<Workspace, String> {
    let path = state.paths.config();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    Ok(build_workspace(&state))
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

fn status_from(state: &AppState, vault_error: Option<String>) -> AuthStatus {
    let prefs = state.prefs.lock().unwrap().clone();
    let guard = state.session.lock().unwrap();
    match guard.as_ref() {
        Some(s) => AuthStatus {
            signed_in: true,
            login: Some(s.account.login.clone()),
            name: s.account.name.clone(),
            method: Some(s.method.label().to_string()),
            scopes: s.account.scopes.clone(),
            missing_scopes: s.account.missing_scopes(),
            client_id: prefs.client_id,
            remember_token: prefs.remember_token,
            vault_error,
        },
        None => AuthStatus {
            signed_in: false,
            login: None,
            name: None,
            method: None,
            scopes: Vec::new(),
            missing_scopes: Vec::new(),
            client_id: prefs.client_id,
            remember_token: prefs.remember_token,
            vault_error,
        },
    }
}

/// Current sign-in state, restoring a remembered token from the vault.
#[tauri::command]
pub async fn auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let already = state.session.lock().unwrap().is_some();
    if already {
        return Ok(status_from(&state, None));
    }

    let prefs = state.prefs.lock().unwrap().clone();
    if !prefs.remember_token {
        return Ok(status_from(&state, None));
    }

    match state.vault.load() {
        Ok(Some(token)) => match auth::verify_token(&token, None).await {
            Ok(account) => {
                *state.session.lock().unwrap() = Some(Session {
                    token: SecretToken::new(token),
                    account,
                    // The vault does not record which flow produced the token;
                    // the label is cosmetic and corrected on a fresh sign-in.
                    method: AuthMethod::PersonalAccessToken,
                });
                Ok(status_from(&state, None))
            }
            Err(e) => {
                // A stored token that no longer works should not block the UI.
                let _ = state.vault.clear();
                Ok(status_from(
                    &state,
                    Some(format!("Stored token is no longer valid: {e}")),
                ))
            }
        },
        Ok(None) => Ok(status_from(&state, None)),
        Err(e) => Ok(status_from(&state, Some(e))),
    }
}

/// Save the OAuth App client id.
#[tauri::command]
pub async fn set_client_id(
    client_id: String,
    state: State<'_, AppState>,
) -> Result<AuthStatus, String> {
    {
        let mut prefs = state.prefs.lock().unwrap();
        prefs.client_id = client_id.trim().to_string();
        prefs.save(&state.paths)?;
    }
    Ok(status_from(&state, None))
}

/// Step 1 of the device flow: get a user code to display.
#[tauri::command]
pub async fn auth_start_device_flow(
    state: State<'_, AppState>,
) -> Result<DeviceCodeInfo, String> {
    let client_id = state.prefs.lock().unwrap().client_id.clone();
    if client_id.trim().is_empty() {
        return Err(
            "No OAuth App client id is configured. Register an OAuth App with device flow \
             enabled (see the README), then paste its client id here — or use a personal \
             access token instead."
                .to_string(),
        );
    }
    let endpoints = auth::OAuthEndpoints::github();
    let flow = auth::start_device_flow(&endpoints, &client_id, auth::REQUIRED_SCOPES)
        .await
        .map_err(|e| e.to_string())?;
    let info = flow.info.clone();
    *state.pending_flow.lock().unwrap() = Some(flow);
    Ok(info)
}

/// Step 2: poll until the user approves, the code expires, or they deny it.
///
/// Progress is streamed on [`AUTH_EVENT`] so the UI can show a countdown.
#[tauri::command]
pub async fn auth_poll_device_flow(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AuthStatus, String> {
    let endpoints = auth::OAuthEndpoints::github();

    loop {
        let flow = {
            let guard = state.pending_flow.lock().unwrap();
            match guard.as_ref() {
                Some(f) => f.clone(),
                None => return Err("No device flow is in progress.".to_string()),
            }
        };

        let now = std::time::Instant::now();
        if flow.is_expired(now) {
            state.pending_flow.lock().unwrap().take();
            let _ = app.emit(AUTH_EVENT, serde_json::json!({ "status": "expired" }));
            return Err(auth::AuthError::Expired.to_string());
        }

        let _ = app.emit(
            AUTH_EVENT,
            serde_json::json!({
                "status": "waiting",
                "secondsRemaining": flow.seconds_remaining(now),
            }),
        );

        match auth::poll_device_flow(&endpoints, &flow, now).await {
            auth::PollOutcome::Token(token) => {
                state.pending_flow.lock().unwrap().take();
                let account = auth::verify_token(&token, None).await.map_err(|e| e.to_string())?;
                store_token(&state, &token)?;
                *state.session.lock().unwrap() = Some(Session {
                    token: SecretToken::new(token),
                    account,
                    method: AuthMethod::DeviceFlow,
                });
                let _ = app.emit(AUTH_EVENT, serde_json::json!({ "status": "approved" }));
                return Ok(status_from(&state, None));
            }
            auth::PollOutcome::Pending => {
                tokio::time::sleep(std::time::Duration::from_secs(flow.interval)).await;
            }
            auth::PollOutcome::SlowDown => {
                // GitHub's documented remedy: add five seconds to the interval.
                let next = flow.interval + 5;
                if let Some(f) = state.pending_flow.lock().unwrap().as_mut() {
                    f.interval = next;
                }
            }
            auth::PollOutcome::Failed(e) => {
                state.pending_flow.lock().unwrap().take();
                let _ = app.emit(
                    AUTH_EVENT,
                    serde_json::json!({ "status": "failed", "message": e.to_string() }),
                );
                return Err(e.to_string());
            }
        }
    }
}

/// Cancel an in-progress device flow.
#[tauri::command]
pub async fn auth_cancel_device_flow(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(mut flow) = state.pending_flow.lock().unwrap().take() {
        flow.clear();
    }
    Ok(())
}

/// Persist a token, honouring the "remember me" preference.
fn store_token(state: &AppState, token: &str) -> Result<(), String> {
    let remember = state.prefs.lock().unwrap().remember_token;
    if remember {
        state.vault.save(token)?;
    } else {
        // Memory-only: make sure nothing stale is left behind.
        let _ = state.vault.clear();
    }
    Ok(())
}

/// Sign in with a pasted personal access token.
#[tauri::command]
pub async fn auth_set_pat(
    token: String,
    state: State<'_, AppState>,
) -> Result<AuthStatus, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("Enter a token first.".to_string());
    }
    let account = auth::verify_token(&token, None)
        .await
        .map_err(|e| format!("That token was rejected: {e}"))?;
    store_token(&state, &token)?;
    *state.session.lock().unwrap() = Some(Session {
        token: SecretToken::new(token),
        account,
        method: AuthMethod::PersonalAccessToken,
    });
    Ok(status_from(&state, None))
}

/// Sign in using an existing `gh` CLI session, for migration from the CLI.
#[tauri::command]
pub async fn auth_import_gh(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let token = auth::token_from_gh_cli().await.map_err(|e| e.to_string())?;
    let account = auth::verify_token(&token, None)
        .await
        .map_err(|e| format!("The gh CLI token was rejected: {e}"))?;
    store_token(&state, &token)?;
    *state.session.lock().unwrap() = Some(Session {
        token: SecretToken::new(token),
        account,
        method: AuthMethod::GhCli,
    });
    Ok(status_from(&state, None))
}

/// Set whether the token survives a restart.
#[tauri::command]
pub async fn auth_set_remember(
    remember: bool,
    state: State<'_, AppState>,
) -> Result<AuthStatus, String> {
    {
        let mut prefs = state.prefs.lock().unwrap();
        prefs.remember_token = remember;
        prefs.save(&state.paths)?;
    }
    // Turning it off must also drop anything already stored.
    if !remember {
        let _ = state.vault.clear();
    } else if let Some(token) = state.token() {
        state.vault.save(&token)?;
    }
    Ok(status_from(&state, None))
}

/// Forget the token and clear the vault.
#[tauri::command]
pub async fn auth_sign_out(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    if let Some(mut flow) = state.pending_flow.lock().unwrap().take() {
        flow.clear();
    }
    *state.session.lock().unwrap() = None;
    state.vault.clear()?;
    Ok(status_from(&state, None))
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Resolve the project board, if the config names one.
async fn resolve_project(
    client: &GithubClient,
    config: &Config,
) -> Result<Option<Project>, String> {
    if config.project.trim().is_empty() || config.project_owner.trim().is_empty() {
        return Ok(None);
    }
    client
        .find_project(&config.project_owner, &config.project)
        .await
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Start an import. Returns immediately; progress arrives on [`PROGRESS_EVENT`].
#[tauri::command]
pub async fn import_start(
    app: AppHandle,
    request: ImportRequest,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // One run at a time: the state file is a single shared resource.
    if state.running.swap(true, Ordering::SeqCst) {
        return Err("An import is already running.".to_string());
    }
    state.cancel.store(false, Ordering::SeqCst);

    let result = run_import(&app, &state, request).await;

    state.running.store(false, Ordering::SeqCst);
    if let Err(e) = &result {
        let _ = app.emit(
            PROGRESS_EVENT,
            &ImportEvent::Failed {
                message: e.clone(),
            },
        );
    }
    result
}

async fn run_import(
    app: &AppHandle,
    state: &AppState,
    request: ImportRequest,
) -> Result<(), String> {
    let config = state.config()?;
    let client = state.client()?;
    let repo = RepoRef::parse(&config.repo).map_err(|e| e.to_string())?;

    let loaded = {
        let guard = state.tasks.lock().unwrap();
        guard.clone().ok_or("Load a tasks.json file first.")?
    };
    if !loaded.report.ok {
        return Err(format!(
            "The backlog did not validate:\n{}",
            loaded.report.to_cli_output()
        ));
    }

    let project = if request.skip_project {
        None
    } else {
        resolve_project(&client, &config).await?
    };

    let mut import_state = ImportState::load(&state.paths.state());
    let mut importer = Importer {
        client: &client,
        repo,
        project,
        tasks: loaded.parsed.clone(),
        settings: loaded.settings.clone(),
        state: std::mem::take(&mut import_state),
        state_path: state.paths.state(),
        options: ImportOptions {
            dry_run: request.dry_run,
            skip_relationships: request.skip_relationships,
            skip_project: request.skip_project,
        },
        sink: Arc::new(TauriSink { app: app.clone() }),
        cancel: state.cancel.clone(),
    };

    importer
        .run()
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Ask a running import to stop after the current step.
#[tauri::command]
pub async fn import_cancel(state: State<'_, AppState>) -> Result<(), String> {
    state.cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Run a dry run and return the CLI-equivalent output, for the parity oracle
/// and the UI's "compare with the Python CLI" affordance.
///
/// Not currently wired to a button; exposed so the parity script can drive the
/// same code path the app uses rather than a reimplementation.
#[tauri::command]
pub async fn import_dry_run_preview(state: State<'_, AppState>) -> Result<RunReport, String> {
    let loaded = {
        let guard = state.tasks.lock().unwrap();
        guard.clone().ok_or("Load a tasks.json file first.")?
    };
    let mut out = ImportSummary {
        dry_run: true,
        ..Default::default()
    };
    out.created = loaded
        .parsed
        .iter()
        .filter(|t| !t.id.is_empty())
        .count();
    let summary = out.clone();
    Ok(RunReport {
        cli_output: summary.to_cli_output(loaded.settings.skip_relationships),
        summary,
    })
}

/// The status of every task after a run, for the results table.
#[tauri::command]
pub async fn import_statuses(state: State<'_, AppState>) -> Result<Vec<Value>, String> {
    let import_state = ImportState::load(&state.paths.state());
    Ok(import_state
        .issues
        .iter()
        .map(|(id, number)| {
            serde_json::json!({
                "id": id,
                "number": number,
                "onProject": import_state.project_items.contains_key(id),
            })
        })
        .collect())
}

/// Expose the task status enum to `import_statuses` callers without duplicating
/// the list in TypeScript.
#[tauri::command]
pub fn task_statuses() -> Vec<&'static str> {
    vec![
        "created", "skipped", "added", "already_set", "failed", "would_create", "would_add",
    ]
}

/// Open the app-managed directory in the OS file manager.
#[tauri::command]
pub async fn reveal_workspace(state: State<'_, AppState>) -> Result<(), String> {
    tauri_plugin_opener::reveal_item_in_dir(state.paths.base_dir.clone())
        .map_err(|e| e.to_string())
}

/// Report the resolved paths, for the diagnostics panel.
#[tauri::command]
pub async fn workspace_paths(state: State<'_, AppState>) -> Result<Value, String> {
    Ok(serde_json::json!({
        "portable": state.paths.portable,
        "baseDir": state.paths.base_dir.display().to_string(),
        "config": state.paths.config().display().to_string(),
        "tasks": state.paths.tasks().display().to_string(),
        "state": state.paths.state().display().to_string(),
        "dotenv": state.paths.dotenv().display().to_string(),
    }))
}

/// Build the shared state. Kept here so `lib.rs` stays declarative.
pub fn build_state(paths: AppPaths, vault: Box<dyn SecretStore>) -> AppState {
    let prefs = Prefs::load(&paths);
    AppState {
        paths,
        session: Mutex::new(None),
        pending_flow: Mutex::new(None),
        prefs: Mutex::new(prefs),
        vault,
        running: Arc::new(AtomicBool::new(false)),
        cancel: Arc::new(AtomicBool::new(false)),
        tasks: Mutex::new(None),
    }
}

/// The vault implied by the saved preferences.
///
/// "Don't remember me" resolves to a memory-only store, so the token is never
/// written anywhere.
pub fn vault_for(paths: &AppPaths) -> Box<dyn SecretStore> {
    let prefs = Prefs::load(paths);
    crate::secrets::store_for(&paths.base_dir, prefs.remember_token, prefs.vault_passphrase.clone())
}
