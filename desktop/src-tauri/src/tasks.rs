//! Task model and the port of `import_tasks.py`'s `validate()`.
//!
//! This module is deliberately network-free. It is the parity oracle's subject:
//! [`ValidationReport::to_cli_output`] must reproduce the Python CLI's stdout
//! byte-for-byte, and [`issue_title`] / [`issue_body`] must reproduce the exact
//! bytes the CLI wrote to GitHub, or a rerun from the desktop app would
//! duplicate every issue the CLI already created.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::LazyLock;

/// Fields `validate()` requires on every task, in the order it checks them.
const REQUIRED_FIELDS: [&str; 8] = [
    "id",
    "module",
    "title",
    "priority",
    "labels",
    "body",
    "depends_on",
    "parent",
];

const VALID_PRIORITIES: [&str; 4] = ["P0", "P1", "P2", "P3"];

/// Idempotency marker. `import_tasks.py` matches issue titles with this.
///
/// Freezing this regex is the single highest-risk correctness item in the port:
/// if it drifts, the desktop app stops recognising issues the CLI created and
/// duplicates all of them.
pub static MANAGED_TITLE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\[([A-Z0-9]+(?:-[A-Z0-9]+)*-\d{3})\]\s").expect("static regex is valid")
});

/// A task as it appears in `tasks.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub module: String,
    pub title: String,
    pub priority: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub parent: Option<String>,
}

/// Result of running [`validate`]. Mirrors the Python function's two outcomes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub ok: bool,
    pub errors: Vec<String>,
    pub task_count: usize,
}

impl ValidationReport {
    /// Reproduce the Python CLI's stdout exactly, so the parity oracle can diff
    /// the two implementations with `diff` and no normalisation.
    pub fn to_cli_output(&self) -> String {
        if self.errors.is_empty() {
            format!("OK: {} tasks validated.\n", self.task_count)
        } else {
            let mut out = String::from("Validation failed:\n");
            for e in &self.errors {
                out.push_str("- ");
                out.push_str(e);
                out.push('\n');
            }
            out
        }
    }
}

/// Render a JSON value the way Python's `str()` would, so interpolated error
/// messages match. Only the common cases need to be exact — task ids are
/// strings in every real `tasks.json` — but the fallbacks are total.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => {
            let inner: Vec<String> = a.iter().map(py_str).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(o) => {
            let inner: Vec<String> = o
                .iter()
                .map(|(k, v)| format!("'{}': {}", k, py_str(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// Python truthiness, for the `if t["parent"] and ...` guard in pass 2.
///
/// A JSON `0`, `false`, `""`, `[]`, or `{}` is falsy and short-circuits the
/// check, so `"parent": 0` must not be reported as an unknown parent.
fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Port of `validate()` from `import_tasks.py`.
///
/// The error list is order-sensitive: the Python function emits all pass-1
/// findings (in task order) before any pass-2 finding, and the parity oracle
/// compares the rendered output verbatim.
///
/// One deliberate divergence: pass 2 of the Python function indexes
/// `t["id"]` / `t["depends_on"]` / `t["parent"]` directly, so a task missing
/// one of those raises `KeyError` and crashes the CLI. Here those tasks are
/// skipped instead. Parity holds for every input the Python tool survives.
pub fn validate(tasks: &[Value]) -> ValidationReport {
    let mut errors: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Pass 1 — per-task structural checks.
    for t in tasks {
        let obj = t.as_object();
        // Python: t.get('id', '?') — '?' only when the key is absent.
        let id_label = match obj.and_then(|o| o.get("id")) {
            Some(v) => py_str(v),
            None => "?".to_string(),
        };

        for k in REQUIRED_FIELDS {
            if !obj.map(|o| o.contains_key(k)).unwrap_or(false) {
                errors.push(format!("{}: missing {}", id_label, k));
            }
        }

        // Python: tid = t.get('id') → None when absent, rendered as "None".
        let tid = obj
            .and_then(|o| o.get("id"))
            .map(py_str)
            .unwrap_or_else(|| "None".to_string());

        if seen.contains(&tid) {
            errors.push(format!("{}: duplicate ID", tid));
        }
        seen.insert(tid.clone());
        ids.push(tid.clone());

        let priority = obj.and_then(|o| o.get("priority"));
        let priority_ok = priority
            .and_then(|p| p.as_str())
            .map(|p| VALID_PRIORITIES.contains(&p))
            .unwrap_or(false);
        if !priority_ok {
            errors.push(format!("{}: invalid priority", tid));
        }
    }

    // Pass 2 — relationship checks, against the full id set.
    let id_set: HashSet<&String> = ids.iter().collect();
    for t in tasks {
        let Some(obj) = t.as_object() else { continue };
        let Some(raw_id) = obj.get("id") else { continue };
        let tid = py_str(raw_id);

        // Python iterates whatever `depends_on` holds. An array is the normal
        // case; a bare string iterates character by character, which is a
        // plausible typo and produces one error per character there, so the
        // port reproduces it rather than silently accepting the value.
        let deps: Vec<String> = match obj.get("depends_on") {
            Some(Value::Array(a)) => a.iter().map(py_str).collect(),
            Some(Value::String(s)) => s.chars().map(|c| c.to_string()).collect(),
            _ => Vec::new(),
        };
        for dep in deps {
            if !id_set.contains(&dep) {
                errors.push(format!("{}: unknown dependency {}", tid, dep));
            }
            if dep == tid {
                errors.push(format!("{}: self dependency", tid));
            }
        }

        if let Some(parent) = obj.get("parent") {
            // Python: `if t["parent"] and t["parent"] not in ids` — the
            // truthiness test runs first, so a falsy parent is skipped.
            if py_truthy(parent) {
                let p = py_str(parent);
                if !id_set.contains(&p) {
                    errors.push(format!("{}: unknown parent {}", tid, p));
                }
            }
        }
    }

    ValidationReport {
        ok: errors.is_empty(),
        task_count: tasks.len(),
        errors,
    }
}

/// Parse `tasks.json` into raw JSON values, ready for [`validate`].
///
/// Returns `Err` with a message shaped like the Python `load()` helper's.
pub fn parse_raw(text: &str) -> Result<Vec<Value>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    match v {
        Value::Array(a) => Ok(a),
        _ => Err("expected a JSON array of tasks".to_string()),
    }
}

/// Convert an already-validated raw task into a [`Task`].
///
/// Only call this once [`validate`] reports `ok` — it coerces missing or
/// wrongly-typed fields rather than reporting them.
pub fn to_task(v: &Value) -> Option<Task> {
    let o = v.as_object()?;
    let strings = |key: &str| -> Vec<String> {
        o.get(key)
            .and_then(|x| x.as_array())
            .map(|a| a.iter().map(py_str).collect())
            .unwrap_or_default()
    };
    Some(Task {
        id: o.get("id").map(py_str).unwrap_or_default(),
        module: o.get("module").map(py_str).unwrap_or_default(),
        title: o.get("title").map(py_str).unwrap_or_default(),
        priority: o.get("priority").map(py_str).unwrap_or_default(),
        labels: strings("labels"),
        body: o.get("body").map(py_str).unwrap_or_default(),
        depends_on: strings("depends_on"),
        parent: o
            .get("parent")
            .filter(|p| !p.is_null())
            .map(py_str)
            .filter(|p| !p.is_empty()),
    })
}

/// Extract the managed task id from an issue title, if it carries one.
pub fn task_id_from_title(title: &str) -> Option<String> {
    MANAGED_TITLE
        .captures(title)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Port of `existing()`: map task id → issue number for every managed issue.
///
/// The caller must exclude pull requests — GitHub's `/issues` REST endpoint
/// returns them, whereas `gh issue list` does not.
pub fn existing_from_issues<'a, I>(issues: I) -> std::collections::BTreeMap<String, u64>
where
    I: IntoIterator<Item = (u64, &'a str)>,
{
    let mut found = std::collections::BTreeMap::new();
    for (number, title) in issues {
        if let Some(id) = task_id_from_title(title) {
            found.insert(id, number);
        }
    }
    found
}

/// The issue title the CLI writes: `[{id}] {title}`.
pub fn issue_title(t: &Task) -> String {
    format!("[{}] {}", t.id, t.title)
}

/// The issue body the CLI writes, byte-for-byte.
pub fn issue_body(t: &Task) -> String {
    format!(
        "<!-- github-task:{} -->\n\n**Module:** `{}`  \n**Priority:** `{}`\n\n{}\n\n## Acceptance Criteria\n\n- [ ] Implementation completed\n- [ ] Appropriate tests added\n- [ ] API/documentation updated where applicable\n",
        t.id, t.module, t.priority, t.body
    )
}

/// The body marker alone, used to recognise managed issues by body.
pub fn body_marker(id: &str) -> String {
    format!("<!-- github-task:{} -->", id)
}

/// Port of `create_issue`'s label list: `backend`, module, priority, then the
/// task's own labels, de-duplicated with order preserved.
pub fn issue_labels(t: &Task) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(t.labels.len() + 3);
    for l in ["backend", t.module.as_str(), t.priority.as_str()] {
        if !l.is_empty() && !out.iter().any(|x| x == l) {
            out.push(l.to_string());
        }
    }
    for l in &t.labels {
        if !out.iter().any(|x| x == l) {
            out.push(l.clone());
        }
    }
    out
}

/// Port of `ensure_labels`'s needed set: always `backend` plus every priority,
/// plus every module and every task label.
pub fn needed_labels(tasks: &[Task]) -> Vec<String> {
    let mut set: Vec<String> = VALID_PRIORITIES
        .iter()
        .map(|s| s.to_string())
        .chain(std::iter::once("backend".to_string()))
        .collect();
    let mut push = |l: &str| {
        if !l.is_empty() && !set.iter().any(|x| x == l) {
            set.push(l.to_string());
        }
    };
    for t in tasks {
        push(&t.module);
        for l in &t.labels {
            push(l);
        }
    }
    set.sort();
    set
}

/// Port of `preview`'s row format: `{id:<15} {priority} {module:<18} {title}`.
pub fn preview_line(t: &Task) -> String {
    format!(
        "{:<15} {} {:<18} {}",
        t.id, t.priority, t.module, t.title
    )
}

/// Port of `project_check`'s failure message.
pub fn project_not_found_message(owner: &str, name: &str, available: &[String]) -> String {
    let mut titles: Vec<&str> = available.iter().map(|s| s.as_str()).collect();
    titles.sort_unstable();
    let rendered: Vec<String> = titles.iter().map(|t| format!("'{}'", t)).collect();
    format!(
        "Project '{}' not found for owner '{}'. Available projects: [{}]",
        name,
        owner,
        rendered.join(", ")
    )
}

/// Read a `.env` file into key/value pairs, matching `load_dotenv()`.
pub fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let (k, v) = line.split_once('=').expect("checked contains('=')");
        let k = k.trim();
        let mut v = v.trim().to_string();
        if v.len() > 1 {
            let bytes = v.as_bytes();
            let first = bytes[0] as char;
            if first == bytes[bytes.len() - 1] as char && (first == '"' || first == '\'') {
                v = v[1..v.len() - 1].to_string();
            }
        }
        if !k.is_empty() {
            out.push((k.to_string(), v));
        }
    }
    out
}

/// The two `.env`-configurable settings, with their defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipErrors {
    Strict,
    Ignore,
}

impl RelationshipErrors {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Ignore => "ignore",
        }
    }
}

/// Effective settings plus where each value came from, mirroring
/// `load_settings()`. The source label is surfaced in the UI so users can see
/// whether a value came from the shell, `.env`, or the default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub relationship_errors: RelationshipErrors,
    pub relationship_errors_source: String,
    pub skip_relationships: bool,
    pub skip_relationships_source: String,
    /// One line per setting whose value was rejected. The CLI prints these to
    /// stderr as it loads them; the UI shows them next to the settings row.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            relationship_errors: RelationshipErrors::Strict,
            relationship_errors_source: "default".to_string(),
            skip_relationships: false,
            skip_relationships_source: "default".to_string(),
            warnings: Vec::new(),
        }
    }
}

impl Settings {
    /// The stderr lines `load_settings()` would have printed, newline-terminated.
    pub fn warnings_text(&self) -> String {
        let mut out = String::new();
        for w in &self.warnings {
            out.push_str(w);
            out.push('\n');
        }
        out
    }
}

/// Resolve settings with Python's precedence: real environment first, then
/// `.env`, then the default. Invalid values fall back to the default with the
/// source annotated, exactly as `load_settings()` does.
pub fn load_settings(
    dotenv: &[(String, String)],
    get_env: impl Fn(&str) -> Option<String>,
) -> Settings {
    let lookup = |key: &str| -> (String, String) {
        if let Some(v) = get_env(key) {
            return (v, "environment".to_string());
        }
        if let Some((_, v)) = dotenv.iter().find(|(k, _)| k == key) {
            return (v.clone(), ".env".to_string());
        }
        (String::new(), "default".to_string())
    };

    let (raw, origin) = lookup("RELATIONSHIP_ERRORS");
    let mut warnings = Vec::new();
    let (relationship_errors, relationship_errors_source) = if raw.is_empty() {
        (RelationshipErrors::Strict, origin)
    } else {
        match raw.trim().to_lowercase().as_str() {
            "strict" => (RelationshipErrors::Strict, origin),
            "ignore" => (RelationshipErrors::Ignore, origin),
            _ => {
                // Python: `! {k}={raw!r} invalid ({origin}); falling back to
                // {default}.` — `{raw!r}` is Python's repr, so the value is
                // single-quoted unless it contains one.
                warnings.push(format!(
                    "! RELATIONSHIP_ERRORS={} invalid ({}); falling back to strict.",
                    py_repr(&raw),
                    origin
                ));
                (
                    RelationshipErrors::Strict,
                    format!("{} (invalid)", origin),
                )
            }
        }
    };

    let (raw, origin) = lookup("SKIP_RELATIONSHIPS");
    let (skip_relationships, skip_relationships_source) = if raw.is_empty() {
        (false, origin)
    } else {
        match raw.trim().to_lowercase().as_str() {
            "true" => (true, origin),
            "false" => (false, origin),
            _ => {
                warnings.push(format!(
                    "! SKIP_RELATIONSHIPS={} invalid ({}); falling back to false.",
                    py_repr(&raw),
                    origin
                ));
                (false, format!("{} (invalid)", origin))
            }
        }
    };

    Settings {
        relationship_errors,
        relationship_errors_source,
        skip_relationships,
        skip_relationships_source,
        warnings,
    }
}

/// Python's `repr()` for a string, for the two cases the settings warning can
/// hit: `'x'` normally, and `"x'y"` when the value itself contains a quote.
fn py_repr(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') {
        format!("\"{s}\"")
    } else {
        format!("'{}'", s.replace('\'', "\\'"))
    }
}

/// Reproduce the CLI's startup settings banner.
pub fn settings_banner(s: &Settings) -> String {
    format!(
        "Settings:\n  RELATIONSHIP_ERRORS={} ({})\n  SKIP_RELATIONSHIPS={} ({})\n",
        s.relationship_errors.as_str(),
        s.relationship_errors_source,
        s.skip_relationships,
        s.skip_relationships_source
    )
}

/// Seed the id→number map from a `.import-state.json` payload.
pub fn issues_from_state(v: &Value) -> std::collections::BTreeMap<String, u64> {
    let mut out = std::collections::BTreeMap::new();
    if let Some(map) = v.get("issues").and_then(|m| m.as_object()) {
        for (k, val) in map {
            if let Some(n) = val.as_u64() {
                out.insert(k.clone(), n);
            }
        }
    }
    out
}

/// Render an id→number map the way the Python tool writes `.import-state.json`
/// (`json.dumps({"issues": imap}, indent=2)`, keys in insertion order).
pub fn state_to_json(issues: &std::collections::BTreeMap<String, u64>) -> Value {
    let mut map = Map::new();
    for (k, v) in issues {
        map.insert(k.clone(), Value::from(*v));
    }
    let mut root = Map::new();
    root.insert("issues".to_string(), Value::Object(map));
    Value::Object(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_task(id: &str) -> Value {
        json!({
            "id": id,
            "module": "setup",
            "title": "A task",
            "priority": "P0",
            "labels": ["frontend"],
            "body": "Do the thing.",
            "depends_on": [],
            "parent": null
        })
    }

    #[test]
    fn accepts_a_valid_backlog() {
        let tasks = vec![valid_task("SETUP-001"), valid_task("SETUP-002")];
        let r = validate(&tasks);
        assert!(r.ok, "{:?}", r.errors);
        assert_eq!(r.to_cli_output(), "OK: 2 tasks validated.\n");
    }

    #[test]
    fn reports_missing_fields() {
        let mut t = valid_task("A-001");
        t.as_object_mut().unwrap().remove("module");
        let r = validate(&[t]);
        assert_eq!(r.errors, vec!["A-001: missing module"]);
        assert_eq!(
            r.to_cli_output(),
            "Validation failed:\n- A-001: missing module\n"
        );
    }

    #[test]
    fn missing_id_uses_question_mark_in_missing_message() {
        let mut t = valid_task("A-001");
        let o = t.as_object_mut().unwrap();
        o.remove("id");
        // Python's `t.get('id', '?')` renders the missing-field message with a
        // bare '?'. Only that one error appears: the task is otherwise valid,
        // and pass 2 skips a task with no id.
        let r = validate(&[t]);
        assert_eq!(r.errors, vec!["?: missing id"]);
        assert_eq!(r.to_cli_output(), "Validation failed:\n- ?: missing id\n");
    }

    #[test]
    fn detects_duplicate_ids() {
        let tasks = vec![valid_task("A-001"), valid_task("A-001")];
        let r = validate(&tasks);
        assert_eq!(r.errors, vec!["A-001: duplicate ID"]);
    }

    #[test]
    fn detects_invalid_priority() {
        let mut t = valid_task("A-001");
        t["priority"] = json!("P9");
        let r = validate(&[t]);
        assert_eq!(r.errors, vec!["A-001: invalid priority"]);
    }

    #[test]
    fn missing_priority_is_invalid_since_key_is_absent() {
        let mut t = valid_task("A-001");
        t.as_object_mut().unwrap().remove("priority");
        let r = validate(&[t]);
        assert!(r.errors.contains(&"A-001: missing priority".to_string()));
        assert!(r.errors.contains(&"A-001: invalid priority".to_string()));
    }

    #[test]
    fn detects_unknown_dependency() {
        let mut t = valid_task("A-001");
        t["depends_on"] = json!(["NOPE-001"]);
        let r = validate(&[t]);
        assert_eq!(r.errors, vec!["A-001: unknown dependency NOPE-001"]);
    }

    #[test]
    fn detects_self_dependency() {
        let mut t = valid_task("A-001");
        t["depends_on"] = json!(["A-001"]);
        let r = validate(&[t]);
        assert_eq!(r.errors, vec!["A-001: self dependency"]);
    }

    #[test]
    fn self_dependency_that_is_also_known_reports_only_self() {
        // The Python checks `d not in ids` first, then `d == tid`; a self
        // dependency is in `ids`, so only the self error is emitted.
        let mut t = valid_task("A-001");
        t["depends_on"] = json!(["A-001"]);
        let r = validate(&[t]);
        assert!(!r.errors.iter().any(|e| e.contains("unknown dependency")));
    }

    #[test]
    fn detects_unknown_parent() {
        let mut t = valid_task("A-001");
        t["parent"] = json!("GHOST-001");
        let r = validate(&[t]);
        assert_eq!(r.errors, vec!["A-001: unknown parent GHOST-001"]);
    }

    #[test]
    fn null_and_empty_parent_are_ignored() {
        let mut a = valid_task("A-001");
        a["parent"] = json!(null);
        let mut b = valid_task("A-002");
        b["parent"] = json!("");
        let r = validate(&[a, b]);
        assert!(r.ok, "{:?}", r.errors);
    }

    #[test]
    fn every_falsy_parent_is_ignored_like_python() {
        // Python's guard is `if t["parent"] and ...`, so its truthiness rules
        // apply: 0, false, "", [], and {} all mean "no parent" rather than
        // "unknown parent 0". A port that only checked for null would report
        // four spurious errors here.
        for falsy in [json!(null), json!(false), json!(0), json!(""), json!([]), json!({})] {
            let mut t = valid_task("A-001");
            t["parent"] = falsy.clone();
            let r = validate(&[t]);
            assert!(r.ok, "parent {falsy} produced {:?}", r.errors);
        }
    }

    #[test]
    fn a_non_empty_parent_that_is_absent_is_reported() {
        // The counterpart to the test above: truthy-but-unknown still errors.
        for truthy in [json!("GHOST-001"), json!(1), json!(true), json!(["GHOST-001"])] {
            let mut t = valid_task("A-001");
            t["parent"] = truthy.clone();
            let r = validate(&[t]);
            assert_eq!(r.errors.len(), 1, "parent {truthy} produced {:?}", r.errors);
            assert!(
                r.errors[0].starts_with("A-001: unknown parent"),
                "parent {truthy} produced {:?}",
                r.errors
            );
        }
    }

    #[test]
    fn a_string_depends_on_iterates_per_character_like_python() {
        // `for d in t["depends_on"]` walks a bare string one character at a
        // time, so a plausible typo yields one error per character. The port
        // reproduces that rather than quietly treating the value as one id.
        let mut a = valid_task("A-001");
        a["depends_on"] = json!("A-002");
        let b = valid_task("A-002");
        let r = validate(&[a, b]);
        assert_eq!(
            r.errors,
            vec![
                "A-001: unknown dependency A",
                "A-001: unknown dependency -",
                "A-001: unknown dependency 0",
                "A-001: unknown dependency 0",
                "A-001: unknown dependency 2",
            ]
        );
    }

    #[test]
    fn a_non_array_non_string_depends_on_is_ignored() {
        // Numbers, objects, null, and booleans are not iterable in Python, so
        // those inputs would crash the CLI; the port skips them instead. This
        // pins the behaviour so the divergence stays deliberate.
        for value in [json!(null), json!(1), json!(true)] {
            let mut t = valid_task("A-001");
            t["depends_on"] = value.clone();
            let r = validate(&[t]);
            assert!(r.ok, "depends_on {value} produced {:?}", r.errors);
        }
    }

    #[test]
    fn pass_one_errors_precede_pass_two_errors() {
        // Ordering is part of the contract the parity oracle diffs.
        let mut a = valid_task("A-001");
        a["priority"] = json!("P9");
        let mut b = valid_task("A-002");
        b["depends_on"] = json!(["MISSING-001"]);
        let r = validate(&[a, b]);
        assert_eq!(
            r.errors,
            vec!["A-001: invalid priority", "A-002: unknown dependency MISSING-001"]
        );
    }

    #[test]
    fn parent_cycle_is_accepted() {
        // Two tasks parenting each other is not something validate() rejects.
        let mut a = valid_task("A-001");
        a["parent"] = json!("A-002");
        let mut b = valid_task("A-002");
        b["parent"] = json!("A-001");
        let r = validate(&[a, b]);
        assert!(r.ok, "{:?}", r.errors);
    }

    #[test]
    fn title_regex_matches_the_documented_shapes() {
        assert_eq!(task_id_from_title("[SETUP-001] Init").as_deref(), Some("SETUP-001"));
        assert_eq!(
            task_id_from_title("[AUTH-FE-003] Login").as_deref(),
            Some("AUTH-FE-003")
        );
        assert_eq!(
            task_id_from_title("[DOC-FE-001] Docs").as_deref(),
            Some("DOC-FE-001")
        );
    }

    #[test]
    fn title_regex_rejects_near_misses() {
        // Must not match: no brackets, wrong digit count, lowercase, no space
        // after the bracket, or a prefix before the bracket.
        assert_eq!(task_id_from_title("SETUP-001 Init"), None);
        assert_eq!(task_id_from_title("[SETUP-01] Init"), None);
        assert_eq!(task_id_from_title("[setup-001] Init"), None);
        assert_eq!(task_id_from_title("[SETUP-001]Init"), None);
        assert_eq!(task_id_from_title("x [SETUP-001] Init"), None);
        assert_eq!(task_id_from_title("[SETUP-0001] Init"), None);
    }

    #[test]
    fn title_regex_requires_leading_position() {
        // Python uses re.match, which anchors at position 0.
        assert_eq!(task_id_from_title("Re: [SETUP-001] Init"), None);
    }

    #[test]
    fn issue_body_is_byte_stable() {
        let t = Task {
            id: "SETUP-001".into(),
            module: "setup".into(),
            title: "Init".into(),
            priority: "P0".into(),
            labels: vec![],
            body: "Hello.".into(),
            depends_on: vec![],
            parent: None,
        };
        let expected = "<!-- github-task:SETUP-001 -->\n\n**Module:** `setup`  \n**Priority:** `P0`\n\nHello.\n\n## Acceptance Criteria\n\n- [ ] Implementation completed\n- [ ] Appropriate tests added\n- [ ] API/documentation updated where applicable\n";
        assert_eq!(issue_body(&t), expected);
    }

    #[test]
    fn issue_title_is_byte_stable() {
        let t = Task {
            id: "SETUP-001".into(),
            module: "setup".into(),
            title: "Init".into(),
            priority: "P0".into(),
            labels: vec![],
            body: String::new(),
            depends_on: vec![],
            parent: None,
        };
        assert_eq!(issue_title(&t), "[SETUP-001] Init");
        assert_eq!(body_marker("SETUP-001"), "<!-- github-task:SETUP-001 -->");
    }

    #[test]
    fn labels_are_deduped_in_order() {
        let t = Task {
            id: "A-001".into(),
            module: "setup".into(),
            title: "t".into(),
            priority: "P0".into(),
            // "backend" and "setup" duplicate the implicit prefixes.
            labels: vec!["backend".into(), "frontend".into(), "setup".into()],
            body: String::new(),
            depends_on: vec![],
            parent: None,
        };
        assert_eq!(issue_labels(&t), vec!["backend", "setup", "P0", "frontend"]);
    }

    #[test]
    fn needed_labels_include_implicit_ones() {
        let t = Task {
            id: "A-001".into(),
            module: "setup".into(),
            title: "t".into(),
            priority: "P0".into(),
            labels: vec!["ui".into()],
            body: String::new(),
            depends_on: vec![],
            parent: None,
        };
        assert_eq!(needed_labels(&[t]), vec!["P0", "P1", "P2", "P3", "backend", "setup", "ui"]);
    }

    #[test]
    fn preview_line_pads_like_python() {
        let t = Task {
            id: "A-001".into(),
            module: "setup".into(),
            title: "Init".into(),
            priority: "P0".into(),
            labels: vec![],
            body: String::new(),
            depends_on: vec![],
            parent: None,
        };
        assert_eq!(preview_line(&t), "A-001           P0 setup              Init");
    }

    #[test]
    fn existing_detects_cli_created_titles() {
        let issues = vec![
            (1u64, "[SETUP-001] Init"),
            (2u64, "Random issue"),
            (3u64, "[AUTH-FE-003] Login"),
        ];
        let found = existing_from_issues(issues);
        assert_eq!(found.get("SETUP-001"), Some(&1));
        assert_eq!(found.get("AUTH-FE-003"), Some(&3));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn dotenv_parsing_matches_python() {
        let parsed = parse_dotenv(
            "# comment\n\nRELATIONSHIP_ERRORS=ignore\nSKIP_RELATIONSHIPS = \"true\"\nnot a pair\n",
        );
        assert_eq!(
            parsed,
            vec![
                ("RELATIONSHIP_ERRORS".to_string(), "ignore".to_string()),
                ("SKIP_RELATIONSHIPS".to_string(), "true".to_string()),
            ]
        );
    }

    #[test]
    fn settings_precedence_env_over_dotenv_over_default() {
        let dotenv = vec![
            ("RELATIONSHIP_ERRORS".to_string(), "ignore".to_string()),
            ("SKIP_RELATIONSHIPS".to_string(), "true".to_string()),
        ];
        let s = load_settings(&dotenv, |k| {
            (k == "RELATIONSHIP_ERRORS").then(|| "strict".to_string())
        });
        assert_eq!(s.relationship_errors, RelationshipErrors::Strict);
        assert_eq!(s.relationship_errors_source, "environment");
        assert!(s.skip_relationships);
        assert_eq!(s.skip_relationships_source, ".env");
    }

    #[test]
    fn invalid_setting_falls_back_and_annotates_source() {
        let dotenv = vec![("RELATIONSHIP_ERRORS".to_string(), "nonsense".to_string())];
        let s = load_settings(&dotenv, |_| None);
        assert_eq!(s.relationship_errors, RelationshipErrors::Strict);
        assert_eq!(s.relationship_errors_source, ".env (invalid)");
    }

    #[test]
    fn state_round_trips() {
        let v = json!({ "issues": { "A-001": 12, "A-002": 13 } });
        let map = issues_from_state(&v);
        assert_eq!(map.get("A-001"), Some(&12));
        assert_eq!(state_to_json(&map), v);
    }
}
