#![recursion_limit = "256"]
mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::scryer_mock::{libraries_response, library, mock_op};
use http_body_util::BodyExt;
use scryarr::AppState;
use scryarr::store::IdKind;
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

fn add_result(t: Value, reused: bool) -> Value {
    json!({"data": {"addTitle": {"title": t, "reusedExistingTitle": reused}}})
}

fn movie_metadata() -> Value {
    json!({"data": {"metadataMovie": {
        "name": "The Matrix", "slug": "the-matrix-603", "year": 1999, "overview": "meta overview",
        "posterUrl": "", "tmdbId": 603, "imdbId": "tt0133093", "runtimeMinutes": 136
    }}})
}

fn movie_body(search: bool) -> Value {
    json!({
        "title": "The Matrix", "tmdbId": 603, "titleSlug": "the-matrix-603",
        "qualityProfileId": 0, "monitored": true, "minimumAvailability": "released",
        "rootFolderPath": "/x", "tags": [5],
        "addOptions": {"searchForMovie": search}
    })
}

async fn qp(st: &AppState, scryer_id: &str) -> i32 {
    st.store
        .arr_id(IdKind::QualityProfile, "L1", scryer_id)
        .await
        .unwrap()
}

#[tokio::test]
async fn post_movie_adds_title_and_searches() {
    let (server, st) = setup("MOVIE").await;
    let qpid = qp(&st, "qp-uuid").await;
    mock_op(&server, "MetadataMovie", movie_metadata()).await;
    expect_op(
        &server,
        "AddTitle",
        add_result(
            title("m1", "MOVIE", &[("tmdb", "603")], Some("qp-uuid"), true),
            false,
        ),
        1,
    )
    .await;
    expect_op(
        &server,
        "TriggerSearch",
        json!({"data": {"triggerAcquisitionSearch": {"id": "j1"}}}),
        1,
    )
    .await;
    let mut body = movie_body(true);
    body["qualityProfileId"] = json!(qpid);
    let (code, resp) = send(&st, Method::POST, "/radarr/L1/api/v3/movie", Some(body)).await;
    assert_eq!(code, StatusCode::CREATED, "{resp}");
    assert_eq!(resp["id"], 603);
    let add = vars_of(&server, "AddTitle").await;
    let input = &add[0]["input"];
    assert_eq!(input["facet"], "MOVIE");
    assert_eq!(input["libraryId"], "L1");
    assert_eq!(input["tmdbId"], 603);
    assert_eq!(input["name"], "The Matrix");
    assert_eq!(input["tags"], json!([]));
    assert_eq!(input["year"], 1999);
    assert_eq!(input["overview"], "meta overview");
    assert_eq!(input["options"]["qualityProfileId"], "qp-uuid");
    assert_eq!(input["options"]["minAvailability"], "released");
    let search = vars_of(&server, "TriggerSearch").await;
    assert_eq!(search[0]["input"]["titleId"], "m1");
    assert_eq!(search[0]["input"]["intent"], "AUTOMATIC");
    server.verify().await;
}

#[tokio::test]
async fn post_movie_without_search_skips_search() {
    let (server, st) = setup("MOVIE").await;
    let qpid = qp(&st, "qp-uuid").await;
    mock_op(&server, "MetadataMovie", movie_metadata()).await;
    expect_op(
        &server,
        "AddTitle",
        add_result(
            title("m1", "MOVIE", &[("tmdb", "603")], Some("qp-uuid"), true),
            false,
        ),
        1,
    )
    .await;
    expect_op(
        &server,
        "TriggerSearch",
        json!({"data": {"triggerAcquisitionSearch": {"id": "j1"}}}),
        0,
    )
    .await;
    let mut body = movie_body(false);
    body["qualityProfileId"] = json!(qpid);
    let (code, _) = send(&st, Method::POST, "/radarr/L1/api/v3/movie", Some(body)).await;
    assert_eq!(code, StatusCode::CREATED);
    server.verify().await;
}

#[tokio::test]
async fn post_movie_existing_title_returns_resource() {
    let (server, st) = setup("MOVIE").await;
    let qpid = qp(&st, "qp-uuid").await;
    mock_op(&server, "MetadataMovie", movie_metadata()).await;
    mock_op(
        &server,
        "AddTitle",
        add_result(
            title("m1", "MOVIE", &[("tmdb", "603")], Some("qp-uuid"), true),
            true,
        ),
    )
    .await;
    let mut body = movie_body(false);
    body["qualityProfileId"] = json!(qpid);
    let (code, resp) = send(&st, Method::POST, "/radarr/L1/api/v3/movie", Some(body)).await;
    assert_eq!(code, StatusCode::CREATED);
    assert_eq!(resp["id"], 603);
}

#[tokio::test]
async fn post_movie_unknown_quality_profile_is_400() {
    let (server, st) = setup("MOVIE").await;
    expect_op(
        &server,
        "AddTitle",
        add_result(title("m1", "MOVIE", &[("tmdb", "603")], None, true), false),
        0,
    )
    .await;
    let mut body = movie_body(false);
    body["qualityProfileId"] = json!(999);
    let (code, _) = send(&st, Method::POST, "/radarr/L1/api/v3/movie", Some(body)).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    server.verify().await;
}

#[tokio::test]
async fn post_movie_without_tmdb_id_is_400() {
    let (server, st) = setup("MOVIE").await;
    let qpid = qp(&st, "qp-uuid").await;
    expect_op(
        &server,
        "AddTitle",
        add_result(title("m1", "MOVIE", &[("tmdb", "603")], None, true), false),
        0,
    )
    .await;
    let mut body = movie_body(false);
    body["qualityProfileId"] = json!(qpid);
    body["tmdbId"] = json!(0);
    let (code, _) = send(&st, Method::POST, "/radarr/L1/api/v3/movie", Some(body)).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    server.verify().await;
}

#[tokio::test]
async fn post_series_monitors_selected_seasons() {
    let (server, st) = setup("SERIES").await;
    let qpid = qp(&st, "qp-uuid").await;
    mock_op(
        &server,
        "MetadataSeries",
        json!({"data": {"metadataSeries": {
            "name": "Show", "slug": "show", "year": 2010, "overview": "series meta",
            "posterUrl": "", "tvdbId": "81189", "tmdbId": 1, "network": "N", "seasons": []
        }}}),
    )
    .await;
    expect_op(
        &server,
        "AddTitle",
        add_result(
            title("s1", "SERIES", &[("tvdb", "81189")], Some("qp-uuid"), true),
            false,
        ),
        1,
    )
    .await;
    expect_op(
        &server,
        "TriggerSearch",
        json!({"data": {"triggerAcquisitionSearch": {"id": "j1"}}}),
        1,
    )
    .await;
    let body = json!({
        "title": "Show", "tvdbId": 81189, "titleSlug": "show", "qualityProfileId": qpid,
        "languageProfileId": 1, "monitored": true, "seriesType": "standard",
        "seasons": [
            {"seasonNumber": 1, "monitored": false},
            {"seasonNumber": 2, "monitored": true},
            {"seasonNumber": 3, "monitored": true}
        ],
        "rootFolderPath": "/x", "tags": [],
        "addOptions": {"searchForMissingEpisodes": true}
    });
    let (code, resp) = send(&st, Method::POST, "/sonarr/L1/api/v3/series", Some(body)).await;
    assert_eq!(code, StatusCode::CREATED, "{resp}");
    assert_eq!(resp["id"], 81189);
    let add = vars_of(&server, "AddTitle").await;
    let input = &add[0]["input"];
    assert_eq!(input["facet"], "SERIES");
    assert_eq!(input["tvdbId"], "81189");
    assert_eq!(input["options"]["monitorType"], "ADVANCED");
    assert_eq!(
        input["options"]["monitorSelection"]["seasonNumbers"],
        json!([2, 3])
    );
    assert_eq!(input["options"]["qualityProfileId"], "qp-uuid");
    server.verify().await;
}

fn series_detail(seasons: Vec<(&str, bool)>, monitored: bool) -> Value {
    let mut t = title(
        "s1",
        "SERIES",
        &[("tvdb", "81189")],
        Some("qp-uuid"),
        monitored,
    );
    t["collections"] =
        json!(seasons
        .into_iter()
        .map(|(i, m)| json!({"collectionType": "SEASON", "collectionIndex": i, "monitored": m,
            "episodesOwned": 0, "episodesMonitored": 0, "episodesTotal": 0, "episodes": []}))
        .collect::<Vec<_>>());
    t["mediaFiles"] = json!([]);
    json!({"data": {"title": t}})
}

#[tokio::test]
async fn put_series_updates_monitor_selection() {
    let (server, st) = setup("SERIES").await;
    let qpid = qp(&st, "qp-uuid").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        by_ext(vec![title(
            "s1",
            "SERIES",
            &[("tvdb", "81189")],
            Some("qp-uuid"),
            true,
        )]),
    )
    .await;
    mock_op(
        &server,
        "TitleDetail",
        series_detail(vec![("1", true), ("2", false)], true),
    )
    .await;
    expect_op(
        &server,
        "UpdateTitle",
        json!({"data": {"updateTitle": title("s1", "SERIES", &[("tvdb", "81189")], Some("qp-uuid"), true)}}),
        1,
    )
    .await;
    expect_op(&server, "SetTitleMonitored", json!({"data": {"setTitleMonitored": title("s1", "SERIES", &[("tvdb", "81189")], Some("qp-uuid"), true)}}), 0).await;
    let body = json!({
        "id": 81189, "title": "Title s1", "tvdbId": 81189, "qualityProfileId": qpid,
        "monitored": true,
        "seasons": [{"seasonNumber": 1, "monitored": false}, {"seasonNumber": 2, "monitored": true}]
    });
    let (code, resp) = send(
        &st,
        Method::PUT,
        "/sonarr/L1/api/v3/series/81189",
        Some(body),
    )
    .await;
    assert_eq!(code, StatusCode::ACCEPTED, "{resp}");
    assert_eq!(resp["id"], 81189);
    let up = vars_of(&server, "UpdateTitle").await;
    assert_eq!(up[0]["input"]["titleId"], "s1");
    assert_eq!(up[0]["input"]["options"]["monitorType"], "ADVANCED");
    assert_eq!(
        up[0]["input"]["options"]["monitorSelection"]["seasonNumbers"],
        json!([2])
    );
    assert!(up[0]["input"]["options"].get("qualityProfileId").is_none());
    server.verify().await;
}

#[tokio::test]
async fn put_movie_changes_profile_and_monitored_only_when_different() {
    let (server, st) = setup("MOVIE").await;
    let old = qp(&st, "qp-old").await;
    let new = qp(&st, "qp-new").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        by_ext(vec![title(
            "m1",
            "MOVIE",
            &[("tmdb", "603")],
            Some("qp-old"),
            true,
        )]),
    )
    .await;
    expect_op(
        &server,
        "UpdateTitle",
        json!({"data": {"updateTitle": title("m1", "MOVIE", &[("tmdb", "603")], Some("qp-new"), true)}}),
        1,
    )
    .await;
    expect_op(
        &server,
        "SetTitleMonitored",
        json!({"data": {"setTitleMonitored": title("m1", "MOVIE", &[("tmdb", "603")], Some("qp-new"), false)}}),
        1,
    )
    .await;
    let body = json!({"id": 603, "title": "Title m1", "tmdbId": 603, "qualityProfileId": new, "monitored": false});
    let (code, resp) = send(&st, Method::PUT, "/radarr/L1/api/v3/movie/603", Some(body)).await;
    assert_eq!(code, StatusCode::ACCEPTED, "{resp}");
    assert_eq!(resp["monitored"], false);
    let up = vars_of(&server, "UpdateTitle").await;
    assert_eq!(up[0]["input"]["options"]["qualityProfileId"], "qp-new");
    assert!(up[0]["input"]["options"].get("monitorType").is_none());
    let sm = vars_of(&server, "SetTitleMonitored").await;
    assert_eq!(sm[0]["input"], json!({"titleId": "m1", "monitored": false}));

    let unchanged = json!({"id": 603, "tmdbId": 603, "qualityProfileId": old, "monitored": true});
    let (code, _) = send(
        &st,
        Method::PUT,
        "/radarr/L1/api/v3/movie/603",
        Some(unchanged),
    )
    .await;
    assert_eq!(code, StatusCode::ACCEPTED);
    assert_eq!(vars_of(&server, "UpdateTitle").await.len(), 1);
    assert_eq!(vars_of(&server, "SetTitleMonitored").await.len(), 1);
}

#[tokio::test]
async fn delete_movie_never_deletes_files() {
    let (server, st) = setup("MOVIE").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        by_ext(vec![title("m1", "MOVIE", &[("tmdb", "603")], None, true)]),
    )
    .await;
    expect_op(
        &server,
        "DeleteTitle",
        json!({"data": {"deleteTitle": {"id": "m1"}}}),
        1,
    )
    .await;
    let (code, resp) = send(
        &st,
        Method::DELETE,
        "/radarr/L1/api/v3/movie/603?deleteFiles=true",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(resp, json!({}));
    let del = vars_of(&server, "DeleteTitle").await;
    assert_eq!(del[0]["input"]["titleId"], "m1");
    assert_eq!(del[0]["input"]["deleteFilesOnDisk"], false);
    server.verify().await;
}

#[tokio::test]
async fn delete_series_never_deletes_files() {
    let (server, st) = setup("SERIES").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        by_ext(vec![title(
            "s1",
            "SERIES",
            &[("tvdb", "81189")],
            None,
            true,
        )]),
    )
    .await;
    expect_op(
        &server,
        "DeleteTitle",
        json!({"data": {"deleteTitle": {"id": "s1"}}}),
        1,
    )
    .await;
    let (code, _) = send(
        &st,
        Method::DELETE,
        "/sonarr/L1/api/v3/series/81189?deleteFiles=true",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let del = vars_of(&server, "DeleteTitle").await;
    assert_eq!(del[0]["input"]["deleteFilesOnDisk"], false);
    server.verify().await;
}

#[tokio::test]
async fn post_movie_succeeds_when_metadata_fails() {
    let (server, st) = setup("MOVIE").await;
    let qpid = qp(&st, "qp-uuid").await;
    common::scryer_mock::mock_op_status(&server, "MetadataMovie", 500).await;
    expect_op(
        &server,
        "AddTitle",
        add_result(
            title("m1", "MOVIE", &[("tmdb", "603")], Some("qp-uuid"), true),
            false,
        ),
        1,
    )
    .await;
    let mut body = movie_body(false);
    body["qualityProfileId"] = json!(qpid);
    let (code, resp) = send(&st, Method::POST, "/radarr/L1/api/v3/movie", Some(body)).await;
    assert_eq!(code, StatusCode::CREATED, "{resp}");
    let add = vars_of(&server, "AddTitle").await;
    assert!(add[0]["input"].get("year").is_none());
    assert!(add[0]["input"].get("overview").is_none());
}

#[tokio::test]
async fn post_series_succeeds_when_metadata_fails() {
    let (server, st) = setup("SERIES").await;
    let qpid = qp(&st, "qp-uuid").await;
    common::scryer_mock::mock_op_status(&server, "MetadataSeries", 500).await;
    expect_op(
        &server,
        "AddTitle",
        add_result(
            title("s1", "SERIES", &[("tvdb", "81189")], Some("qp-uuid"), true),
            false,
        ),
        1,
    )
    .await;
    let body = json!({
        "title": "Show", "tvdbId": 81189, "qualityProfileId": qpid, "monitored": true,
        "seasons": [{"seasonNumber": 1, "monitored": true}]
    });
    let (code, resp) = send(&st, Method::POST, "/sonarr/L1/api/v3/series", Some(body)).await;
    assert_eq!(code, StatusCode::CREATED, "{resp}");
    server.verify().await;
}
