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

fn title(id: &str, lib: &str, facet: &str, ext: &[(&str, &str)]) -> Value {
    json!({
        "id": id, "libraryId": lib, "name": format!("Title {id}"), "facet": facet,
        "monitored": true, "year": 2020, "slug": format!("title-{id}"),
        "overview": "o", "tags": [],
        "externalIds": ext.iter().map(|(s, v)| json!({"source": s, "value": v})).collect::<Vec<_>>(),
        "imdbId": null, "qualityProfileId": null, "rootFolderPath": null,
        "minAvailability": null, "monitorType": null, "sizeBytes": 0,
        "episodesOwned": 0, "episodesMonitored": 0, "episodesTotal": 0,
        "createdAt": "2024-01-01T00:00:00Z", "posterUrl": null
    })
}

fn qitem(item: &str, title_id: &str, lib: &str, state: &str, tmdb: &str) -> Value {
    json!({
        "id": format!("q-{item}"), "titleId": title_id, "episodeId": null,
        "titleName": format!("Release {item}"), "facet": "movie",
        "clientId": "c1", "clientType": "sabnzbd",
        "downloadClientItemId": item, "downloadId": format!("dl-{item}"),
        "displayState": state, "trackedState": null,
        "progressPercent": 25, "sizeBytes": 1000, "remainingSeconds": 3725,
        "sourceProvider": null, "queueScope": null,
        "title": {"id": title_id, "libraryId": lib,
                  "externalIds": [{"source": "tmdb", "value": tmdb}]}
    })
}

fn queue_resp(items: Vec<Value>) -> Value {
    let n = items.len();
    json!({"data": {"downloadQueuePage": {"items": items, "hasMore": false, "totalCount": n}}})
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

#[tokio::test]
async fn queue_filters_to_library_and_paginates() {
    let (server, state) = setup("MOVIE").await;
    mock_op(
        &server,
        "QueuePage",
        queue_resp(vec![
            qitem("a", "T1", "L1", "DOWNLOADING", "11"),
            qitem("b", "T2", "L2", "DOWNLOADING", "22"),
            qitem("c", "T3", "L1", "QUEUED", "33"),
        ]),
    )
    .await;
    let (s, v) = send(&state, Method::GET, "/radarr/L1/api/v3/queue", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["totalRecords"], 2);
    assert_eq!(v["records"].as_array().unwrap().len(), 2);
    assert_eq!(v["records"][0]["movieId"], 11);

    let (_, v) = send(
        &state,
        Method::GET,
        "/radarr/L1/api/v3/queue?page=2&pageSize=1",
        None,
    )
    .await;
    assert_eq!(v["page"], 2);
    assert_eq!(v["pageSize"], 1);
    assert_eq!(v["totalRecords"], 2);
    assert_eq!(v["records"].as_array().unwrap().len(), 1);
    assert_eq!(v["records"][0]["movieId"], 33);
    assert_eq!(v["records"][0]["status"], "queued");
}

#[tokio::test]
async fn queue_maps_progress() {
    let (server, state) = setup("MOVIE").await;
    mock_op(
        &server,
        "QueuePage",
        queue_resp(vec![qitem("a", "T1", "L1", "DOWNLOADING", "11")]),
    )
    .await;
    let (_, v) = send(&state, Method::GET, "/radarr/L1/api/v3/queue", None).await;
    let r = &v["records"][0];
    assert_eq!(r["size"], 1000.0);
    assert_eq!(r["sizeleft"], 750.0);
    assert_eq!(r["timeleft"], "01:02:05");
    assert_eq!(r["status"], "downloading");
    assert_eq!(r["trackedDownloadState"], "downloading");
    assert_eq!(r["downloadId"], "dl-a");
    assert_eq!(r["protocol"], "unknown");
    assert_eq!(r["downloadClient"], "sabnzbd");
    assert_eq!(r["title"], "Release a");
}

#[tokio::test]
async fn queue_delete_respects_remove_from_client() {
    let (server, state) = setup("MOVIE").await;
    mock_op(
        &server,
        "QueuePage",
        queue_resp(vec![qitem("a", "T1", "L1", "DOWNLOADING", "11")]),
    )
    .await;
    mock_op(
        &server,
        "DeleteDownload",
        json!({"data": {"deleteDownload": {"removed": true}}}),
    )
    .await;
    mock_op(
        &server,
        "IgnoreDownload",
        json!({"data": {"ignoreTrackedDownload": {"kind": "ok"}}}),
    )
    .await;
    let (_, v) = send(&state, Method::GET, "/radarr/L1/api/v3/queue", None).await;
    let id = v["records"][0]["id"].as_i64().unwrap();

    let uri = format!("/radarr/L1/api/v3/queue/{id}?removeFromClient=true&blocklist=false");
    let (s, _) = send(&state, Method::DELETE, &uri, None).await;
    assert_eq!(s, StatusCode::OK);
    let del = vars_of(&server, "DeleteDownload").await;
    assert_eq!(del.len(), 1);
    assert_eq!(
        del[0]["input"],
        json!({"clientId": "c1", "clientType": "sabnzbd", "downloadClientItemId": "a", "isHistory": false})
    );
    assert!(vars_of(&server, "IgnoreDownload").await.is_empty());

    let uri = format!("/radarr/L1/api/v3/queue/{id}?removeFromClient=false&blocklist=false");
    let (s, _) = send(&state, Method::DELETE, &uri, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(vars_of(&server, "DeleteDownload").await.len(), 1);
    let ign = vars_of(&server, "IgnoreDownload").await;
    assert_eq!(ign.len(), 1);
    assert_eq!(ign[0]["input"]["downloadClientItemId"], "a");

    let (s, _) = send(
        &state,
        Method::DELETE,
        "/radarr/L1/api/v3/queue/99999",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn history_failed_resolves_client_type() {
    let (server, state) = setup("MOVIE").await;
    mock_op(
        &server,
        "TitleHistory",
        json!({"data": {"titleHistory": {"hasMore": false, "items": [{
            "id": "h1", "titleId": "T1", "episodeId": null, "episodeIds": [], "collectionId": null,
            "eventType": "grabbed",
            "sourceTitle": "Rel", "quality": null, "downloadId": "dl-x", "clientId": "c9",
            "occurredAt": "2024-02-02T00:00:00Z"
        }, {
            "id": "h2", "titleId": "T1", "episodeId": null, "episodeIds": [], "collectionId": null,
            "eventType": "imported",
            "sourceTitle": "Rel", "quality": null, "downloadId": "dl-x", "clientId": "c9",
            "occurredAt": "2024-02-03T00:00:00Z"
        }]}}}),
    )
    .await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        json!({"data": {"titlesByExternalIds": [title("T1", "L1", "MOVIE", &[("tmdb", "11")])]}}),
    )
    .await;
    mock_op(
        &server,
        "TitleDownloads",
        json!({"data": {"title": {"downloadQueueItems": [
            {"clientId": "c9", "clientType": "nzbget", "downloadClientItemId": "item-x", "downloadId": "dl-x"},
            {"clientId": "c9", "clientType": "other", "downloadClientItemId": "item-y", "downloadId": "dl-y"}
        ]}}}),
    )
    .await;
    mock_op(
        &server,
        "MarkFailed",
        json!({"data": {"markTrackedDownloadFailed": {"kind": "ok"}}}),
    )
    .await;

    let (s, v) = send(
        &state,
        Method::GET,
        "/radarr/L1/api/v3/history/movie?movieId=11",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v[0]["eventType"], "grabbed");
    assert_eq!(v[0]["movieId"], 11);
    assert_eq!(v[0]["downloadId"], "dl-x");
    assert_eq!(v[0]["date"], "2024-02-02T00:00:00Z");
    assert_eq!(v[1]["eventType"], "downloadFolderImported");
    assert_ne!(v[0]["id"], v[1]["id"]);
    let id = v[1]["id"].as_i64().unwrap();

    let (s, _) = send(
        &state,
        Method::POST,
        &format!("/radarr/L1/api/v3/history/failed/{id}"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let m = vars_of(&server, "MarkFailed").await;
    assert_eq!(m.len(), 1);
    assert_eq!(m[0]["input"]["clientType"], "nzbget");
    assert_eq!(m[0]["input"]["downloadClientItemId"], "item-x");
    assert_eq!(m[0]["input"]["clientId"], "c9");

    let (s, _) = send(
        &state,
        Method::POST,
        "/radarr/L1/api/v3/history/failed/424242",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn command_movies_search_triggers_search() {
    let (server, state) = setup("MOVIE").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        json!({"data": {"titlesByExternalIds": [title("T1", "L1", "MOVIE", &[("tmdb", "11")])]}}),
    )
    .await;
    mock_op(
        &server,
        "TriggerSearch",
        json!({"data": {"triggerAcquisitionSearch": {"id": "job"}}}),
    )
    .await;
    let (s, v) = send(
        &state,
        Method::POST,
        "/radarr/L1/api/v3/command",
        Some(json!({"name": "MoviesSearch", "movieIds": [11]})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(v["name"], "MoviesSearch");
    assert_eq!(v["status"], "completed");
    let first = v["id"].as_i64().unwrap();
    let t = vars_of(&server, "TriggerSearch").await;
    assert_eq!(t.len(), 1);
    assert_eq!(t[0]["input"]["titleId"], "T1");
    assert_eq!(t[0]["input"]["intent"], "AUTOMATIC");

    let (s, v) = send(
        &state,
        Method::POST,
        "/radarr/L1/api/v3/command",
        Some(json!({"name": "RefreshMonitoredDownloads"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert!(v["id"].as_i64().unwrap() > first);
    assert_eq!(vars_of(&server, "TriggerSearch").await.len(), 1);
}

#[tokio::test]
async fn command_unknown_is_400() {
    let (_server, state) = setup("MOVIE").await;
    let (s, _) = send(
        &state,
        Method::POST,
        "/radarr/L1/api/v3/command",
        Some(json!({"name": "RescanMovie"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

fn series_detail(id: &str) -> Value {
    let mut t = title(id, "L1", "SERIES", &[("tvdb", "81189")]);
    let ep = |id: &str, s: &str, n: &str| {
        json!({"id": id, "seasonNumber": s, "episodeNumber": n, "title": null, "airDate": null,
               "monitored": true, "mediaFiles": []})
    };
    t["collections"] = json!([
        {"id": "c1", "collectionType": "SEASON", "collectionIndex": "1", "monitored": true,
         "episodesOwned": 0, "episodesMonitored": 2, "episodesTotal": 2,
         "episodes": [ep("e1", "1", "1"), ep("e2", "1", "2")]},
        {"id": "c2", "collectionType": "SEASON", "collectionIndex": "2", "monitored": true,
         "episodesOwned": 0, "episodesMonitored": 1, "episodesTotal": 1,
         "episodes": [ep("e3", "2", "1")]}
    ]);
    t["mediaFiles"] = json!([]);
    json!({"data": {"title": t}})
}

async fn mock_detail(server: &MockServer, id: &str, resp: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(
            json!({"operationName": "TitleDetail", "variables": {"id": id}}),
        ))
        .respond_with(resp)
        .mount(server)
        .await;
}

fn series_qitem(item: &str, title_id: &str, episode: Option<&str>, scope: Value) -> Value {
    let mut v = qitem(item, title_id, "L1", "DOWNLOADING", "0");
    v["facet"] = json!("series");
    v["episodeId"] = json!(episode);
    v["queueScope"] = scope;
    v["title"]["externalIds"] = json!([{"source": "tvdb", "value": "81189"}]);
    v
}

#[tokio::test]
async fn queue_resolves_season_from_scope() {
    let (server, state) = setup("SERIES").await;
    mock_op(
        &server,
        "QueuePage",
        queue_resp(vec![
            series_qitem(
                "pack",
                "T1",
                None,
                json!({"__typename": "CollectionScopePayload", "collectionId": "c2"}),
            ),
            series_qitem(
                "set",
                "T1",
                None,
                json!({"__typename": "EpisodeSetScopePayload", "episodeIds": ["e1", "e2"]}),
            ),
            series_qitem("ep", "T1", Some("e3"), Value::Null),
            series_qitem(
                "mixed",
                "T1",
                None,
                json!({"__typename": "EpisodeSetScopePayload", "episodeIds": ["e1", "e3"]}),
            ),
            series_qitem(
                "broken",
                "T2",
                None,
                json!({"__typename": "CollectionScopePayload", "collectionId": "c9"}),
            ),
            series_qitem("none", "T1", None, Value::Null),
        ]),
    )
    .await;
    mock_detail(
        &server,
        "T1",
        ResponseTemplate::new(200).set_body_json(series_detail("T1")),
    )
    .await;
    mock_detail(&server, "T2", ResponseTemplate::new(500)).await;
    let (s, v) = send(
        &state,
        Method::GET,
        "/sonarr/L1/api/v3/queue?pageSize=20",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let seasons: Vec<Value> = v["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["seasonNumber"].clone())
        .collect();
    assert_eq!(
        seasons,
        vec![
            json!(2),
            json!(1),
            json!(2),
            Value::Null,
            Value::Null,
            Value::Null
        ]
    );
    assert!(v["records"][2]["episodeId"].is_i64());
    assert!(v["records"][0]["episodeId"].is_null());
    assert_eq!(v["records"][0]["seriesId"], 81189);
}

#[tokio::test]
async fn history_series_filters_on_resolved_season() {
    let (server, state) = setup("SERIES").await;
    let h = |id: &str, episode: Option<&str>, episodes: &[&str], collection: Option<&str>| {
        json!({"id": id, "titleId": "T1", "episodeId": episode, "episodeIds": episodes,
               "collectionId": collection, "eventType": "grabbed", "sourceTitle": id,
               "quality": null, "downloadId": null, "clientId": null,
               "occurredAt": "2024-02-02T00:00:00Z"})
    };
    mock_op(
        &server,
        "TitleHistory",
        json!({"data": {"titleHistory": {"hasMore": false, "items": [
            h("ep1", Some("e1"), &[], None),
            h("set2", None, &["e3"], None),
            h("pack2", None, &[], Some("c2")),
            h("pack1", None, &["e1", "e2"], Some("c1")),
            h("bare", None, &[], None),
        ]}}}),
    )
    .await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        json!({"data": {"titlesByExternalIds": [title("T1", "L1", "SERIES", &[("tvdb", "81189")])]}}),
    )
    .await;
    mock_detail(
        &server,
        "T1",
        ResponseTemplate::new(200).set_body_json(series_detail("T1")),
    )
    .await;
    let (s, v) = send(
        &state,
        Method::GET,
        "/sonarr/L1/api/v3/history/series?seriesId=81189&seasonNumber=2",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let titles: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["sourceTitle"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["set2", "pack2"]);

    let (_, v) = send(
        &state,
        Method::GET,
        "/sonarr/L1/api/v3/history/series?seriesId=81189&seasonNumber=1",
        None,
    )
    .await;
    let titles: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["sourceTitle"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["ep1", "pack1"]);

    let (_, v) = send(
        &state,
        Method::GET,
        "/sonarr/L1/api/v3/history/series?seriesId=81189",
        None,
    )
    .await;
    assert_eq!(v.as_array().unwrap().len(), 5);
}

async fn mock_title(server: &MockServer, tmdb: &str, id: &str) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({
            "operationName": "TitlesByExternalIds", "variables": {"values": [tmdb]}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data": {"titlesByExternalIds": [title(id, "L1", "MOVIE", &[("tmdb", tmdb)])]}}),
        ))
        .mount(server)
        .await;
}

#[tokio::test]
async fn command_search_tolerates_a_running_search() {
    let (server, state) = setup("MOVIE").await;
    mock_title(&server, "11", "T1").await;
    mock_title(&server, "22", "T2").await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({
            "operationName": "TriggerSearch", "variables": {"input": {"titleId": "T1"}}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": null, "errors": [{
            "message": "validation: Another search is running.",
            "extensions": {"code": "VALIDATION_ERROR", "reason": "ACQUISITION_ANOTHER_SEARCH_RUNNING"}
        }]})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({
            "operationName": "TriggerSearch", "variables": {"input": {"titleId": "T2"}}
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": {"triggerAcquisitionSearch": {"id": "job"}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (s, v) = send(
        &state,
        Method::POST,
        "/radarr/L1/api/v3/command",
        Some(json!({"name": "MoviesSearch", "movieIds": [11, 22]})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["name"], "MoviesSearch");
    server.verify().await;
}

#[tokio::test]
async fn command_search_other_scryer_errors_still_fail() {
    let (server, state) = setup("MOVIE").await;
    mock_title(&server, "11", "T1").await;
    mock_op(
        &server,
        "TriggerSearch",
        json!({"data": null, "errors": [{"message": "unauthorized: no", "extensions": {"code": "UNAUTHORIZED"}}]}),
    )
    .await;
    let (s, _) = send(
        &state,
        Method::POST,
        "/radarr/L1/api/v3/command",
        Some(json!({"name": "MoviesSearch", "movieIds": [11]})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn queue_stops_on_an_empty_page() {
    let (server, state) = setup("MOVIE").await;
    mock_op(
        &server,
        "QueuePage",
        json!({"data": {"downloadQueuePage": {"items": [], "hasMore": true, "totalCount": 7}}}),
    )
    .await;
    let (s, v) = send(&state, Method::GET, "/radarr/L1/api/v3/queue", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalRecords"], 0);
    assert_eq!(vars_of(&server, "QueuePage").await.len(), 1);
}

#[tokio::test]
async fn queue_selects_scope_only_for_sonarr() {
    let (server, state) = setup("MOVIE").await;
    mock_op(&server, "QueuePage", queue_resp(vec![])).await;
    send(&state, Method::GET, "/radarr/L1/api/v3/queue", None).await;
    let vars = vars_of(&server, "QueuePage").await;
    assert_eq!(vars[0]["withScope"], false);

    let (server, state) = setup("SERIES").await;
    mock_op(&server, "QueuePage", queue_resp(vec![])).await;
    send(&state, Method::GET, "/sonarr/L1/api/v3/queue", None).await;
    let vars = vars_of(&server, "QueuePage").await;
    assert_eq!(vars[0]["withScope"], true);
}

#[tokio::test]
async fn queue_tolerates_scope_errors_only() {
    let (server, state) = setup("SERIES").await;
    let mut resp = queue_resp(vec![series_qitem("ep", "T1", Some("e3"), Value::Null)]);
    resp["errors"] = json!([{
        "message": "scope lookup failed",
        "path": ["downloadQueuePage", "items", 0, "queueScope"]
    }]);
    mock_op(&server, "QueuePage", resp).await;
    mock_detail(
        &server,
        "T1",
        ResponseTemplate::new(200).set_body_json(series_detail("T1")),
    )
    .await;
    let (s, v) = send(&state, Method::GET, "/sonarr/L1/api/v3/queue", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["totalRecords"], 1);
    assert_eq!(v["records"][0]["seasonNumber"], 2);

    let (server, state) = setup("SERIES").await;
    let mut resp = queue_resp(vec![series_qitem("ep", "T1", Some("e3"), Value::Null)]);
    resp["errors"] = json!([
        {"message": "scope lookup failed", "path": ["downloadQueuePage", "items", 0, "queueScope"]},
        {"message": "title lookup failed", "path": ["downloadQueuePage", "items", 0, "title"]}
    ]);
    mock_op(&server, "QueuePage", resp).await;
    let (s, _) = send(&state, Method::GET, "/sonarr/L1/api/v3/queue", None).await;
    assert!(!s.is_success(), "{s}");
}
