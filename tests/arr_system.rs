#![recursion_limit = "256"]
mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::scryer_mock::{libraries_response, library, mock_op, mock_op_status};
use http_body_util::BodyExt;
use scryarr::AppState;
use scryarr::domain::Facet;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::MockServer;

async fn get(state: &AppState, uri: &str, key: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder().uri(uri);
    if let Some(k) = key {
        req = req.header("X-Api-Key", k);
    }
    let resp = scryarr::app(state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn setup(facet: &str, roots: &[&str]) -> (MockServer, AppState) {
    let server = MockServer::start().await;
    mock_op(
        &server,
        "Libraries",
        libraries_response(vec![library("L1", facet, "Main", roots)]),
    )
    .await;
    mock_op(
        &server,
        "ScryerVersion",
        json!({"data": {"scryerVersion": "0.22.4"}}),
    )
    .await;
    let state = common::test_state_with(&server.uri()).await;
    (server, state)
}

#[tokio::test]
async fn status_reports_radarr_v5() {
    let (_s, st) = setup("MOVIE", &[]).await;
    let (code, body) = get(&st, "/radarr/L1/api/v3/system/status", Some("k1")).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["appName"], "Radarr");
    assert!(body["version"].as_str().unwrap().starts_with("5."));
    assert_eq!(body["instanceName"], "Main");
}

#[tokio::test]
async fn status_reports_sonarr_v4_for_anime() {
    let (_s, st) = setup("ANIME", &[]).await;
    let (code, body) = get(&st, "/sonarr/L1/api/v3/system/status", Some("k1")).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["appName"], "Sonarr");
    assert!(body["version"].as_str().unwrap().starts_with("4."));
}

#[tokio::test]
async fn status_with_bad_key_is_401() {
    let server = MockServer::start().await;
    mock_op_status(&server, "Libraries", 401).await;
    let st = common::test_state_with(&server.uri()).await;
    let (code, _) = get(&st, "/radarr/L1/api/v3/system/status", Some("bad")).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn missing_api_key_is_401() {
    let (_s, st) = setup("MOVIE", &[]).await;
    let (code, _) = get(&st, "/radarr/L1/api/v3/system/status", None).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_library_is_404() {
    let (_s, st) = setup("MOVIE", &[]).await;
    let (code, _) = get(&st, "/radarr/NOPE/api/v3/system/status", Some("k1")).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn facet_mismatch_is_400() {
    let (_s, st) = setup("SERIES", &[]).await;
    let (code, body) = get(&st, "/radarr/L1/api/v3/system/status", Some("k1")).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(
        body[0]["errorMessage"],
        "library facet does not match radarr"
    );
}

#[tokio::test]
async fn rootfolder_records_library_roots() {
    let (_s, st) = setup("MOVIE", &["/media2/library/movies"]).await;
    let (code, body) = get(&st, "/radarr/L1/api/v3/rootfolder", Some("k1")).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body[0]["path"], "/media2/library/movies");
    assert_eq!(body[0]["accessible"], true);
    assert_eq!(body[0]["freeSpace"], 0);
    assert_eq!(
        st.store
            .library_for_path(Facet::Movie, "/media2/library/movies/X (2020)/x.mkv")
            .await
            .unwrap(),
        Some("L1".to_string())
    );
}

#[tokio::test]
async fn qualityprofile_ids_are_stable_ints() {
    let (server, st) = setup("MOVIE", &[]).await;
    mock_op(
        &server,
        "QualityProfiles",
        json!({"data": {"qualityProfileSettings": {
            "profiles": [{"id": "p1", "name": "HD"}, {"id": "p2", "name": "UHD"}],
            "globalProfileId": "p1"}}}),
    )
    .await;
    let (_, a) = get(&st, "/radarr/L1/api/v3/qualityprofile", Some("k1")).await;
    let (_, b) = get(&st, "/radarr/L1/api/v3/qualityprofile", Some("k1")).await;
    assert_eq!(a, b);
    assert_eq!(a.as_array().unwrap().len(), 2);
    assert!(a[0]["id"].is_i64());
    assert_ne!(a[0]["id"], a[1]["id"]);
    assert_eq!(a[0]["name"], "HD");
}

#[tokio::test]
async fn tags_get_and_post() {
    let (server, st) = setup("MOVIE", &[]).await;
    mock_op(
        &server,
        "TagDefinitions",
        json!({"data": {"titleTagDefinitions": [{"id": "t1", "label": "kids"}]}}),
    )
    .await;
    let (_, tags) = get(&st, "/radarr/L1/api/v3/tag", Some("k1")).await;
    assert_eq!(tags[0]["label"], "kids");
    assert!(tags[0]["id"].is_i64());

    let req = Request::builder()
        .method("POST")
        .uri("/radarr/L1/api/v3/tag")
        .header("X-Api-Key", "k1")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"label":"new"}"#))
        .unwrap();
    let resp = scryarr::app(st.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let b: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(b["label"], "new");
    assert!(b["id"].is_i64());
}

#[tokio::test]
async fn languageprofile_is_empty() {
    let (_s, st) = setup("SERIES", &[]).await;
    let (code, body) = get(&st, "/sonarr/L1/api/v3/languageprofile", Some("k1")).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, json!([]));
}
