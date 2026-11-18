#![recursion_limit = "256"]
mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::scryer_mock::{libraries_response, library, mock_op};
use http_body_util::BodyExt;
use scryarr::AppState;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn send(state: &AppState, m: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(m)
        .uri(uri)
        .header("X-Api-Key", "k1");
    let req = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            b.body(Body::from(v.to_string())).unwrap()
        }
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = scryarr::app(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn setup(facet: &str) -> (MockServer, AppState) {
    let server = MockServer::start().await;
    mock_op(
        &server,
        "Libraries",
        libraries_response(vec![library("L1", facet, "Main", &[])]),
    )
    .await;
    let state = common::test_state_with(&server.uri()).await;
    (server, state)
}

fn title(id: &str, facet: &str, ext: &[(&str, &str)], qp: Option<&str>, monitored: bool) -> Value {
    json!({
        "id": id, "libraryId": "L1", "name": format!("Title {id}"), "facet": facet,
        "monitored": monitored, "year": 2020, "slug": format!("title-{id}"),
        "overview": "o", "tags": [],
        "externalIds": ext.iter().map(|(s, v)| json!({"source": s, "value": v})).collect::<Vec<_>>(),
        "imdbId": null, "qualityProfileId": qp, "rootFolderPath": null,
        "minAvailability": null, "monitorType": null, "sizeBytes": 0,
        "episodesOwned": 0, "episodesMonitored": 0, "episodesTotal": 0,
        "createdAt": "2024-01-01T00:00:00Z", "posterUrl": null
    })
}

fn by_ext(items: Vec<Value>) -> Value {
    json!({"data": {"titlesByExternalIds": items}})
}

async fn expect_op(server: &MockServer, op: &str, resp: Value, times: u64) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({"operationName": op})))
        .respond_with(ResponseTemplate::new(200).set_body_json(resp))
        .expect(times)
        .mount(server)
        .await;
}

async fn vars_of(server: &MockServer, op: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter_map(|r| {
            let v: Value = serde_json::from_slice(&r.body).ok()?;
            (v["operationName"] == op).then(|| v["variables"].clone())
        })
        .collect()
}

fn detail_resp() -> Value {
    let mut t = title("T1", "SERIES", &[("tvdb", "81189")], None, true);
    let o = t.as_object_mut().unwrap();
    o.insert("collections".into(), json!([
        {"collectionType":"SEASON","collectionIndex":"1","monitored":true,"episodesOwned":1,"episodesMonitored":2,"episodesTotal":2,
         "episodes":[
            {"id":"e11","seasonNumber":"1","episodeNumber":"1","title":"Pilot","airDate":"2020-01-01","monitored":true,
             "mediaFiles":[{"id":"f11","filePath":"/tv/s1e1.mkv"}]},
            {"id":"e12","seasonNumber":"1","episodeNumber":"2","title":"Two","airDate":null,"monitored":false,"mediaFiles":[]}]},
        {"collectionType":"SEASON","collectionIndex":"2","monitored":true,"episodesOwned":0,"episodesMonitored":2,"episodesTotal":2,
         "episodes":[
            {"id":"e21","seasonNumber":"2","episodeNumber":"1","title":"Three","airDate":"2021-01-01","monitored":true,"mediaFiles":[]},
            {"id":"e2x","seasonNumber":"2","episodeNumber":"x","title":"Bad","airDate":null,"monitored":true,"mediaFiles":[]}]}
    ]));
    o.insert("mediaFiles".into(), json!([{"id":"f11","filePath":"/tv/s1e1.mkv","episodeId":"e11","sizeBytes":4096},{"id":"orphan","filePath":"/tv/o.mkv","episodeId":null,"sizeBytes":1}]));
    json!({"data": {"title": t}})
}

async fn series_setup() -> (MockServer, AppState) {
    let (server, state) = setup("SERIES").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        by_ext(vec![title(
            "T1",
            "SERIES",
            &[("tvdb", "81189")],
            None,
            true,
        )]),
    )
    .await;
    mock_op(&server, "TitleDetail", detail_resp()).await;
    (server, state)
}

const BASE: &str = "/sonarr/L1/api/v3";

#[tokio::test]
async fn episodes_flatten_seasons() {
    let (_s, state) = series_setup().await;
    let (st, body) = send(
        &state,
        Method::GET,
        &format!("{BASE}/episode?seriesId=81189"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let eps = body.as_array().unwrap();
    assert_eq!(eps.len(), 3);
    assert_eq!(eps[0]["seriesId"], 81189);
    assert_eq!(eps[0]["seasonNumber"], 1);
    assert_eq!(eps[0]["episodeNumber"], 1);
    assert_eq!(eps[0]["hasFile"], true);
    assert_eq!(eps[0]["airDateUtc"], "2020-01-01T00:00:00Z");
    assert!(eps[0]["episodeFileId"].as_i64().unwrap() > 0);
    assert_eq!(eps[1]["hasFile"], false);
    assert_eq!(eps[1]["episodeFileId"], 0);
    assert!(eps[1]["airDateUtc"].is_null());
    assert_eq!(eps[2]["seasonNumber"], 2);
    assert_eq!(eps[2]["hasFile"], false);
}

#[tokio::test]
async fn episode_routes_reject_radarr_mount() {
    let (_s, state) = setup("MOVIE").await;
    let (st, _) = send(
        &state,
        Method::GET,
        "/radarr/L1/api/v3/episode?seriesId=1",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

async fn ids(state: &AppState) -> Vec<i64> {
    let (_, body) = send(
        state,
        Method::GET,
        &format!("{BASE}/episode?seriesId=81189"),
        None,
    )
    .await;
    body.as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn episode_monitor_maps_ids() {
    let (server, state) = series_setup().await;
    expect_op(
        &server,
        "SetEpisodeMonitored",
        json!({"data": {"setEpisodeMonitored": {"id": "x", "monitored": true}}}),
        2,
    )
    .await;
    let ids = ids(&state).await;
    let (st, _) = send(
        &state,
        Method::PUT,
        &format!("{BASE}/episode/monitor"),
        Some(json!({"episodeIds": [ids[0], ids[1]], "monitored": true})),
    )
    .await;
    assert!(st.is_success());
    let v = vars_of(&server, "SetEpisodeMonitored").await;
    assert_eq!(v.len(), 2);
    assert_eq!(
        v[0]["input"],
        json!({"episodeId": "e11", "monitored": true})
    );
    assert_eq!(
        v[1]["input"],
        json!({"episodeId": "e12", "monitored": true})
    );
}

#[tokio::test]
async fn episode_monitor_unknown_id_is_404_and_mutates_nothing() {
    let (server, state) = series_setup().await;
    let ids = ids(&state).await;
    let (st, _) = send(
        &state,
        Method::PUT,
        &format!("{BASE}/episode/monitor"),
        Some(json!({"episodeIds": [ids[0], 999999], "monitored": true})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(vars_of(&server, "SetEpisodeMonitored").await.is_empty());
}

#[tokio::test]
async fn episodefile_list_and_get() {
    let (_s, state) = series_setup().await;
    let (st, body) = send(
        &state,
        Method::GET,
        &format!("{BASE}/episodefile?seriesId=81189"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let files = body.as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["seasonNumber"], 1);
    assert_eq!(files[0]["path"], "/tv/s1e1.mkv");
    assert_eq!(files[0]["size"], 4096);
    assert_eq!(files[0]["seriesId"], 81189);
    let id = files[0]["id"].as_i64().unwrap();
    let (st, one) = send(
        &state,
        Method::GET,
        &format!("{BASE}/episodefile/{id}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(one["id"], id);
    assert_eq!(one["size"], 4096);
    let (_, eps) = send(
        &state,
        Method::GET,
        &format!("{BASE}/episode?seriesId=81189"),
        None,
    )
    .await;
    assert_eq!(eps[0]["episodeFileId"], id);
    let (st, _) = send(
        &state,
        Method::GET,
        &format!("{BASE}/episodefile/424242"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn episodefile_delete_uses_preview_fingerprint() {
    let (server, state) = series_setup().await;
    expect_op(&server, "DeleteEpisodeFilesPreview", json!({"data": {"deleteEpisodeFilesPreview": {
        "preview": {"fingerprint": "fp1", "requiresTypedConfirmation": true, "typedConfirmationPrompt": "DELETE"},
        "fileCount": 1}}}), 1).await;
    expect_op(
        &server,
        "DeleteEpisodeFiles",
        json!({"data": {"deleteEpisodeFiles": {"acceptedFileIds": ["f11"]}}}),
        1,
    )
    .await;
    let (_, body) = send(
        &state,
        Method::GET,
        &format!("{BASE}/episodefile?seriesId=81189"),
        None,
    )
    .await;
    let id = body[0]["id"].as_i64().unwrap();
    let (st, resp) = send(
        &state,
        Method::DELETE,
        &format!("{BASE}/episodefile/{id}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(resp, json!({}));
    let p = vars_of(&server, "DeleteEpisodeFilesPreview").await;
    assert_eq!(
        p[0]["input"],
        json!({"titleId": "T1", "episodeIds": ["e11"]})
    );
    let d = vars_of(&server, "DeleteEpisodeFiles").await;
    assert_eq!(
        d[0]["input"],
        json!({"titleId": "T1", "episodeIds": ["e11"], "deleteFromDisk": true,
        "previewFingerprint": "fp1", "typedConfirmation": "DELETE"})
    );
}

#[tokio::test]
async fn episodefile_delete_unknown_is_404() {
    let (server, state) = series_setup().await;
    let (st, _) = send(
        &state,
        Method::DELETE,
        &format!("{BASE}/episodefile/777"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(vars_of(&server, "DeleteEpisodeFiles").await.is_empty());
}
