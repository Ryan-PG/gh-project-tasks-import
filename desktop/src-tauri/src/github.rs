//! REST client for the GitHub API.
//!
//! Uses `reqwest` + `serde` directly rather than `octocrab`: the surface is
//! about ten calls, and `octocrab`'s typed layer can lag newly-GA endpoints
//! such as sub-issues and issue dependencies.
//!
//! Everything here is deliberately sequential. Creating ~350 resources
//! back-to-back trips GitHub's secondary (abuse) rate limits, so the client
//! serialises writes and backs off on 403/429 honouring `Retry-After`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.github.com";

/// Ceiling on how many times a single request is retried before giving up.
pub(crate) const MAX_RETRIES: u32 = 5;
/// Cap on a single backoff sleep, so a hostile `Retry-After` cannot hang the app.
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Bounds on the retry budget for a paginated GET.
pub(crate) const MAX_PAGES: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    #[error("GitHub returned {status}: {message}")]
    Api {
        status: u16,
        message: String,
        /// True when GitHub reports a secondary rate limit.
        rate_limited: bool,
    },
    #[error("not authenticated")]
    Unauthenticated,
    #[error("{0}")]
    Other(String),
}

impl GithubError {
    /// GitHub reports "already exists" conditions as 422. Callers that treat
    /// those as success (label creation, re-applying a relationship) check this.
    pub fn is_validation_failed(&self) -> bool {
        matches!(self, Self::Api { status: 422, .. })
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Api { status: 404, .. })
    }

    pub fn is_forbidden(&self) -> bool {
        matches!(self, Self::Api { status: 403, .. })
    }

    /// True for the error GitHub returns when a relationship link already
    /// exists — the case `RELATIONSHIP_ERRORS=ignore` tolerates.
    pub fn is_already_set(&self) -> bool {
        is_benign_already_set(&self.to_string())
    }

    /// True when a 422 means the label is already there, and nothing else.
    ///
    /// This needs its own check rather than [`Self::is_already_set`]: GitHub's
    /// duplicate-label body is
    /// `{"message":"Validation Failed","errors":[{"resource":"Label",
    /// "code":"already_exists","field":"name"}]}`, whose only human text is the
    /// generic "Validation Failed" and whose code spells the condition with an
    /// underscore. A plain text match misses it, and a bare `status == 422`
    /// check would also swallow a genuinely invalid label name.
    pub fn is_duplicate_label(&self) -> bool {
        match self {
            Self::Api { status: 422, message, .. } => {
                message.to_lowercase().contains("already_exists")
                    || is_benign_already_set(message)
            }
            _ => false,
        }
    }
}

/// Port of the Python `BENIGN` regex: GitHub reports re-applying an existing
/// parent / blocked-by link this way, and the CLI treats it as success.
pub fn is_benign_already_set(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("already been taken") || lower.contains("already exists")
}

/// A repository reference split into owner and name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    /// Parse `owner/name`, the format `config.json` uses for `repo`.
    pub fn parse(s: &str) -> Result<Self, GithubError> {
        let (owner, name) = s
            .split_once('/')
            .ok_or_else(|| GithubError::Other(format!("repo must be 'owner/name', got '{s}'")))?;
        if owner.is_empty() || name.is_empty() {
            return Err(GithubError::Other(format!(
                "repo must be 'owner/name', got '{s}'"
            )));
        }
        Ok(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }
}

/// An issue as returned by the REST API. Only the fields the importer needs.
#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u64,
    /// The *database* id — what `sub_issue_id` and `issue_id` require.
    pub id: u64,
    /// The GraphQL global id — what `addProjectV2ItemById`'s `contentId` takes.
    #[serde(default)]
    pub node_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    /// Present only on pull requests. GitHub's `/issues` endpoint returns PRs
    /// too, whereas `gh issue list` filters them out.
    #[serde(default)]
    pub pull_request: Option<Value>,
}

impl Issue {
    pub fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }
}

/// A label as returned by the REST API.
#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub name: String,
}

/// A created or fetched issue's identity plus its database and global ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedIssue {
    pub number: u64,
    /// Database id — `sub_issue_id` / `issue_id` on the relationship endpoints.
    pub id: u64,
    /// GraphQL global id — `contentId` on `addProjectV2ItemById`.
    pub node_id: String,
}

/// Parse the `Link` header for the `rel="next"` URL.
///
/// `gh` used to do this for us; with a raw HTTP client it has to be
/// reimplemented, which is why it is factored out and tested directly.
pub fn next_link(header: Option<&str>) -> Option<String> {
    let header = header?;
    for part in header.split(',') {
        let part = part.trim();
        let (url_part, params) = part.split_once(';')?;
        let rel_is_next = params
            .split(';')
            .any(|p| p.trim().trim_start_matches("rel=").trim_matches('"') == "next");
        if rel_is_next {
            let url = url_part.trim().trim_start_matches('<').trim_end_matches('>');
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// Pull the human-readable message out of a GitHub error body.
pub(crate) fn error_message_for(status: u16, body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(m) = v.get("message").and_then(|m| m.as_str()) {
            // 422 responses carry the useful detail in `errors`.
            if let Some(errors) = v.get("errors").and_then(|e| e.as_array()) {
                let detail: Vec<String> = errors
                    .iter()
                    .filter_map(|e| {
                        e.get("message")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| Some(e.to_string()))
                    })
                    .collect();
                if !detail.is_empty() {
                    return format!("{} ({})", m, detail.join("; "));
                }
            }
            return m.to_string();
        }
    }
    if body.trim().is_empty() {
        format!("HTTP {status}")
    } else {
        body.trim().to_string()
    }
}

/// Turn a GraphQL `errors` array into a [`GithubError`].
///
/// GraphQL reports both hard failures and partial successes this way, and the
/// useful text is in `message`, sometimes nested under `extensions`.
pub(crate) fn error_from_graphql_body(errors: &[Value], whole: &Value) -> GithubError {
    let messages: Vec<String> = errors
        .iter()
        .map(|e| {
            e.get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| e.to_string())
        })
        .collect();
    let joined = messages.join("; ");

    // "not found" and "could not resolve to a node" mean a bad owner or board.
    let lower = joined.to_lowercase();
    let status = if lower.contains("could not resolve to a node") || lower.contains("not found") {
        404
    } else if lower.contains("resource not accessible") || lower.contains("forbidden") {
        403
    } else {
        200
    };

    // Preserve the raw payload when there is no message, so nothing is lost.
    let message = if joined.is_empty() {
        whole.to_string()
    } else {
        joined
    };
    let rate_limited = message.to_lowercase().contains("rate limit");

    GithubError::Api {
        status,
        message,
        rate_limited,
    }
}

/// Decide whether a failed response should be retried, and for how long.
///
/// Returns `None` when the error is not retryable. `Retry-After` is honoured
/// when present, otherwise exponential backoff is used.
pub(crate) fn retry_delay_for(
    status: u16,
    retry_after: Option<&str>,
    attempt: u32,
    rate_limited: bool,
) -> Option<Duration> {
    let retryable = status == 429
        || (status == 403 && rate_limited)
        // 5xx are worth one more try; GitHub has transient gateway failures.
        || (500..600).contains(&status);
    if !retryable {
        return None;
    }
    if let Some(secs) = retry_after.and_then(|s| s.trim().parse::<u64>().ok()) {
        return Some(Duration::from_secs(secs).min(MAX_BACKOFF));
    }
    // 1s, 2s, 4s, 8s, 16s ...
    let exp = Duration::from_millis(1000u64.saturating_mul(1u64 << attempt.min(5)));
    Some(exp.min(MAX_BACKOFF))
}

/// Does this 403 look like a rate limit rather than a permission problem?
fn is_rate_limit_body(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("rate limit") || lower.contains("abuse") || lower.contains("secondary rate")
}

/// Client for one GitHub host and one token.
///
/// Fields are crate-visible so [`crate::graphql`] can reuse the same
/// authenticated, backoff-aware transport rather than duplicating it.
#[derive(Debug, Clone)]
pub struct GithubClient {
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) token: String,
}

impl GithubClient {
    pub fn new(token: impl Into<String>, base_url: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("github-importer/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client builds with static configuration");
        Self {
            http,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            token: token.into(),
        }
    }

    /// Build a client with no token, for requests that do not need one
    /// (device flow). Sending a request through it yields
    /// [`GithubError::Unauthenticated`].
    pub fn anonymous(base_url: Option<String>) -> Self {
        Self::new(String::new(), base_url)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Send a request with retry/backoff, returning the parsed JSON body.
    ///
    /// `path` is appended to the base URL; `body` is sent as JSON when present.
    async fn request_json(
        &self,
        method: reqwest::Method,
        path_or_url: &str,
        body: Option<Value>,
    ) -> Result<Value, GithubError> {
        let text = self.request_text(method, path_or_url, body).await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| GithubError::Other(format!("bad JSON from GitHub: {e}")))
    }

    async fn request_text(
        &self,
        method: reqwest::Method,
        path_or_url: &str,
        body: Option<Value>,
    ) -> Result<String, GithubError> {
        if self.token.is_empty() {
            return Err(GithubError::Unauthenticated);
        }
        let url = self.absolute_url(path_or_url);
        let mut attempt = 0u32;
        loop {
            let mut req = self
                .http
                .request(method.clone(), &url)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .bearer_auth(&self.token);
            if let Some(b) = &body {
                req = req.json(b);
            }

            let resp = req.send().await?;
            let status = resp.status().as_u16();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            if resp.status().is_success() {
                return Ok(resp.text().await?);
            }

            let text = resp.text().await.unwrap_or_default();
            let rate_limited = status == 429 || (status == 403 && is_rate_limit_body(&text));

            if let Some(delay) = retry_delay_for(status, retry_after.as_deref(), attempt, rate_limited) {
                if attempt < MAX_RETRIES {
                    attempt += 1;
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }

            return Err(GithubError::Api {
                status,
                message: error_message_for(status, &text),
                rate_limited,
            });
        }
    }

    /// GET a paginated endpoint, following `Link: rel="next"`.
    ///
    /// `gh` handled this; the REST equivalent must do it by hand. Capped at
    /// [`MAX_PAGES`] so a bad `Link` header cannot loop forever.
    async fn get_paginated<T: for<'de> Deserialize<'de>>(
        &self,
        path_and_query: &str,
    ) -> Result<Vec<T>, GithubError> {
        if self.token.is_empty() {
            return Err(GithubError::Unauthenticated);
        }
        let mut url = self.absolute_url(path_and_query);
        let mut out: Vec<T> = Vec::new();

        for page in 0..MAX_PAGES {
            let mut attempt = 0u32;
            let resp = loop {
                let resp = self
                    .http
                    .get(&url)
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2022-11-28")
                    .bearer_auth(&self.token)
                    .send()
                    .await?;
                if resp.status().is_success() {
                    break resp;
                }
                let status = resp.status().as_u16();
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                let text = resp.text().await.unwrap_or_default();
                let rate_limited = status == 429 || (status == 403 && is_rate_limit_body(&text));
                match retry_delay_for(status, retry_after.as_deref(), attempt, rate_limited) {
                    Some(delay) if attempt < MAX_RETRIES => {
                        attempt += 1;
                        tokio::time::sleep(delay).await;
                    }
                    _ => {
                        return Err(GithubError::Api {
                            status,
                            message: error_message_for(status, &text),
                            rate_limited,
                        })
                    }
                }
            };

            let next = next_link(
                resp.headers()
                    .get("link")
                    .and_then(|v| v.to_str().ok()),
            );
            let text = resp.text().await?;
            if !text.trim().is_empty() {
                let mut chunk: Vec<T> = serde_json::from_str(&text).map_err(|e| {
                    GithubError::Other(format!("bad JSON from GitHub (page {page}): {e}"))
                })?;
                out.append(&mut chunk);
            }
            match next {
                Some(n) => url = n,
                None => return Ok(out),
            }
        }
        Err(GithubError::Other(format!(
            "pagination exceeded {MAX_PAGES} pages"
        )))
    }

    /// Absolute-ise a path, or pass an absolute URL through (Link headers
    /// return absolute URLs).
    fn absolute_url(&self, path_or_url: &str) -> String {
        if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
            path_or_url.to_string()
        } else {
            format!(
                "{}/{}",
                self.base_url.trim_end_matches('/'),
                path_or_url.trim_start_matches('/')
            )
        }
    }

    // ---- Repository -------------------------------------------------------

    /// Port of `gh repo view` — confirms the token can see the repository.
    pub async fn get_repo(&self, repo: &RepoRef) -> Result<Value, GithubError> {
        self.request_json(
            reqwest::Method::GET,
            &format!("/repos/{}/{}", repo.owner, repo.name),
            None,
        )
        .await
    }

    /// The authenticated user. Used to validate a pasted PAT.
    pub async fn get_authenticated_user(&self) -> Result<Value, GithubError> {
        self.request_json(reqwest::Method::GET, "/user", None).await
    }

    /// Scopes attached to the token, from the `x-oauth-scopes` header.
    ///
    /// Classic PATs and OAuth tokens report scopes here; fine-grained tokens
    /// return an empty header, so an empty result is not proof of no scopes.
    ///
    /// This reads a response header rather than a body, so it cannot go through
    /// the shared `request_text` helper. It is also the one call whose failure
    /// is not worth retrying: every caller falls back to "scopes unknown", and
    /// the next real API call reports any actual permission problem.
    pub async fn get_token_scopes(&self) -> Result<Vec<String>, GithubError> {
        if self.token.is_empty() {
            return Err(GithubError::Unauthenticated);
        }
        let url = self.absolute_url("/user");
        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .bearer_auth(&self.token)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(GithubError::Api {
                status,
                message: error_message_for(status, &text),
                rate_limited: false,
            });
        }
        let scopes = resp
            .headers()
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Ok(scopes)
    }

    // ---- Labels -----------------------------------------------------------

    /// Port of `gh label list --limit 1000`.
    pub async fn list_labels(&self, repo: &RepoRef) -> Result<Vec<Label>, GithubError> {
        self.get_paginated(&format!(
            "/repos/{}/{}/labels?per_page=100",
            repo.owner, repo.name
        ))
        .await
    }

    /// Port of `gh label create`.
    ///
    /// `ensure_labels` only creates labels that are missing, so in the normal
    /// flow a 422 here is not a duplicate at all — it is a name GitHub rejects,
    /// which the CLI fails hard on. Only the duplicate case is tolerated, which
    /// covers the one real race: label names are case-insensitive, so a board
    /// holding "Backend" still lists "backend" as missing.
    pub async fn create_label(&self, repo: &RepoRef, name: &str) -> Result<(), GithubError> {
        let result = self
            .request_json(
                reqwest::Method::POST,
                &format!("/repos/{}/{}/labels", repo.owner, repo.name),
                Some(json!({ "name": name })),
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) if e.is_duplicate_label() => Ok(()),
            Err(e) => Err(e),
        }
    }

    // ---- Issues -----------------------------------------------------------

    /// Port of `gh issue list --state all --limit 1000`.
    ///
    /// Pull requests are filtered out: GitHub's `/issues` endpoint includes
    /// them, but `gh issue list` does not. Without this filter a PR whose title
    /// happened to match the task-id regex would be mistaken for an import.
    pub async fn list_issues(&self, repo: &RepoRef) -> Result<Vec<Issue>, GithubError> {
        let all: Vec<Issue> = self
            .get_paginated(&format!(
                "/repos/{}/{}/issues?state=all&per_page=100",
                repo.owner, repo.name
            ))
            .await?;
        Ok(all.into_iter().filter(|i| !i.is_pull_request()).collect())
    }

    /// Fetch one issue, to resolve its database id when it already exists.
    ///
    /// Sub-issue and dependency endpoints take the integer database id, not the
    /// issue number, so a pre-existing issue needs this extra round trip.
    pub async fn get_issue(&self, repo: &RepoRef, number: u64) -> Result<Issue, GithubError> {
        let v = self
            .request_json(
                reqwest::Method::GET,
                &format!("/repos/{}/{}/issues/{}", repo.owner, repo.name, number),
                None,
            )
            .await?;
        serde_json::from_value(v)
            .map_err(|e| GithubError::Other(format!("unexpected issue payload: {e}")))
    }

    /// Port of the create half of `gh issue create`.
    ///
    /// Returns the issue number *and* database id, so the caller can add it to
    /// a project and attach relationships without a second lookup.
    pub async fn create_issue(
        &self,
        repo: &RepoRef,
        title: &str,
        body: &str,
        labels: &[String],
    ) -> Result<CreatedIssue, GithubError> {
        let v = self
            .request_json(
                reqwest::Method::POST,
                &format!("/repos/{}/{}/issues", repo.owner, repo.name),
                Some(json!({ "title": title, "body": body, "labels": labels })),
            )
            .await?;
        let number = v
            .get("number")
            .and_then(|n| n.as_u64())
            .ok_or_else(|| GithubError::Other("created issue has no number".into()))?;
        let id = v
            .get("id")
            .and_then(|n| n.as_u64())
            .ok_or_else(|| GithubError::Other("created issue has no id".into()))?;
        let node_id = v
            .get("node_id")
            .and_then(|n| n.as_str())
            .ok_or_else(|| GithubError::Other("created issue has no node_id".into()))?
            .to_string();
        Ok(CreatedIssue {
            number,
            id,
            node_id,
        })
    }

    // ---- Relationships ----------------------------------------------------

    /// Port of `gh issue edit --parent`.
    ///
    /// `sub_issue_id` takes the child's **database id**.
    pub async fn add_sub_issue(
        &self,
        repo: &RepoRef,
        parent_number: u64,
        child_id: u64,
    ) -> Result<(), GithubError> {
        self.request_json(
            reqwest::Method::POST,
            &format!(
                "/repos/{}/{}/issues/{}/sub_issues",
                repo.owner, repo.name, parent_number
            ),
            Some(json!({ "sub_issue_id": child_id })),
        )
        .await
        .map(|_| ())
    }

    /// Port of `gh issue edit --add-blocked-by`.
    ///
    /// `issue_id` takes the **blocking** issue's database id.
    pub async fn add_blocked_by(
        &self,
        repo: &RepoRef,
        number: u64,
        blocking_issue_id: u64,
    ) -> Result<(), GithubError> {
        self.request_json(
            reqwest::Method::POST,
            &format!(
                "/repos/{}/{}/issues/{}/dependencies/blocked_by",
                repo.owner, repo.name, number
            ),
            Some(json!({ "issue_id": blocking_issue_id })),
        )
        .await
        .map(|_| ())
    }

    /// The repository's node id, needed to create a linked Project.
    pub async fn get_repo_node_id(&self, repo: &RepoRef) -> Result<String, GithubError> {
        let v = self.get_repo(repo).await?;
        v.get("node_id")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| GithubError::Other("repo has no node_id".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_next_link() {
        let header = r#"<https://api.github.com/repos/o/r/issues?page=2>; rel="next", <https://api.github.com/repos/o/r/issues?page=5>; rel="last""#;
        assert_eq!(
            next_link(Some(header)).as_deref(),
            Some("https://api.github.com/repos/o/r/issues?page=2")
        );
    }

    #[test]
    fn next_link_ignores_prev_and_last() {
        let header = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=9>; rel="last""#;
        assert_eq!(next_link(Some(header)), None);
        assert_eq!(next_link(None), None);
        assert_eq!(next_link(Some("")), None);
    }

    #[test]
    fn next_link_handles_a_next_that_is_not_first() {
        let header =
            r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=3>; rel="next""#;
        assert_eq!(
            next_link(Some(header)).as_deref(),
            Some("https://api.github.com/x?page=3")
        );
    }

    #[test]
    fn repo_ref_parses_owner_and_name() {
        assert_eq!(
            RepoRef::parse("PersianRepo/front").unwrap(),
            RepoRef {
                owner: "PersianRepo".into(),
                name: "front".into()
            }
        );
        assert!(RepoRef::parse("nope").is_err());
        assert!(RepoRef::parse("/x").is_err());
        assert!(RepoRef::parse("x/").is_err());
    }

    #[test]
    fn benign_message_matches_the_python_regex() {
        assert!(is_benign_already_set(
            "Validation failed: Target issue has already been taken (addBlockedBy)"
        ));
        assert!(is_benign_already_set("Label already exists"));
        assert!(!is_benign_already_set("Permission denied"));
    }

    #[test]
    fn a_duplicate_label_is_recognised_from_its_error_code() {
        // GitHub's real duplicate-label body. Its only human text is the
        // generic "Validation Failed", and the machine-readable code spells the
        // condition `already_exists` — with an underscore. Matching only the
        // phrase "already exists" misses this, so every duplicate would look
        // like a rejected name and fail the run.
        let body = r#"{"message":"Validation Failed","errors":[{"resource":"Label","code":"already_exists","field":"name"}]}"#;
        let err = GithubError::Api {
            status: 422,
            message: error_message_for(422, body),
            rate_limited: false,
        };
        assert!(err.is_duplicate_label(), "message was: {err}");
        assert!(
            !err.is_already_set(),
            "the phrase match is not what catches this case"
        );
    }

    #[test]
    fn a_rejected_label_name_is_not_mistaken_for_a_duplicate() {
        // `ensure_labels` only creates labels it believes are missing, so this
        // is the far more likely 422 — and it must surface.
        let body = r#"{"message":"Validation Failed","errors":[{"resource":"Label","code":"invalid","field":"name"}]}"#;
        let err = GithubError::Api {
            status: 422,
            message: error_message_for(422, body),
            rate_limited: false,
        };
        assert!(!err.is_duplicate_label(), "message was: {err}");
    }

    #[test]
    fn a_non_422_is_never_a_duplicate_label() {
        let err = GithubError::Api {
            status: 403,
            message: "already_exists".into(),
            rate_limited: false,
        };
        assert!(!err.is_duplicate_label());
    }

    #[test]
    fn retry_delay_honours_retry_after() {
        // 429 with Retry-After: 7 → 7 seconds, not exponential.
        assert_eq!(
            retry_delay_for(429, Some("7"), 0, true),
            Some(Duration::from_secs(7))
        );
    }

    #[test]
    fn retry_delay_caps_a_hostile_retry_after() {
        assert_eq!(
            retry_delay_for(429, Some("99999"), 0, true),
            Some(MAX_BACKOFF)
        );
    }

    #[test]
    fn retry_delay_backs_off_exponentially_without_a_header() {
        assert_eq!(retry_delay_for(429, None, 0, true), Some(Duration::from_secs(1)));
        assert_eq!(retry_delay_for(429, None, 2, true), Some(Duration::from_secs(4)));
    }

    #[test]
    fn retry_delay_does_not_retry_plain_forbidden() {
        // A 403 that is not a rate limit is a permission problem: retrying
        // would just burn the user's time.
        assert_eq!(retry_delay_for(403, None, 0, false), None);
        assert_eq!(retry_delay_for(404, None, 0, false), None);
        assert_eq!(retry_delay_for(422, None, 0, false), None);
    }

    #[test]
    fn retry_delay_retries_rate_limited_forbidden() {
        assert!(retry_delay_for(403, None, 0, true).is_some());
    }

    #[test]
    fn error_message_extracts_validation_detail() {
        let body = r#"{"message":"Validation Failed","errors":[{"resource":"Issue","field":"title","message":"is too long"}]}"#;
        assert_eq!(
            error_message_for(422, body),
            "Validation Failed (is too long)"
        );
    }

    #[test]
    fn error_message_falls_back_to_the_raw_body() {
        assert_eq!(error_message_for(500, "boom"), "boom");
        assert_eq!(error_message_for(500, ""), "HTTP 500");
    }

    #[test]
    fn pull_requests_are_recognised() {
        let pr: Issue = serde_json::from_value(json!({
            "number": 5, "id": 55, "title": "[SETUP-001] Init",
            "pull_request": { "url": "https://api.github.com/x" }
        }))
        .unwrap();
        assert!(pr.is_pull_request());

        let issue: Issue = serde_json::from_value(json!({
            "number": 6, "id": 66, "title": "[SETUP-002] Init"
        }))
        .unwrap();
        assert!(!issue.is_pull_request());
    }
}
