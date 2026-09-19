//! Portable-aware workspace resolution and the resumable import state store.
//!
//! `.import-state.json` is only a cache: the importer re-derives what already
//! exists from the GitHub API, so a missing or stale state file costs time but
//! never correctness. That is what makes the per-step resume strategy safe.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Marker file that switches the app into portable mode.
///
/// When this sits beside the executable, config, tasks, and state are read and
/// written next to the binary rather than in the OS app-config directory, so
/// the whole thing travels on a USB stick.
pub const PORTABLE_MARKER: &str = "portable.txt";
/// Accepted alternative spelling, for people who reach for a dotfile.
pub const PORTABLE_MARKER_ALT: &str = ".portable";

/// `config.json`, as the Python tool defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub repo: String,
    pub project: String,
    pub project_owner: String,
}

/// A resolved set of file locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// True when a portable marker was found beside the executable.
    pub portable: bool,
    /// Directory holding `config.json`, `tasks.json`, and the state file.
    pub base_dir: PathBuf,
}

impl AppPaths {
    /// Resolve the workspace.
    ///
    /// Order, per the plan:
    /// 1. a portable marker beside the executable → that directory;
    /// 2. a debug build → the nearest ancestor of `cwd` that looks like the
    ///    repo, so `tauri dev` picks up the existing `config.json`;
    /// 3. otherwise the OS app-config directory.
    pub fn resolve(exe_dir: &Path, cwd: &Path, app_config_dir: &Path, is_debug: bool) -> Self {
        if has_portable_marker(exe_dir) {
            return Self {
                portable: true,
                base_dir: exe_dir.to_path_buf(),
            };
        }
        if is_debug {
            if let Some(dir) = find_repo_root(cwd) {
                return Self {
                    portable: false,
                    base_dir: dir,
                };
            }
        }
        Self {
            portable: false,
            base_dir: app_config_dir.to_path_buf(),
        }
    }

    pub fn config(&self) -> PathBuf {
        self.base_dir.join("config.json")
    }

    pub fn tasks(&self) -> PathBuf {
        self.base_dir.join("tasks.json")
    }

    pub fn state(&self) -> PathBuf {
        self.base_dir.join(".import-state.json")
    }

    pub fn dotenv(&self) -> PathBuf {
        self.base_dir.join(".env")
    }

    /// Where a portable release expects to find `portable.txt`.
    pub fn portable_marker_path(&self) -> PathBuf {
        self.base_dir.join(PORTABLE_MARKER)
    }
}

/// Is either portable marker present in `dir`?
pub fn has_portable_marker(dir: &Path) -> bool {
    dir.join(PORTABLE_MARKER).is_file() || dir.join(PORTABLE_MARKER_ALT).is_file()
}

/// Walk up from `start` looking for a directory that holds the project's
/// `config.json` or `tasks.json`. Used only for debug builds.
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join("config.json").is_file() || d.join("tasks.json").is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Persisted import progress.
///
/// Every field is a resume optimisation. The `issues` map keeps the exact shape
/// the Python tool writes, so external tooling that reads it keeps working; the
/// rest is additive.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportState {
    /// Task id → issue number. Same shape as the CLI's `.import-state.json`.
    #[serde(default)]
    pub issues: BTreeMap<String, u64>,
    /// Task id → issue **database** id. Relationship endpoints need this, and
    /// resolving it costs an extra `GET /issues/{n}` per task if it is missing.
    #[serde(default)]
    pub issue_ids: BTreeMap<String, u64>,
    /// Task id → issue GraphQL node id, for `addProjectV2ItemById`.
    #[serde(default)]
    pub node_ids: BTreeMap<String, String>,
    /// Task id → the project item id returned by `addProjectV2ItemById`.
    ///
    /// Its presence is what lets a resumed run skip the add-to-project step,
    /// which is the window where a partial failure is possible.
    #[serde(default)]
    pub project_items: BTreeMap<String, String>,
}

impl ImportState {
    pub fn from_json(v: &Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .map(|v: Value| Self::from_json(&v))
            .unwrap_or_default()
    }

    /// Write the state atomically.
    ///
    /// This is called after *each step*, not after each task, so a crash
    /// between creating an issue and adding it to the board leaves enough
    /// information to resume. A torn write would be worse than no write at all,
    /// hence the temp-file-and-rename.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut root = Map::new();
        root.insert("version".into(), json!(2));
        root.insert("issues".into(), json!(self.issues));
        root.insert("issue_ids".into(), json!(self.issue_ids));
        root.insert("node_ids".into(), json!(self.node_ids));
        root.insert("project_items".into(), json!(self.project_items));

        let text = serde_json::to_string_pretty(&Value::Object(root))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Merge an id→number map, as returned by the `existing()` port.
    pub fn seed_issues(&mut self, found: &BTreeMap<String, u64>) {
        for (id, number) in found {
            self.issues.insert(id.clone(), *number);
        }
    }
}

/// Read a JSON file, with an error message shaped like the Python `load()`.
pub fn load_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read {}: {}", path.display(), e))?;
    serde_json::from_str(&text).map_err(|e| format!("Cannot read {}: {}", path.display(), e))
}

/// Parse a `config.json` payload.
pub fn parse_config(v: &Value) -> Result<Config, String> {
    serde_json::from_value(v.clone()).map_err(|e| format!("config.json is not valid: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_marker_wins_over_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("exe");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join(PORTABLE_MARKER), "").unwrap();

        let paths = AppPaths::resolve(&exe, tmp.path(), &tmp.path().join("cfg"), true);
        assert!(paths.portable);
        assert_eq!(paths.base_dir, exe);
        // With a marker present the state file sits beside the binary, so it
        // travels with the stick.
        assert_eq!(paths.state(), exe.join(".import-state.json"));
    }

    #[test]
    fn alt_marker_is_also_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("exe");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join(PORTABLE_MARKER_ALT), "").unwrap();
        assert!(has_portable_marker(&exe));
    }

    #[test]
    fn debug_build_uses_the_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let nested = repo.join("desktop/src-tauri");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(repo.join("config.json"), "{}").unwrap();

        let exe = tmp.path().join("exe");
        std::fs::create_dir_all(&exe).unwrap();

        let paths = AppPaths::resolve(&exe, &nested, &tmp.path().join("cfg"), true);
        assert!(!paths.portable);
        assert_eq!(paths.base_dir, repo);
    }

    #[test]
    fn release_build_uses_the_app_config_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("config.json"), "{}").unwrap();
        let exe = tmp.path().join("exe");
        std::fs::create_dir_all(&exe).unwrap();
        let cfg = tmp.path().join("cfg");

        // is_debug = false: the repo root must be ignored in a real install.
        let paths = AppPaths::resolve(&exe, &repo, &cfg, false);
        assert!(!paths.portable);
        assert_eq!(paths.base_dir, cfg);
    }

    #[test]
    fn state_round_trips_and_keeps_the_cli_key() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".import-state.json");

        let mut state = ImportState::default();
        state.issues.insert("A-001".into(), 12);
        state.issue_ids.insert("A-001".into(), 1200);
        state.node_ids.insert("A-001".into(), "I_abc".into());
        state.project_items.insert("A-001".into(), "PVTI_1".into());
        state.save(&path).unwrap();

        // The on-disk shape must still expose `issues` exactly as the CLI
        // wrote it, so external tooling keeps working.
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["issues"]["A-001"], json!(12));

        let back = ImportState::load(&path);
        assert_eq!(back.issues.get("A-001"), Some(&12));
        assert_eq!(back.issue_ids.get("A-001"), Some(&1200));
        assert_eq!(back.node_ids.get("A-001").map(String::as_str), Some("I_abc"));
        assert_eq!(
            back.project_items.get("A-001").map(String::as_str),
            Some("PVTI_1")
        );
    }

    #[test]
    fn state_load_of_a_cli_written_file_works() {
        // The CLI writes only {"issues": {...}}; that must load cleanly.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".import-state.json");
        std::fs::write(&path, r#"{"issues": {"DOC-FE-001": 92}}"#).unwrap();
        let state = ImportState::load(&path);
        assert_eq!(state.issues.get("DOC-FE-001"), Some(&92));
        assert!(state.project_items.is_empty());
    }

    #[test]
    fn missing_state_file_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let state = ImportState::load(&tmp.path().join("nope.json"));
        assert!(state.issues.is_empty());
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".import-state.json");
        ImportState::default().save(&path).unwrap();
        assert!(path.is_file());
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn config_parses_the_documented_shape() {
        let v = json!({
            "repo": "PersianRepo/front",
            "project": "Front",
            "project_owner": "PersianRepo"
        });
        let c = parse_config(&v).unwrap();
        assert_eq!(c.repo, "PersianRepo/front");
        assert_eq!(c.project, "Front");
        assert_eq!(c.project_owner, "PersianRepo");
    }

    #[test]
    fn config_rejects_a_missing_key() {
        let v = json!({ "repo": "a/b", "project": "P" });
        assert!(parse_config(&v).is_err());
    }
}
