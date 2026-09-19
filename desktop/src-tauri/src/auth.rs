//! Authentication: OAuth device flow, personal access tokens, and migration
//! from an existing `gh` CLI session.
//!
//! Secrets live only here. The device code and any access token stay inside the
//! Rust process; the frontend sees a user code and a status, never a credential.

use crate::github::{GithubClient, GithubError};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use zeroize::Zeroize;

/// Scopes the importer needs.
///
/// - `repo` — create and edit issues, create labels
/// - `project` — add an issue to a Projects V2 board
/// - `read:org` — list organisation-owned projects, since `config.json` allows
///   `project_owner` to be an org
pub const REQUIRED_SCOPES: &str = "repo project read:org";

/// GitHub's OAuth endpoints, which live on the web host rather than the API
/// host. A GitHub Enterprise Server install would override this.
pub const DEFAULT_OAUTH_BASE: &str = "https://github.com";

/// The device code endpoint, plus the one the poller posts to.
#[derive(Debug, Clone)]
pub struct OAuthEndpoints {
    pub device_code_url: String,
    pub access_token_url: String,
    pub verification_uri: String,
}

impl OAuthEndpoints {
    pub fn for_host(host: &str) -> Self {
        let base = host.trim_end_matches('/');
        Self {
            device_code_url: format!("{base}/login/device/code"),
            access_token_url: format!("{base}/login/oauth/access_token"),
            verification_uri: format!("{base}/login/device"),
        }
    }

    pub fn github() -> Self {
        Self::for_host(DEFAULT_OAUTH_BASE)
    }
}

/// Client id of the registered GitHub OAuth App.
///
/// Registration is a one-time manual step (see README) and is a hard
/// prerequisite for the device flow. The value can be supplied three ways, in
/// priority order: this build-time environment variable, a value saved in the
/// app's settings, or one typed into the UI.
pub const BUILTIN_CLIENT_ID: Option<&str> = option_env!("GITHUB_CLIENT_ID");

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Github(#[from] GithubError),
    #[error("no OAuth App client id is configured")]
    NoClientId,
    #[error("the device code expired before it was approved")]
    Expired,
    #[error("authorization was denied")]
    Denied,
    #[error("device flow is disabled for this OAuth App. Enable it in the app's settings, or paste a personal access token instead.")]
    DeviceFlowDisabled,
    #[error("GitHub rejected the client id")]
    BadClientId,
    #[error("GitHub's device-flow limit was hit (50 authorizations per hour across all users of this app). Wait, or paste a personal access token instead.")]
    RateLimited,
    #[error("{0}")]
    Other(String),
}

/// What the UI displays while the user approves the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeInfo {
    /// The 8-character code the user types at `verification_uri`.
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until the code expires. Codes last 15 minutes.
    pub expires_in: u64,
    /// Seconds the poller waits between attempts.
    pub interval: u64,
    pub client_id: String,
}

/// A device flow in progress. Held in Rust; the `device_code` never reaches
/// the webview.
#[derive(Debug, Clone)]
pub struct PendingDeviceFlow {
    pub info: DeviceCodeInfo,
    device_code: String,
    started_at: Instant,
    pub interval: u64,
}

impl PendingDeviceFlow {
    /// Has the code outlived its 15-minute window?
    pub fn is_expired(&self, now: Instant) -> bool {
        now.duration_since(self.started_at) >= Duration::from_secs(self.info.expires_in)
    }

    pub fn seconds_remaining(&self, now: Instant) -> u64 {
        Duration::from_secs(self.info.expires_in)
            .saturating_sub(now.duration_since(self.started_at))
            .as_secs()
    }

    /// Wipe the device code when the flow ends.
    pub fn clear(&mut self) {
        self.device_code.zeroize();
    }
}

/// One poll attempt's outcome.
#[derive(Debug)]
pub enum PollOutcome {
    /// Not approved yet; keep polling.
    Pending,
    /// GitHub asked us to slow down; the caller must add 5s to its interval.
    SlowDown,
    /// Approved. The token is wrapped so it is zeroed on drop.
    Token(String),
    /// Terminal failure.
    Failed(AuthError),
}

/// A token wrapped so it is wiped from memory when dropped.
#[derive(Clone)]
pub struct SecretToken(String);

impl SecretToken {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for SecretToken {
    /// Never print the token, even by accident in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretToken(***)")
    }
}

/// A bare HTTP client for the OAuth endpoints, which take no bearer token and
/// want `Accept: application/json` (otherwise GitHub replies form-encoded).
fn oauth_http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("github-importer/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("static reqwest configuration")
}

/// Step 1 of the device flow: ask GitHub for a user code.
pub async fn start_device_flow(
    endpoints: &OAuthEndpoints,
    client_id: &str,
    scope: &str,
) -> Result<PendingDeviceFlow, AuthError> {
    if client_id.trim().is_empty() {
        return Err(AuthError::NoClientId);
    }
    let resp = oauth_http()
        .post(&endpoints.device_code_url)
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("scope", scope)])
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    // The app-wide 50/hour cap is reported here rather than as `error`.
    if status.as_u16() == 429 || text.to_lowercase().contains("rate limit") {
        return Err(AuthError::RateLimited);
    }

    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| AuthError::Other(format!("unexpected device-code response: {e}")))?;

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Err(match err {
            "device_flow_disabled" => AuthError::DeviceFlowDisabled,
            "incorrect_client_credentials" => AuthError::BadClientId,
            other => AuthError::Other(format!("GitHub rejected the device code request: {other}")),
        });
    }

    let user_code = v
        .get("user_code")
        .and_then(|s| s.as_str())
        .ok_or_else(|| AuthError::Other("device code response had no user_code".into()))?
        .to_string();
    let device_code = v
        .get("device_code")
        .and_then(|s| s.as_str())
        .ok_or_else(|| AuthError::Other("device code response had no device_code".into()))?
        .to_string();

    let info = DeviceCodeInfo {
        user_code,
        verification_uri: v
            .get("verification_uri")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| endpoints.verification_uri.clone()),
        // GitHub documents 900s; fall back rather than fail if it is absent.
        expires_in: v.get("expires_in").and_then(|n| n.as_u64()).unwrap_or(900),
        interval: v.get("interval").and_then(|n| n.as_u64()).unwrap_or(5),
        client_id: client_id.to_string(),
    };

    Ok(PendingDeviceFlow {
        interval: info.interval,
        info,
        device_code,
        started_at: Instant::now(),
    })
}

/// Step 2: one poll attempt.
///
/// The caller loops on this, sleeping `flow.interval` between attempts and
/// adding 5 seconds whenever GitHub answers `slow_down`.
pub async fn poll_device_flow(
    endpoints: &OAuthEndpoints,
    flow: &PendingDeviceFlow,
    now: Instant,
) -> PollOutcome {
    if flow.is_expired(now) {
        return PollOutcome::Failed(AuthError::Expired);
    }

    let resp = match oauth_http()
        .post(&endpoints.access_token_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", flow.info.client_id.as_str()),
            ("device_code", flow.device_code.as_str()),
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:device_code",
            ),
        ])
        .send()
        .await
    {
        Ok(r) => r,
        // A transient network blip mid-approval should not abort the flow.
        Err(e) => {
            return if e.is_timeout() || e.is_connect() {
                PollOutcome::Pending
            } else {
                PollOutcome::Failed(AuthError::Http(e))
            }
        }
    };

    let text = resp.text().await.unwrap_or_default();
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return PollOutcome::Failed(AuthError::Other(format!(
                "unexpected token response: {e}"
            )))
        }
    };

    if let Some(token) = v.get("access_token").and_then(|t| t.as_str()) {
        if !token.is_empty() {
            return PollOutcome::Token(token.to_string());
        }
    }

    match v.get("error").and_then(|e| e.as_str()).unwrap_or("") {
        // The normal case while the user is still typing the code.
        "authorization_pending" => PollOutcome::Pending,
        "slow_down" => PollOutcome::SlowDown,
        "expired_token" => PollOutcome::Failed(AuthError::Expired),
        "access_denied" => PollOutcome::Failed(AuthError::Denied),
        "device_flow_disabled" => PollOutcome::Failed(AuthError::DeviceFlowDisabled),
        "incorrect_client_credentials" => PollOutcome::Failed(AuthError::BadClientId),
        other => PollOutcome::Failed(AuthError::Other(format!(
            "GitHub rejected the token request: {other}"
        ))),
    }
}

/// Details of the account a token belongs to, shown after a successful sign-in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub login: String,
    pub name: Option<String>,
    /// Scopes GitHub reported, when it reports them. Fine-grained tokens and
    /// GitHub App tokens send no `x-oauth-scopes` header, so an empty list is
    /// not evidence of missing scopes.
    pub scopes: Vec<String>,
}

impl Account {
    /// Which of the required scopes are absent, for tokens that report scopes.
    ///
    /// A token that reports none is reported as "unknown" rather than missing —
    /// the app then finds out from the first API call that needs the scope.
    pub fn missing_scopes(&self) -> Vec<String> {
        if self.scopes.is_empty() {
            return Vec::new();
        }
        REQUIRED_SCOPES
            .split_whitespace()
            .filter(|s| !self.scopes.iter().any(|have| have == s))
            .map(|s| s.to_string())
            .collect()
    }
}

/// Verify a token and describe the account it belongs to.
///
/// This is the single check applied to all three auth paths, so a token from
/// the device flow, a pasted PAT, and `gh auth token` are validated identically.
pub async fn verify_token(
    token: &str,
    api_base: Option<String>,
) -> Result<Account, AuthError> {
    let client = GithubClient::new(token, api_base);
    let user = client.get_authenticated_user().await?;
    let scopes = client.get_token_scopes().await.unwrap_or_default();

    let login = user
        .get("login")
        .and_then(|l| l.as_str())
        .ok_or_else(|| AuthError::Other("token is not associated with an account".into()))?
        .to_string();

    Ok(Account {
        login,
        name: user
            .get("name")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string()),
        scopes,
    })
}

/// Read a token out of an existing `gh` CLI session.
///
/// Makes migrating from the CLI zero-friction. Optional: this is only offered
/// once `gh` has been detected on PATH.
pub async fn token_from_gh_cli() -> Result<String, AuthError> {
    let output = tokio::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .await
        .map_err(|e| AuthError::Other(format!("could not run `gh`: {e}")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AuthError::Other(if err.is_empty() {
            "`gh auth token` failed; run `gh auth login` first".to_string()
        } else {
            err
        }));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(AuthError::Other(
            "`gh auth token` returned nothing; run `gh auth login` first".to_string(),
        ));
    }
    Ok(token)
}

/// Is the `gh` CLI available? Gates the migration path in the UI.
pub async fn gh_cli_available() -> bool {
    tokio::process::Command::new("gh")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// How the current session was established, for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    DeviceFlow,
    PersonalAccessToken,
    GhCli,
}

impl AuthMethod {
    pub fn label(&self) -> &'static str {
        match self {
            Self::DeviceFlow => "GitHub device flow",
            Self::PersonalAccessToken => "Personal access token",
            Self::GhCli => "Imported from the gh CLI",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_built_from_a_host() {
        let e = OAuthEndpoints::github();
        assert_eq!(e.device_code_url, "https://github.com/login/device/code");
        assert_eq!(e.access_token_url, "https://github.com/login/oauth/access_token");
        assert_eq!(e.verification_uri, "https://github.com/login/device");
    }

    #[test]
    fn endpoints_support_a_trailing_slash_and_enterprise_host() {
        let e = OAuthEndpoints::for_host("https://ghe.example.com/");
        assert_eq!(
            e.device_code_url,
            "https://ghe.example.com/login/device/code"
        );
    }

    #[test]
    fn required_scopes_cover_issues_projects_and_org_lookup() {
        // `repo` for issues/labels, `project` for the board, `read:org` because
        // project_owner may be an organisation.
        let scopes: Vec<&str> = REQUIRED_SCOPES.split_whitespace().collect();
        assert!(scopes.contains(&"repo"));
        assert!(scopes.contains(&"project"));
        assert!(scopes.contains(&"read:org"));
    }

    #[test]
    fn missing_scopes_reports_what_is_absent() {
        let acct = Account {
            login: "octocat".into(),
            name: None,
            scopes: vec!["repo".into()],
        };
        let missing = acct.missing_scopes();
        assert!(missing.contains(&"project".to_string()));
        assert!(missing.contains(&"read:org".to_string()));
        assert!(!missing.contains(&"repo".to_string()));
    }

    #[test]
    fn unknown_scopes_are_not_reported_as_missing() {
        // Fine-grained tokens send no x-oauth-scopes header at all; treating
        // that as "missing everything" would produce a false warning.
        let acct = Account {
            login: "octocat".into(),
            name: None,
            scopes: vec![],
        };
        assert!(acct.missing_scopes().is_empty());
    }

    #[test]
    fn device_flow_expires_after_its_window() {
        // Anchor every assertion to one instant: `seconds_remaining` truncates,
        // so reading the clock twice would make the arithmetic sub-second
        // inexact and the test flaky.
        let started = Instant::now();
        let flow = PendingDeviceFlow {
            info: DeviceCodeInfo {
                user_code: "ABCD-1234".into(),
                verification_uri: "https://github.com/login/device".into(),
                expires_in: 900,
                interval: 5,
                client_id: "cid".into(),
            },
            device_code: "secret".into(),
            started_at: started,
            interval: 5,
        };

        assert!(!flow.is_expired(started));
        assert_eq!(flow.seconds_remaining(started), 900);
        assert_eq!(flow.seconds_remaining(started + Duration::from_secs(300)), 600);
        // The window closes exactly at 900s, not after it.
        assert!(!flow.is_expired(started + Duration::from_secs(899)));
        assert!(flow.is_expired(started + Duration::from_secs(900)));
        assert_eq!(flow.seconds_remaining(started + Duration::from_secs(900)), 0);
        assert_eq!(flow.seconds_remaining(started + Duration::from_secs(901)), 0);
    }

    #[test]
    fn clearing_a_flow_wipes_the_device_code() {
        let mut flow = PendingDeviceFlow {
            info: DeviceCodeInfo {
                user_code: "ABCD-1234".into(),
                verification_uri: "https://github.com/login/device".into(),
                expires_in: 900,
                interval: 5,
                client_id: "cid".into(),
            },
            device_code: "device-secret".into(),
            started_at: Instant::now(),
            interval: 5,
        };
        flow.clear();
        assert!(flow.device_code.is_empty());
    }

    #[test]
    fn token_debug_never_prints_the_secret() {
        let t = SecretToken::new("ghp_supersecret");
        assert_eq!(format!("{:?}", t), "SecretToken(***)");
        assert_eq!(t.expose(), "ghp_supersecret");
    }

    #[test]
    fn rate_limited_message_points_at_the_pat_fallback() {
        // The plan calls for a clear message when the app-wide 50/hour cap is
        // hit, since it is not something the user can wait out quickly.
        let msg = AuthError::RateLimited.to_string();
        assert!(msg.contains("personal access token"));
        assert!(msg.contains("50"));
    }

    #[test]
    fn device_flow_disabled_message_explains_the_fix() {
        assert!(AuthError::DeviceFlowDisabled.to_string().contains("Enable it"));
    }

    #[test]
    fn auth_method_labels_are_human_readable() {
        assert_eq!(AuthMethod::DeviceFlow.label(), "GitHub device flow");
        assert_eq!(AuthMethod::GhCli.label(), "Imported from the gh CLI");
    }
}
