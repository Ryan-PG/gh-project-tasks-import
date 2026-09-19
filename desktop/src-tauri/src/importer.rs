//! Import orchestration: preflight, label creation, issue creation, adding to
//! the project board, and the relationship pass.
//!
//! Two properties drive the shape of this module:
//!
//! * **Resumability.** `POST /issues` and `addProjectV2ItemById` are two calls
//!   where the CLI's `gh issue create --project` was one, so an issue can exist
//!   without being on the board. State is written after *each step*, and a
//!   rerun reconciles rather than starting over.
//! * **Idempotency.** Issues are recognised by the frozen title regex and body
//!   marker, so a rerun never duplicates what the CLI already created.

use crate::github::{GithubClient, GithubError, RepoRef};
use crate::graphql::Project;
use crate::state::ImportState;
use crate::tasks::{self, Settings, Task};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Which stage of the run is in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preflight,
    Labels,
    Issues,
    Project,
    Relationships,
    Finished,
}

impl Phase {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Preflight => "Checking access",
            Self::Labels => "Creating labels",
            Self::Issues => "Creating issues",
            Self::Project => "Adding to the project board",
            Self::Relationships => "Applying relationships",
            Self::Finished => "Finished",
        }
    }
}

/// The outcome of one task in one phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Created,
    /// Already existed on GitHub, so nothing was written.
    Skipped,
    Added,
    /// A relationship was already in place.
    AlreadySet,
    Failed,
    /// Counted only, for a dry run.
    WouldCreate,
    WouldAdd,
}

/// Progress emitted to the frontend as the run proceeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportEvent {
    Phase {
        phase: Phase,
        message: String,
    },
    Task {
        phase: Phase,
        index: usize,
        total: usize,
        id: String,
        status: TaskStatus,
        detail: Option<String>,
    },
    Log {
        message: String,
    },
    Done {
        summary: Box<ImportSummary>,
    },
    Failed {
        message: String,
    },
}

/// Receives progress. Kept as a trait so the importer can be tested, and run
/// headless, without a Tauri app handle.
pub trait ProgressSink: Send + Sync {
    fn emit(&self, event: ImportEvent);
}

/// Discards everything. For tests and headless runs.
pub struct NullSink;

impl ProgressSink for NullSink {
    fn emit(&self, _event: ImportEvent) {}
}

/// Collects events in memory. For tests.
#[derive(Default)]
pub struct VecSink(pub std::sync::Mutex<Vec<ImportEvent>>);

impl ProgressSink for VecSink {
    fn emit(&self, event: ImportEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl VecSink {
    pub fn logs(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                ImportEvent::Log { message } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn summary(&self) -> Option<ImportSummary> {
        self.0.lock().unwrap().iter().find_map(|e| match e {
            ImportEvent::Done { summary } => Some((**summary).clone()),
            _ => None,
        })
    }
}

/// Running totals, mirroring the CLI's end-of-run report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub created: usize,
    pub skipped: usize,
    pub tracked: usize,
    pub labels_created: Vec<String>,
    pub project_items_added: usize,
    pub relationships_applied: usize,
    pub relationships_already: usize,
    pub relationships_failed: usize,
    pub already_ids: Vec<String>,
    pub failed_ids: Vec<String>,
    /// Non-fatal problems worth showing at the end.
    pub warnings: Vec<String>,
    pub dry_run: bool,
}

impl ImportSummary {
    /// Reproduce the CLI's closing lines so a dry run can be diffed against the
    /// Python tool by the parity oracle.
    pub fn to_cli_output(&self, skip_relationships: bool) -> String {
        let mut out = String::new();
        if skip_relationships {
            out.push_str("\nRelationships skipped (SKIP_RELATIONSHIPS=true).\n");
        } else {
            out.push_str(&format!(
                "\nAlready set ({}): {}\n",
                self.relationships_already,
                if self.already_ids.is_empty() {
                    "none".to_string()
                } else {
                    self.already_ids.join(", ")
                }
            ));
            if !self.failed_ids.is_empty() {
                out.push_str(&format!(
                    "Failed ({}): {}\n",
                    self.relationships_failed,
                    self.failed_ids.join(", ")
                ));
            }
            out.push_str(&format!(
                "Relationships: {} applied, {} already set, {} failed.\n",
                self.relationships_applied, self.relationships_already, self.relationships_failed
            ));
        }
        out.push_str(&format!(
            "\nDone. Created {}; tracked {} tasks.\n",
            self.created, self.tracked
        ));
        out
    }
}

/// What to do.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Resolve everything but write nothing.
    pub dry_run: bool,
    /// Skip the parent / blocked-by pass, as `SKIP_RELATIONSHIPS=true` does.
    pub skip_relationships: bool,
    /// Skip adding issues to the Projects V2 board.
    pub skip_project: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            skip_relationships: false,
            skip_project: false,
        }
    }
}

/// Everything a run needs.
pub struct Importer<'a> {
    pub client: &'a GithubClient,
    pub repo: RepoRef,
    pub project: Option<Project>,
    pub tasks: Vec<Task>,
    pub settings: Settings,
    pub state: ImportState,
    pub state_path: PathBuf,
    pub options: ImportOptions,
    pub sink: Arc<dyn ProgressSink>,
    pub cancel: Arc<AtomicBool>,
}

/// The `link()` helper's per-call result.
enum LinkResult {
    Applied,
    Already,
    Failed(String),
}

impl<'a> Importer<'a> {
    fn emit(&self, e: ImportEvent) {
        self.sink.emit(e);
    }

    fn log(&self, message: impl Into<String>) {
        self.emit(ImportEvent::Log {
            message: message.into(),
        });
    }

    fn phase(&self, phase: Phase, message: impl Into<String>) {
        self.emit(ImportEvent::Phase {
            phase,
            message: message.into(),
        });
    }

    fn task_event(
        &self,
        phase: Phase,
        index: usize,
        total: usize,
        id: &str,
        status: TaskStatus,
        detail: Option<String>,
    ) {
        self.emit(ImportEvent::Task {
            phase,
            index,
            total,
            id: id.to_string(),
            status,
            detail,
        });
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Persist state. Called after each step rather than each task, so a crash
    /// mid-task leaves enough information to resume.
    fn save_state(&mut self) -> Result<(), GithubError> {
        if self.options.dry_run {
            return Ok(());
        }
        self.state
            .save(&self.state_path)
            .map_err(|e| GithubError::Other(format!("could not write the state file: {e}")))
    }

    /// Preflight: confirm the token can see the repository, and that the
    /// configured project exists.
    pub async fn preflight(&mut self) -> Result<(), GithubError> {
        self.phase(Phase::Preflight, "Checking repository access");
        let repo_json = self.client.get_repo(&self.repo).await?;
        let name = repo_json
            .get("full_name")
            .and_then(|n| n.as_str())
            .unwrap_or(&format!("{}/{}", self.repo.owner, self.repo.name))
            .to_string();
        self.log(format!("Repository: {name}"));

        if let Some(project) = &self.project {
            self.log(format!(
                "Project: {} ({} owner, number {})",
                project.title, project.owner_kind, project.number
            ));
        }
        Ok(())
    }

    /// Port of `existing()`, plus the persistent state as a seed.
    ///
    /// The API is the source of truth; the state file only saves a round trip
    /// on the id→database-id lookups. That is why a missing state file costs
    /// time but never correctness.
    pub async fn discover_existing(&mut self) -> Result<BTreeMap<String, u64>, GithubError> {
        self.phase(Phase::Preflight, "Looking for issues that already exist");

        let issues = self.client.list_issues(&self.repo).await?;
        let pairs: Vec<(u64, &str)> = issues
            .iter()
            .map(|i| (i.number, i.title.as_str()))
            .collect();
        let found = tasks::existing_from_issues(pairs);

        // Cache the ids we just learned, so the relationship pass does not have
        // to re-fetch them one by one.
        for issue in &issues {
            if let Some(id) = tasks::task_id_from_title(&issue.title) {
                self.state.issue_ids.insert(id.clone(), issue.id);
                if let Some(node) = &issue.node_id {
                    self.state.node_ids.insert(id, node.clone());
                }
            }
        }

        self.state.seed_issues(&found);
        self.save_state()?;
        Ok(found)
    }

    /// Port of `ensure_labels`.
    pub async fn ensure_labels(&mut self, out: &mut ImportSummary) -> Result<(), GithubError> {
        self.phase(Phase::Labels, "Creating any missing labels");
        let existing = self.client.list_labels(&self.repo).await?;
        let have: std::collections::HashSet<String> =
            existing.into_iter().map(|l| l.name).collect();

        for label in tasks::needed_labels(&self.tasks) {
            if have.contains(&label) {
                continue;
            }
            if self.cancelled() {
                return Ok(());
            }
            if self.options.dry_run {
                self.log(format!("+ label {label} (dry run)"));
                out.labels_created.push(label);
                continue;
            }
            // A 422 here means the label appeared between the list and the
            // create; `create_label` treats that as success.
            self.client.create_label(&self.repo, &label).await?;
            self.log(format!("+ label {label}"));
            out.labels_created.push(label);
        }
        Ok(())
    }

    /// Create every missing issue, then add each to the project board.
    ///
    /// These are separate phases because they are separate API calls with a
    /// resumable window between them.
    pub async fn create_issues(
        &mut self,
        imap: &mut BTreeMap<String, u64>,
        out: &mut ImportSummary,
    ) -> Result<(), GithubError> {
        self.phase(Phase::Issues, "Creating issues");
        let total = self.tasks.len();
        // Snapshot what each task needs before the loop: the body of the loop
        // calls `&mut self` (to save state), which cannot overlap a borrow of
        // `self.tasks`.
        let plan: Vec<(String, String, String, Vec<String>)> = self
            .tasks
            .iter()
            .map(|t| {
                (
                    t.id.clone(),
                    tasks::issue_title(t),
                    tasks::issue_body(t),
                    tasks::issue_labels(t),
                )
            })
            .collect();

        for (index, (id, title, body, labels)) in plan.into_iter().enumerate() {
            if self.cancelled() {
                self.log("Cancelled.");
                return Ok(());
            }
            if let Some(number) = imap.get(&id) {
                self.log(format!("= {id} -> #{number}"));
                out.skipped += 1;
                self.task_event(
                    Phase::Issues,
                    index,
                    total,
                    &id,
                    TaskStatus::Skipped,
                    Some(format!("#{number}")),
                );
                continue;
            }

            if self.options.dry_run {
                self.log(format!("+ creating {id} (dry run)"));
                out.created += 1;
                self.task_event(
                    Phase::Issues,
                    index,
                    total,
                    &id,
                    TaskStatus::WouldCreate,
                    None,
                );
                continue;
            }

            self.log(format!("+ creating {id}"));
            let created = self
                .client
                .create_issue(&self.repo, &title, &body, &labels)
                .await?;

            imap.insert(id.clone(), created.number);
            self.state.issues.insert(id.clone(), created.number);
            self.state.issue_ids.insert(id.clone(), created.id);
            self.state.node_ids.insert(id.clone(), created.node_id.clone());
            // Write immediately: the issue now exists on GitHub, and a crash
            // before the next line must not lose that fact.
            self.save_state()?;

            out.created += 1;
            self.task_event(
                Phase::Issues,
                index,
                total,
                &id,
                TaskStatus::Created,
                Some(format!("#{}", created.number)),
            );
        }
        Ok(())
    }

    /// Add every tracked issue to the Projects V2 board.
    ///
    /// Resumable independently of issue creation: an issue that exists but is
    /// not on the board is picked up here on the next run.
    pub async fn add_to_project(
        &mut self,
        imap: &BTreeMap<String, u64>,
        out: &mut ImportSummary,
    ) -> Result<(), GithubError> {
        if self.options.skip_project {
            self.log("Project board step skipped.");
            return Ok(());
        }
        let Some(project) = self.project.clone() else {
            return Ok(());
        };

        self.phase(Phase::Project, "Adding issues to the project board");
        let total = self.tasks.len();
        // Snapshot the ids: the loop body calls `&mut self` to save state.
        let ids: Vec<String> = self.tasks.iter().map(|t| t.id.clone()).collect();

        for (index, id) in ids.into_iter().enumerate() {
            if self.cancelled() {
                self.log("Cancelled.");
                return Ok(());
            }
            // Already on the board, per the state file.
            if self.state.project_items.contains_key(&id) {
                self.task_event(Phase::Project, index, total, &id, TaskStatus::Skipped, None);
                continue;
            }
            let Some(number) = imap.get(&id).copied() else {
                // Not created yet — a dry run, or a cancelled earlier phase.
                continue;
            };

            let node_id = match self.state.node_ids.get(&id) {
                Some(n) => n.clone(),
                None => {
                    // Should not happen for tasks we created, but a task that
                    // already existed on GitHub arrives here without one.
                    match self.resolve_node_id(&id, number).await {
                        Ok(n) => n,
                        Err(e) => {
                            out.warnings.push(format!("{id}: {e}"));
                            continue;
                        }
                    }
                }
            };

            if self.options.dry_run {
                self.task_event(
                    Phase::Project,
                    index,
                    total,
                    &id,
                    TaskStatus::WouldAdd,
                    None,
                );
                continue;
            }

            match self.client.add_project_item(&project.id, &node_id).await {
                Ok(item_id) => {
                    self.state.project_items.insert(id.clone(), item_id);
                    // Written per step: this is exactly the window the plan
                    // flags as the partial-failure risk.
                    self.save_state()?;
                    out.project_items_added += 1;
                    self.task_event(Phase::Project, index, total, &id, TaskStatus::Added, None);
                }
                Err(e) => {
                    out.warnings
                        .push(format!("{id}: could not add to the project: {e}"));
                    self.task_event(
                        Phase::Project,
                        index,
                        total,
                        &id,
                        TaskStatus::Failed,
                        Some(e.to_string()),
                    );
                }
            }
        }
        Ok(())
    }

    /// Resolve an issue's GraphQL node id, caching the result.
    async fn resolve_node_id(&mut self, id: &str, number: u64) -> Result<String, GithubError> {
        let issue = self.client.get_issue(&self.repo, number).await?;
        self.state.issue_ids.insert(id.to_string(), issue.id);
        let node = issue
            .node_id
            .ok_or_else(|| GithubError::Other("issue has no node_id".into()))?;
        self.state.node_ids.insert(id.to_string(), node.clone());
        self.save_state()?;
        Ok(node)
    }

    /// Resolve an issue's database id, caching the result.
    ///
    /// Sub-issue and dependency endpoints take the integer database id, not the
    /// issue number, so an issue that predates this run needs one extra GET.
    async fn resolve_db_id(
        &mut self,
        id: &str,
        number: u64,
        out: &mut ImportSummary,
    ) -> Option<u64> {
        if let Some(db) = self.state.issue_ids.get(id) {
            return Some(*db);
        }
        match self.client.get_issue(&self.repo, number).await {
            Ok(issue) => {
                self.state.issue_ids.insert(id.to_string(), issue.id);
                if let Some(node) = issue.node_id {
                    self.state.node_ids.insert(id.to_string(), node);
                }
                if let Err(e) = self.save_state() {
                    out.warnings.push(format!("could not save state: {e}"));
                }
                Some(issue.id)
            }
            Err(e) => {
                out.warnings
                    .push(format!("{id}: could not resolve its database id: {e}"));
                None
            }
        }
    }

    /// Port of the relationship pass.
    ///
    /// Both link types are re-applied on every run; GitHub answers "already
    /// been taken" for links that exist, which `ignore` mode tolerates and
    /// `strict` mode aborts on — matching the CLI exactly.
    pub async fn apply_relationships(
        &mut self,
        imap: &BTreeMap<String, u64>,
        out: &mut ImportSummary,
    ) -> Result<(), GithubError> {
        if self.settings.skip_relationships {
            self.log("\nRelationships skipped (SKIP_RELATIONSHIPS=true).");
            return Ok(());
        }

        self.phase(Phase::Relationships, "Applying parent and blocked-by links");
        let strict = self.settings.relationship_errors == tasks::RelationshipErrors::Strict;
        let total = self.tasks.len();
        // Snapshot the task metadata so `resolve_db_id` can borrow self mutably.
        let plan: Vec<(String, Option<String>, Vec<String>)> = self
            .tasks
            .iter()
            .map(|t| (t.id.clone(), t.parent.clone(), t.depends_on.clone()))
            .collect();

        for (index, (id, parent, depends_on)) in plan.iter().enumerate() {
            if self.cancelled() {
                self.log("Cancelled.");
                return Ok(());
            }
            let Some(&number) = imap.get(id) else { continue };

            if self.options.dry_run {
                if parent.is_some() || !depends_on.is_empty() {
                    self.task_event(
                        Phase::Relationships,
                        index,
                        total,
                        id,
                        TaskStatus::WouldAdd,
                        None,
                    );
                }
                continue;
            }

            // Sub-issue: the child's database id goes in the body, the parent's
            // number in the path.
            if let Some(parent_id) = parent {
                if let Some(&parent_number) = imap.get(parent_id) {
                    let result = match self.resolve_db_id(id, number, out).await {
                        Some(child_db) => {
                            match self
                                .client
                                .add_sub_issue(&self.repo, parent_number, child_db)
                                .await
                            {
                                Ok(()) => LinkResult::Applied,
                                Err(e) if e.is_already_set() => LinkResult::Already,
                                Err(e) => LinkResult::Failed(e.to_string()),
                            }
                        }
                        None => LinkResult::Failed("could not resolve the child's database id".into()),
                    };
                    if self.record_link(
                        id,
                        "parent",
                        result,
                        strict,
                        out,
                        index,
                        total,
                    )? {
                        return Ok(());
                    }
                }
            }

            // Blocked-by: the dependency's database id goes in the body.
            for dep in depends_on {
                let Some(&dep_number) = imap.get(dep) else {
                    continue;
                };
                let result = match self.resolve_db_id(dep, dep_number, out).await {
                    Some(blocking_db) => {
                        match self
                            .client
                            .add_blocked_by(&self.repo, number, blocking_db)
                            .await
                        {
                            Ok(()) => LinkResult::Applied,
                            Err(e) if e.is_already_set() => LinkResult::Already,
                            Err(e) => LinkResult::Failed(e.to_string()),
                        }
                    }
                    None => LinkResult::Failed("could not resolve the blocker's database id".into()),
                };
                if self.record_link(
                    id,
                    "blocked-by",
                    result,
                    strict,
                    out,
                    index,
                    total,
                )? {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Tally one link attempt and emit its progress event.
    ///
    /// Returns `Ok(true)` when `strict` mode must abort the run, matching the
    /// CLI's `SystemExit` on the first relationship failure.
    #[allow(clippy::too_many_arguments)]
    fn record_link(
        &self,
        id: &str,
        what: &str,
        result: LinkResult,
        strict: bool,
        out: &mut ImportSummary,
        index: usize,
        total: usize,
    ) -> Result<bool, GithubError> {
        match result {
            LinkResult::Applied => {
                out.relationships_applied += 1;
                self.task_event(
                    Phase::Relationships,
                    index,
                    total,
                    id,
                    TaskStatus::Added,
                    Some(what.to_string()),
                );
                Ok(false)
            }
            LinkResult::Already => {
                out.relationships_already += 1;
                out.already_ids.push(format!("{id} ({what})"));
                self.task_event(
                    Phase::Relationships,
                    index,
                    total,
                    id,
                    TaskStatus::AlreadySet,
                    Some(what.to_string()),
                );
                Ok(false)
            }
            LinkResult::Failed(message) => {
                if strict {
                    // Strict mode stops at the first failure, as the CLI does.
                    return Err(GithubError::Other(format!(
                        "{id} {what} failed: {}",
                        message.lines().next().unwrap_or(&message)
                    )));
                }
                out.relationships_failed += 1;
                out.failed_ids.push(format!("{id} ({what})"));
                self.log(format!(
                    "! {id} {what} failed: {}",
                    message.lines().next().unwrap_or(&message)
                ));
                self.task_event(
                    Phase::Relationships,
                    index,
                    total,
                    id,
                    TaskStatus::Failed,
                    Some(what.to_string()),
                );
                Ok(false)
            }
        }
    }

    /// The whole run, in order.
    pub async fn run(&mut self) -> Result<ImportSummary, GithubError> {
        let mut out = ImportSummary {
            dry_run: self.options.dry_run,
            ..Default::default()
        };

        self.preflight().await?;
        let mut imap = self.discover_existing().await?;
        self.ensure_labels(&mut out).await?;
        self.create_issues(&mut imap, &mut out).await?;
        self.add_to_project(&imap, &mut out).await?;
        self.apply_relationships(&imap, &mut out).await?;

        out.tracked = imap.len();
        self.save_state()?;
        self.phase(Phase::Finished, "Finished");
        self.emit(ImportEvent::Done {
            summary: Box::new(out.clone()),
        });
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::RelationshipErrors;

    fn settings() -> Settings {
        Settings {
            relationship_errors: RelationshipErrors::Strict,
            relationship_errors_source: "default".into(),
            skip_relationships: false,
            skip_relationships_source: "default".into(),
            warnings: Vec::new(),
        }
    }

    fn summary_with(
        applied: usize,
        already: usize,
        failed: usize,
        created: usize,
        tracked: usize,
    ) -> ImportSummary {
        ImportSummary {
            created,
            tracked,
            relationships_applied: applied,
            relationships_already: already,
            relationships_failed: failed,
            already_ids: vec!["A-001 (parent)".into()],
            failed_ids: if failed > 0 {
                vec!["B-001 (blocked-by)".into()]
            } else {
                vec![]
            },
            ..Default::default()
        }
    }

    #[test]
    fn summary_matches_the_cli_closing_lines() {
        let s = summary_with(84, 5, 0, 3, 89);
        let text = s.to_cli_output(false);
        assert!(text.contains("Already set (5): A-001 (parent)"));
        assert!(text.contains("Relationships: 84 applied, 5 already set, 0 failed."));
        assert!(text.contains("Done. Created 3; tracked 89 tasks."));
        assert!(!text.contains("Failed ("));
    }

    #[test]
    fn summary_reports_failures_when_there_are_any() {
        let s = summary_with(10, 1, 2, 1, 12);
        let text = s.to_cli_output(false);
        assert!(text.contains("Failed (2): B-001 (blocked-by)"));
    }

    #[test]
    fn summary_says_none_when_nothing_was_already_set() {
        let mut s = summary_with(10, 0, 0, 1, 12);
        s.already_ids.clear();
        assert!(s.to_cli_output(false).contains("Already set (0): none"));
    }

    #[test]
    fn skipped_relationships_replace_the_report() {
        let s = summary_with(0, 0, 0, 1, 12);
        let text = s.to_cli_output(true);
        assert!(text.contains("Relationships skipped (SKIP_RELATIONSHIPS=true)."));
        assert!(!text.contains("Relationships: 0 applied"));
    }

    #[test]
    fn phase_labels_are_human_readable() {
        assert_eq!(Phase::Issues.label(), "Creating issues");
        assert_eq!(Phase::Project.label(), "Adding to the project board");
    }

    #[test]
    fn dry_run_defaults_are_off() {
        let o = ImportOptions::default();
        assert!(!o.dry_run);
        assert!(!o.skip_relationships);
        assert!(!o.skip_project);
    }

    #[test]
    fn events_serialise_with_a_kind_tag_for_the_frontend() {
        let e = ImportEvent::Task {
            phase: Phase::Issues,
            index: 1,
            total: 10,
            id: "A-001".into(),
            status: TaskStatus::Created,
            detail: Some("#12".into()),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "task");
        assert_eq!(v["phase"], "issues");
        assert_eq!(v["status"], "created");

        let p = serde_json::to_value(ImportEvent::Phase {
            phase: Phase::Labels,
            message: "x".into(),
        })
        .unwrap();
        assert_eq!(p["kind"], "phase");
        assert_eq!(p["phase"], "labels");
    }

    #[test]
    fn vec_sink_collects_logs_and_summary() {
        let sink = VecSink::default();
        sink.emit(ImportEvent::Log {
            message: "+ label backend".into(),
        });
        sink.emit(ImportEvent::Done {
            summary: Box::new(summary_with(1, 0, 0, 1, 1)),
        });
        assert_eq!(sink.logs(), vec!["+ label backend"]);
        assert_eq!(sink.summary().unwrap().created, 1);
    }

    #[test]
    fn default_settings_are_strict_and_run_relationships() {
        let s = settings();
        assert_eq!(s.relationship_errors, RelationshipErrors::Strict);
        assert!(!s.skip_relationships);
    }
}
