//! HTTP-level tests against a mock GitHub API.
//!
//! The unit tests in `src/github.rs` cover the pure helpers (`next_link`,
//! `retry_delay_for`, message extraction). These cover the parts that only
//! exist as behaviour over the wire: following `Link` pagination, retrying the
//! right failures and not the wrong ones, and the two response shapes the CLI
//! leans on — a 422 for an existing label and the `/issues` endpoint returning
//! pull requests that `gh issue list` would have hidden.

use github_importer_lib::github::{GithubClient, RepoRef};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn repo() -> RepoRef {
    RepoRef {
        owner: "acme".into(),
        name: "widgets".into(),
    }
}

fn client(server: &MockServer) -> GithubClient {
    GithubClient::new("ghp_test_token", Some(server.uri()))
}

/// An issue payload with only the fields `Issue` requires.
fn issue(number: u64, title: &str) -> serde_json::Value {
    json!({
        "number": number,
        "id": 10_000 + number,
        "node_id": format!("I_node{number}"),
        "title": title,
        "body": "body",
    })
}

/// A pull request as `/issues` reports it: issue fields plus `pull_request`.
fn pull_request(number: u64, title: &str) -> serde_json::Value {
    let mut v = issue(number, title);
    v["pull_request"] = json!({ "url": "https://api.github.com/repos/acme/widgets/pulls/1" });
    v
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn follows_link_next_across_pages() {
    let server = MockServer::start().await;

    // The page-2 and page-3 mocks are mounted with the highest priority. Without
    // it, insertion order would let the page-1 mock (which must not require a
    // `page` parameter, since the first request carries none) answer for them
    // too, and the walk would never advance.
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "link",
                    format!(
                        r#"<{}/repos/acme/widgets/issues?state=all&per_page=100&page=3>; rel="next""#,
                        server.uri()
                    )
                    .as_str(),
                )
                .set_body_json(json!([issue(3, "[A-003] three")])),
        )
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues"))
        .and(query_param("page", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([issue(4, "[A-004] four")])))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;

    // Page 1: no `page` parameter, and the only page that advertises a `last`.
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "link",
                    format!(
                        r#"<{}/repos/acme/widgets/issues?state=all&per_page=100&page=2>; rel="next", <{}/repos/acme/widgets/issues?state=all&per_page=100&page=3>; rel="last""#,
                        server.uri(),
                        server.uri()
                    )
                    .as_str(),
                )
                .set_body_json(json!([issue(1, "[A-001] one"), issue(2, "[A-002] two")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let issues = client(&server).list_issues(&repo()).await.unwrap();

    let titles: Vec<&str> = issues.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["[A-001] one", "[A-002] two", "[A-003] three", "[A-004] four"]
    );
    // Each page's `expect(1)` asserts it was fetched exactly once, so a walk
    // that re-requested page 1 or stopped early fails here.
}

#[tokio::test]
async fn the_first_request_carries_the_query_the_endpoint_needs() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues"))
        .and(query_param("state", "all"))
        .and(query_param("per_page", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;

    let issues = client(&server).list_issues(&repo()).await.unwrap();
    assert!(issues.is_empty());
}

#[tokio::test]
async fn requires_a_bearer_token_and_api_version_header() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/labels"))
        .and(header("authorization", "Bearer ghp_test_token"))
        .and(header("x-github-api-version", "2022-11-28"))
        .and(header("accept", "application/vnd.github+json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;

    client(&server).list_labels(&repo()).await.unwrap();
}

// ---------------------------------------------------------------------------
// Pull requests are not issues
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pull_requests_returned_by_the_issues_endpoint_are_filtered_out() {
    let server = MockServer::start().await;

    // A PR titled like a managed issue is exactly the case that would make the
    // importer skip a real task, or link a relationship to the wrong number.
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            issue(1, "[A-001] a real issue"),
            pull_request(2, "[A-002] a pull request that looks like one"),
            issue(3, "[A-003] another real issue"),
            pull_request(4, "[A-004] another PR"),
        ])))
        .mount(&server)
        .await;

    let issues = client(&server).list_issues(&repo()).await.unwrap();

    let numbers: Vec<u64> = issues.iter().map(|i| i.number).collect();
    assert_eq!(numbers, vec![1, 3]);
}

#[tokio::test]
async fn a_pull_request_is_recognised_by_the_presence_of_its_field() {
    // `get_issue` does not filter, so the distinction has to be available.
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull_request(7, "[A-007] a PR")))
        .mount(&server)
        .await;

    let found = client(&server).get_issue(&repo(), 7).await.unwrap();
    assert!(found.is_pull_request());

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues/8"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue(8, "[A-008] an issue")))
        .mount(&server)
        .await;

    let found = client(&server).get_issue(&repo(), 8).await.unwrap();
    assert!(!found.is_pull_request());
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_422_when_creating_a_label_means_it_already_exists_and_is_success() {
    let server = MockServer::start().await;

    // The exact body GitHub returns for a duplicate label.
    Mock::given(method("POST"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Validation Failed",
            "errors": [{ "resource": "Label", "code": "already_exists", "field": "name" }],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let result = client(&server).create_label(&repo(), "backend").await;
    assert!(
        result.is_ok(),
        "an existing label must not fail the run: {result:?}"
    );
}

#[tokio::test]
async fn a_422_for_another_reason_is_still_an_error() {
    let server = MockServer::start().await;

    // `ensure_labels` only posts labels it believes are missing, so a rejected
    // name is a real failure — the Python CLI exits non-zero on it. Treating
    // every 422 as "already exists" would report a label as created when the
    // board never got it.
    Mock::given(method("POST"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Validation Failed",
            "errors": [{ "resource": "Label", "code": "invalid", "field": "name" }],
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .create_label(&repo(), "bad name")
        .await
        .unwrap_err();
    assert!(err.is_validation_failed());
    assert!(!err.is_duplicate_label(), "only already_exists is benign");
    assert!(!err.is_already_set());
}

#[tokio::test]
async fn a_duplicate_label_is_tolerated_by_create_label() {
    // The only case `create_label` swallows. Label names are case-insensitive,
    // so a board holding "Backend" still reports "backend" as missing, and the
    // create that follows is a 422. The predicate itself, including why a plain
    // phrase match is not enough, is covered by unit tests in `src/github.rs`.
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Validation Failed",
            "errors": [{ "resource": "Label", "code": "already_exists", "field": "name" }],
        })))
        .expect(1)
        .mount(&server)
        .await;

    assert!(client(&server).create_label(&repo(), "backend").await.is_ok());
}

#[tokio::test]
async fn a_created_label_sends_its_name_as_json() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/repos/acme/widgets/labels"))
        .and(header("content-type", "application/json"))
        .and(wiremock::matchers::body_json(json!({ "name": "P0" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "name": "P0" })))
        .expect(1)
        .mount(&server)
        .await;

    client(&server).create_label(&repo(), "P0").await.unwrap();
}

// ---------------------------------------------------------------------------
// Retry and backoff
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_429_with_retry_after_is_retried_and_then_succeeds() {
    let server = MockServer::start().await;

    // `Retry-After: 1` keeps the test's wall clock to a second. The assertion on
    // request counts is what proves the backoff happened rather than the call
    // simply being repeated.
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_json(json!({ "message": "API rate limit exceeded" })),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "name": "backend" }])))
        .expect(1)
        .mount(&server)
        .await;

    let started = std::time::Instant::now();
    let labels = client(&server).list_labels(&repo()).await.unwrap();

    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].name, "backend");
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "the Retry-After delay was not honoured"
    );
}

#[tokio::test]
async fn a_secondary_rate_limit_reported_as_403_is_retried() {
    let server = MockServer::start().await;

    // GitHub reports secondary limits as 403 with a body, not 429. Treating
    // every 403 as fatal would abort a run that only needed to wait.
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("retry-after", "1")
                .set_body_json(json!({ "message": "You have exceeded a secondary rate limit" })),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;

    client(&server).list_labels(&repo()).await.unwrap();
}

#[tokio::test]
async fn a_forbidden_that_is_not_a_rate_limit_is_not_retried() {
    let server = MockServer::start().await;

    // A genuine permission problem must fail immediately: retrying it five
    // times would stall the run for half a minute and still fail.
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": "Resource not accessible by personal access token",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let err = client(&server).list_labels(&repo()).await.unwrap_err();
    assert!(err.is_forbidden());
    assert!(!err.is_validation_failed());
}

#[tokio::test]
async fn a_404_is_not_retried() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues/999"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })))
        .expect(1)
        .mount(&server)
        .await;

    let err = client(&server).get_issue(&repo(), 999).await.unwrap_err();
    assert!(err.is_not_found());
    assert!(err.to_string().contains("Not Found"));
}

#[tokio::test]
async fn a_422_is_not_retried() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/repos/acme/widgets/issues"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Validation Failed",
            "errors": [{ "message": "title is too long" }],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let err = client(&server)
        .create_issue(
            &repo(),
            "[A-001] a title",
            "a body",
            &["backend".to_string()],
        )
        .await
        .unwrap_err();

    assert!(err.is_validation_failed());
    // The nested detail is what tells the user which field GitHub rejected.
    assert!(
        err.to_string().contains("title is too long"),
        "message was: {err}"
    );
}

#[tokio::test]
async fn a_transient_server_error_is_retried() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(
            ResponseTemplate::new(502)
                .insert_header("retry-after", "1")
                .set_body_json(json!({ "message": "Bad Gateway" })),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "name": "backend" }])))
        .expect(1)
        .mount(&server)
        .await;

    let labels = client(&server).list_labels(&repo()).await.unwrap();
    assert_eq!(labels.len(), 1);
}

#[tokio::test]
async fn retries_are_capped_so_a_persistent_failure_gives_up() {
    let server = MockServer::start().await;

    // GitHub is down and stays down. `Retry-After: 1` keeps each backoff to a
    // second so the test is quick, while still exercising the give-up path.
    // One initial attempt plus MAX_RETRIES retries, then the error surfaces.
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/labels"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_json(json!({ "message": "API rate limit exceeded" })),
        )
        .mount(&server)
        .await;

    let err = client(&server).list_labels(&repo()).await.unwrap_err();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        6,
        "expected the initial attempt plus 5 retries"
    );
    assert!(matches!(
        err,
        github_importer_lib::github::GithubError::Api {
            status: 429,
            rate_limited: true,
            ..
        }
    ));
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_anonymous_client_refuses_to_send_anything() {
    let server = MockServer::start().await;

    // No mock is mounted: if the client sent a request the server would 404 and
    // the assertion below would see a different error.
    let anonymous = GithubClient::anonymous(Some(server.uri()));
    let err = anonymous.list_labels(&repo()).await.unwrap_err();

    assert!(matches!(
        err,
        github_importer_lib::github::GithubError::Unauthenticated
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn token_scopes_come_from_the_response_header() {
    let server = MockServer::start().await;

    // Both accessors hit `/user`; scopes ride on a header, the profile on the body.
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-oauth-scopes", "repo, project, read:org, workflow")
                .set_body_json(json!({ "login": "octocat", "name": "The Octocat" })),
        )
        .expect(2)
        .mount(&server)
        .await;

    let client = client(&server);
    let scopes = client.get_token_scopes().await.unwrap();
    assert_eq!(scopes, vec!["repo", "project", "read:org", "workflow"]);

    let user = client.get_authenticated_user().await.unwrap();
    assert_eq!(user["login"], "octocat");
}

#[tokio::test]
async fn a_fine_grained_token_reporting_no_scopes_yields_an_empty_list() {
    let server = MockServer::start().await;

    // Fine-grained tokens send no x-oauth-scopes header at all. This must not
    // be an error, and it must not read as "all scopes are missing".
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "octocat" })))
        .mount(&server)
        .await;

    let scopes = client(&server).get_token_scopes().await.unwrap();
    assert!(scopes.is_empty());
}

// ---------------------------------------------------------------------------
// Repository node id, needed before any GraphQL call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_repository_node_id_is_read_from_the_rest_payload() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "name": "widgets", "node_id": "R_kgDOABCDEF" })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let node_id = client(&server).get_repo_node_id(&repo()).await.unwrap();
    assert_eq!(node_id, "R_kgDOABCDEF");
}
