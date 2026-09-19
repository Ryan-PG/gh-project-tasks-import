//! GitHub Importer — a desktop front end for the GitHub Project Tasks
//! Importer.
//!
//! The app replaces the Python CLI's two runtime dependencies (Python and the
//! `gh` CLI) with a single binary. All GitHub traffic happens here in Rust:
//! the webview talks to this process over IPC and never sees a token, which is
//! why the window's Content-Security-Policy needs no remote origins.
//!
//! Module map:
//!
//! * [`tasks`] — the task model and a byte-faithful port of the CLI's
//!   `validate()`; also the frozen idempotency markers.
//! * [`github`] — REST client, `Link` pagination, retry/backoff.
//! * [`graphql`] — Projects V2, which has no REST equivalent.
//! * [`auth`] — device flow, personal access tokens, `gh` CLI import.
//! * [`secrets`] — encrypted-at-rest token storage.
//! * [`state`] — portable-mode resolution and the resume state file.
//! * [`importer`] — orchestration, progress events, resumability.
//! * [`commands`] — the IPC surface.

pub mod auth;
pub mod commands;
pub mod github;
pub mod graphql;
pub mod importer;
pub mod secrets;
pub mod state;
pub mod tasks;

use state::AppPaths;
use tauri::Manager;

/// Resolve where the app reads and writes its files.
///
/// Portable resolution is a three-step fallback; see [`AppPaths::resolve`].
fn resolve_paths(app: &tauri::AppHandle) -> Result<AppPaths, Box<dyn std::error::Error>> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let app_config_dir = app.path().app_config_dir()?;

    Ok(AppPaths::resolve(
        &exe_dir,
        &cwd,
        &app_config_dir,
        cfg!(debug_assertions),
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let paths = resolve_paths(app.handle())?;
            // Make sure the workspace exists before the first command runs, so
            // a fresh install can save config without a missing-directory error.
            std::fs::create_dir_all(&paths.base_dir).ok();
            let vault = commands::vault_for(&paths);
            app.manage(commands::build_state(paths, vault));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::load_workspace,
            commands::load_tasks_from_path,
            commands::revalidate,
            commands::save_config,
            commands::workspace_paths,
            commands::reveal_workspace,
            commands::task_statuses,
            commands::auth_status,
            commands::set_client_id,
            commands::auth_start_device_flow,
            commands::auth_poll_device_flow,
            commands::auth_cancel_device_flow,
            commands::auth_set_pat,
            commands::auth_import_gh,
            commands::auth_set_remember,
            commands::auth_sign_out,
            commands::import_start,
            commands::import_cancel,
            commands::import_dry_run_preview,
            commands::import_statuses,
        ])
        .run(tauri::generate_context!())
        .expect("error while running GitHub Importer");
}
