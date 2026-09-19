//! GitHub Projects V2 over GraphQL.
//!
//! Projects V2 has no REST equivalent, so this is the only part of the API
//! surface that needs GraphQL. Everything else goes through [`crate::github`].

use crate::github::{error_from_graphql_body, GithubClient, GithubError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One Projects V2 board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// The GraphQL global id — `projectId` on `addProjectV2ItemById`.
    pub id: String,
    pub title: String,
    pub number: u64,
    /// Which of the two owner types answered, for the UI's diagnostics.
    pub owner_kind: String,
}

/// The wire shape of a `projectsV2` connection.
#[derive(Debug, Deserialize)]
struct ProjectsPage {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Vec<ProjectNode>,
}

#[derive(Debug, Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProjectNode {
    id: String,
    title: String,
    number: u64,
}

#[derive(Debug, Deserialize)]
struct OwnerProjects {
    #[serde(rename = "projectsV2")]
    projects_v2: ProjectsPage,
}

/// One query fetches both owner types; whichever is non-null answers.
///
/// `config.json` allows `project_owner` to be a user *or* an organisation, and
/// `gh project list --owner` resolves both. `read:org` may be required to see
/// an organisation's boards — that is surfaced as a permissions error rather
/// than silently reported as "not found".
const PROJECTS_QUERY: &str = r#"
query($login: String!, $cursor: String) {
  user(login: $login) {
    projectsV2(first: 100, after: $cursor) {
      pageInfo { hasNextPage endCursor }
      nodes { id title number }
    }
  }
  organization(login: $login) {
    projectsV2(first: 100, after: $cursor) {
      pageInfo { hasNextPage endCursor }
      nodes { id title number }
    }
  }
}"#;

const ADD_ITEM_MUTATION: &str = r#"
mutation($projectId: ID!, $contentId: ID!) {
  addProjectV2ItemById(input: { projectId: $projectId, contentId: $contentId }) {
    item { id }
  }
}"#;

impl GithubClient {
    /// POST a GraphQL document, applying the same retry policy as the REST
    /// client.
    ///
    /// GraphQL answers with HTTP 200 and an `errors` array, so partial failure
    /// has to be detected in the body rather than from the status code.
    pub(crate) async fn graphql(&self, query: &str, variables: Value) -> Result<Value, GithubError> {
        if self.token.is_empty() {
            return Err(GithubError::Unauthenticated);
        }
        let url = format!("{}/graphql", self.base_url.trim_end_matches('/'));
        let payload = json!({ "query": query, "variables": variables });

        let mut attempt = 0u32;
        loop {
            let resp = self
                .http
                .post(&url)
                .header("Accept", "application/vnd.github+json")
                .bearer_auth(&self.token)
                .json(&payload)
                .send()
                .await?;

            let status = resp.status().as_u16();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let text = resp.text().await.unwrap_or_default();

            if status == 200 {
                let parsed: Value = serde_json::from_str(&text)
                    .map_err(|e| GithubError::Other(format!("bad GraphQL JSON: {e}")))?;
                if let Some(errors) = parsed.get("errors").and_then(|e| e.as_array()) {
                    if !errors.is_empty() {
                        return Err(error_from_graphql_body(errors, &parsed));
                    }
                }
                return parsed.get("data").cloned().ok_or_else(|| {
                    GithubError::Other("GraphQL response has no data".to_string())
                });
            }

            let rate_limited = status == 429
                || (status == 403 && text.to_lowercase().contains("rate limit"));
            if let Some(delay) = crate::github::retry_delay_for(status, retry_after.as_deref(), attempt, rate_limited)
            {
                if attempt < crate::github::MAX_RETRIES {
                    attempt += 1;
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }
            return Err(GithubError::Api {
                status,
                message: crate::github::error_message_for(status, &text),
                rate_limited,
            });
        }
    }

    /// Port of `gh project list --owner <login> --limit 1000`.
    ///
    /// Pages through the connection: a user or org with more than 100 boards
    /// would otherwise appear to have none of the later ones.
    pub async fn list_projects(&self, login: &str) -> Result<Vec<Project>, GithubError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;

        for _ in 0..crate::github::MAX_PAGES {
            let vars = json!({ "login": login, "cursor": cursor });
            let data = self.graphql(PROJECTS_QUERY, vars).await?;

            // At most one of these is non-null; a login is either a user or an
            // organisation, never both.
            let (owner, kind) = match (
                data.get("user").filter(|v| !v.is_null()),
                data.get("organization").filter(|v| !v.is_null()),
            ) {
                (Some(u), _) => (u, "user"),
                (None, Some(o)) => (o, "organization"),
                (None, None) => {
                    return Err(GithubError::Other(format!(
                        "no user or organization named '{login}'"
                    )))
                }
            };

            let page: OwnerProjects = serde_json::from_value(
                json!({ "projectsV2": owner.get("projectsV2").cloned().unwrap_or(Value::Null) }),
            )
            .map_err(|e| GithubError::Other(format!("unexpected projects payload: {e}")))?;

            for n in page.projects_v2.nodes {
                out.push(Project {
                    id: n.id,
                    title: n.title,
                    number: n.number,
                    owner_kind: kind.to_string(),
                });
            }

            if page.projects_v2.page_info.has_next_page {
                cursor = page.projects_v2.page_info.end_cursor;
                if cursor.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(out)
    }

    /// Resolve a board by exact title, the way `project_check` does.
    ///
    /// The error message matches the Python tool's so the UI can show the same
    /// guidance the CLI did.
    pub async fn find_project(&self, owner: &str, name: &str) -> Result<Project, GithubError> {
        let projects = self.list_projects(owner).await?;
        match projects.iter().find(|p| p.title == name) {
            Some(p) => Ok(p.clone()),
            None => {
                let titles: Vec<String> = projects.into_iter().map(|p| p.title).collect();
                Err(GithubError::Other(crate::tasks::project_not_found_message(
                    owner, name, &titles,
                )))
            }
        }
    }

    /// Port of `gh issue create --project`: add an existing issue to a board.
    ///
    /// `content_id` is the issue's GraphQL **node id**, not its number or
    /// database id.
    pub async fn add_project_item(
        &self,
        project_id: &str,
        content_id: &str,
    ) -> Result<String, GithubError> {
        let data = self
            .graphql(
                ADD_ITEM_MUTATION,
                json!({ "projectId": project_id, "contentId": content_id }),
            )
            .await?;
        data.get("addProjectV2ItemById")
            .and_then(|v| v.get("item"))
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| GithubError::Other("addProjectV2ItemById returned no item".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_query_requests_both_owner_types() {
        // The whole point of the combined query: `project_owner` may be either.
        assert!(PROJECTS_QUERY.contains("user(login: $login)"));
        assert!(PROJECTS_QUERY.contains("organization(login: $login)"));
        assert!(PROJECTS_QUERY.contains("pageInfo"));
    }

    #[test]
    fn add_item_mutation_uses_the_documented_input() {
        assert!(ADD_ITEM_MUTATION.contains("addProjectV2ItemById(input: { projectId: $projectId, contentId: $contentId })"));
    }

    #[test]
    fn owner_projects_shape_deserialises() {
        let raw = json!({
            "projectsV2": {
                "pageInfo": { "hasNextPage": false, "endCursor": null },
                "nodes": [
                    { "id": "PVT_1", "title": "Front", "number": 1 },
                    { "id": "PVT_2", "title": "API", "number": 2 }
                ]
            }
        });
        let page: OwnerProjects = serde_json::from_value(raw).unwrap();
        assert_eq!(page.projects_v2.nodes.len(), 2);
        assert_eq!(page.projects_v2.nodes[0].title, "Front");
        assert!(!page.projects_v2.page_info.has_next_page);
    }
}
