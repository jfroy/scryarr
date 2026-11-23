#![recursion_limit = "256"]
mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::scryer_mock::{libraries_response, library, mock_op};
use http_body_util::BodyExt;
use scryarr::metrics::check_version;
use scryarr::scryer::SCRYER_SCHEMA_VERSION;
use serde_json::json;
use tower::ServiceExt;
use wiremock::MockServer;

// Versions are derived from the vendored schema so that scryer-sync's bump of
// SCRYER_SCHEMA_VERSION does not invert these tests.
fn vendored_major_minor() -> (u64, u64) {
    let mut parts = SCRYER_SCHEMA_VERSION
        .split('.')
        .map(|p| p.parse().expect("numeric version component"));
    (parts.next().unwrap(), parts.next().unwrap())
}

/// The vendored major.minor with a patch scryer has not released.
fn same_minor_version() -> String {
    let (major, minor) = vendored_major_minor();
    format!("{major}.{minor}.999")
}

/// The minor release after the vendored one.
fn next_minor_version() -> String {
    let (major, minor) = vendored_major_minor();
    format!("{major}.{}.0", minor + 1)
}

#[test]
fn version_same_minor_ok() {
    assert!(check_version(&same_minor_version()));
}

#[test]
fn version_new_minor_mismatch() {
    assert!(!check_version(&next_minor_version()));
}

async fn get_text(
    state: &scryarr::AppState,
    uri: &str,
    key: Option<&str>,
) -> (StatusCode, String, Option<String>) {
    let mut req = Request::builder().uri(uri);
    if let Some(k) = key {
        req = req.header("X-Api-Key", k);
    }
    let resp = scryarr::app(state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned(), ct)
}

#[tokio::test]
async fn metrics_endpoint_exposes_counters() {
    let st = common::test_state().await;
    get_text(&st, "/healthz", None).await;
    let (code, body, ct) = get_text(&st, "/metrics", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(ct.as_deref(), Some("text/plain; version=0.0.4"));
    assert!(body.contains("scryarr_http_requests_total"));
    assert!(body.contains(r#"flavor="other""#));
    assert!(body.contains(r#"status="200""#));
}

async fn status_with_version(version: &str) -> String {
    let server = MockServer::start().await;
    mock_op(
        &server,
        "Libraries",
        libraries_response(vec![library("L1", "MOVIE", "Main", &[])]),
    )
    .await;
    mock_op(
        &server,
        "ScryerVersion",
        json!({"data": {"scryerVersion": version}}),
    )
    .await;
    let st = common::test_state_with(&server.uri()).await;
    let (code, _, _) = get_text(&st, "/radarr/L1/api/v3/system/status", Some("k1")).await;
    assert_eq!(code, StatusCode::OK);
    get_text(&st, "/metrics", None).await.1
}

/// The mismatch gauge's sample value. Matching whole lines matters: the
/// gauge's `# HELP` text itself reads "scryarr_scryer_version_mismatch 1 when
/// ...", so a substring search for the sample finds the help line instead.
fn mismatch_gauge(body: &str) -> Option<&str> {
    body.lines()
        .find_map(|l| l.strip_prefix("scryarr_scryer_version_mismatch "))
}

#[tokio::test]
async fn mismatched_version_sets_gauge() {
    let body = status_with_version(&next_minor_version()).await;
    assert_eq!(mismatch_gauge(&body), Some("1"), "{body}");
    assert!(body.contains(r#"flavor="radarr""#));
}

#[tokio::test]
async fn matching_version_clears_gauge() {
    let body = status_with_version(&same_minor_version()).await;
    assert_eq!(mismatch_gauge(&body), Some("0"), "{body}");
}

#[tokio::test]
async fn scryer_errors_are_counted() {
    let server = MockServer::start().await;
    common::scryer_mock::mock_op_status(&server, "Libraries", 401).await;
    let st = common::test_state_with(&server.uri()).await;
    get_text(&st, "/radarr/L1/api/v3/system/status", Some("bad")).await;
    let body = get_text(&st, "/metrics", None).await.1;
    assert!(
        body.contains(r#"scryarr_scryer_errors_total{kind="unauthorized"} 1"#),
        "{body}"
    );
}

#[tokio::test]
async fn forbidden_scryer_errors_are_counted() {
    let server = MockServer::start().await;
    common::scryer_mock::mock_op_status(&server, "Libraries", 403).await;
    let st = common::test_state_with(&server.uri()).await;
    let (code, _, _) = get_text(&st, "/radarr/L1/api/v3/system/status", Some("k")).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let body = get_text(&st, "/metrics", None).await.1;
    assert!(
        body.contains(r#"scryarr_scryer_errors_total{kind="forbidden"} 1"#),
        "{body}"
    );
}
