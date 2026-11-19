#![recursion_limit = "256"]
mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::scryer_mock::{libraries_response, library, mock_op};
use http_body_util::BodyExt;
use scryarr::AppState;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn get(state: &AppState, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .uri(uri)
        .header("X-Api-Key", "k1")
        .body(Body::empty())
        .unwrap();
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
        "minAvailability": null, "monitorType": null, "sizeBytes": 100,
        "episodesOwned": 3, "episodesMonitored": 4, "episodesTotal": 10,
        "createdAt": "2024-01-01T00:00:00Z", "posterUrl": null, "collections": []
    })
}

fn by_ext(items: Vec<Value>) -> Value {
    json!({"data": {"titlesByExternalIds": items}})
}

#[tokio::test]
async fn movie_list_pages_through_catalog() {
    let (server, st) = setup("MOVIE").await;
    let page1: Vec<Value> = (0..200)
        .map(|i| {
            title(
                &format!("a{i}"),
                "L1",
                "MOVIE",
                &[("tmdb", &(i + 1).to_string())],
            )
        })
        .collect();
    let page2: Vec<Value> = (0..3)
        .map(|i| {
            title(
                &format!("b{i}"),
                "L1",
                "MOVIE",
                &[("tmdb", &(1000 + i).to_string())],
            )
        })
        .collect();
    for (offset, items, more) in [(0, page1, true), (200, page2, false)] {
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_partial_json(json!({
                "operationName": "TitlesPage",
                "variables": {"limit": 200, "offset": offset, "libraryIds": ["L1"]}
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"data": {"titles": {"items": items, "hasMore": more}}})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let (code, body) = get(&st, "/radarr/L1/api/v3/movie").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 203);
    assert_eq!(body[0]["id"], 1);
    assert_eq!(body[0]["tmdbId"], 1);
    for id in ["a0", "b2"] {
        assert_eq!(
            st.store.title_library(id).await.unwrap(),
            Some(("L1".to_owned(), None)),
            "{id}"
        );
    }
    server.verify().await;
}

#[tokio::test]
async fn series_list_skips_titles_without_tvdb() {
    let (server, st) = setup("SERIES").await;
    mock_op(
        &server,
        "TitlesPage",
        json!({"data": {"titles": {"hasMore": false, "items": [
            title("s1", "L1", "SERIES", &[("tvdb", "81189")]),
            title("s2", "L1", "SERIES", &[("anilist", "5")]),
        ]}}}),
    )
    .await;
    let (code, body) = get(&st, "/sonarr/L1/api/v3/series").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], 81189);
    assert_eq!(body[0]["tvdbId"], 81189);
    assert_eq!(body[0]["seasons"], json!([]));
    assert_eq!(body[0]["statistics"]["episodeFileCount"], 3);
}

#[tokio::test]
async fn series_list_includes_seasons_in_one_query() {
    let (server, st) = setup("SERIES").await;
    let mut s1 = title("s1", "L1", "SERIES", &[("tvdb", "81189")]);
    s1["collections"] = json!([
        {"id": "c1", "collectionType": "SEASON", "collectionIndex": "1", "monitored": false,
         "episodesOwned": 7, "episodesMonitored": 0, "episodesTotal": 7},
        {"id": "c2", "collectionType": "SEASON", "collectionIndex": "2", "monitored": true,
         "episodesOwned": 3, "episodesMonitored": 4, "episodesTotal": 10},
        {"id": "cs", "collectionType": "SPECIALS", "collectionIndex": "0", "monitored": false,
         "episodesOwned": 0, "episodesMonitored": 0, "episodesTotal": 2}
    ]);
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({"operationName": "TitlesPage"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": {"titles": {"hasMore": false, "items": [s1]}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (code, body) = get(&st, "/sonarr/L1/api/v3/series").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        body[0]["seasons"],
        json!([
            {"seasonNumber": 1, "monitored": false,
             "statistics": {"episodeFileCount": 7, "episodeCount": 0, "totalEpisodeCount": 7,
                            "sizeOnDisk": 0, "percentOfEpisodes": 0.0}},
            {"seasonNumber": 2, "monitored": true,
             "statistics": {"episodeFileCount": 3, "episodeCount": 4, "totalEpisodeCount": 10,
                            "sizeOnDisk": 0, "percentOfEpisodes": 75.0}}
        ])
    );
    assert_eq!(body[0]["statistics"]["seasonCount"], 2);
    let reqs = server.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .all(|r| !String::from_utf8_lossy(&r.body).contains("TitleDetail")),
        "no per-title detail query"
    );
    server.verify().await;
}

#[tokio::test]
async fn series_by_tvdb_includes_seasons() {
    let (server, st) = setup("SERIES").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        by_ext(vec![title("s1", "L1", "SERIES", &[("tvdb", "81189")])]),
    )
    .await;
    mock_op(
        &server,
        "TitleDetail",
        json!({"data": {"title": {
            "id": "s1", "libraryId": "L1", "name": "Title s1", "facet": "SERIES",
            "monitored": true, "year": 2020, "slug": "title-s1", "overview": "o", "tags": [],
            "externalIds": [{"source": "tvdb", "value": "81189"}],
            "imdbId": null, "qualityProfileId": null, "rootFolderPath": null,
            "minAvailability": null, "monitorType": null, "sizeBytes": 100,
            "episodesOwned": 3, "episodesMonitored": 4, "episodesTotal": 10,
            "createdAt": "2024-01-01T00:00:00Z", "posterUrl": null,
            "collections": [
                {"id": "c2", "collectionType": "SEASON", "collectionIndex": "2", "monitored": true,
                 "episodesOwned": 3, "episodesMonitored": 4, "episodesTotal": 10, "episodes": []},
                {"id": "cx", "collectionType": "ARC", "collectionIndex": "x", "monitored": true,
                 "episodesOwned": 0, "episodesMonitored": 0, "episodesTotal": 0, "episodes": []}
            ],
            "mediaFiles": []
        }}}),
    )
    .await;
    let (code, body) = get(&st, "/sonarr/L1/api/v3/series?tvdbId=81189").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(
        body[0]["seasons"],
        json!([{
            "seasonNumber": 2, "monitored": true,
            "statistics": {"episodeFileCount": 3, "episodeCount": 4, "totalEpisodeCount": 10,
                           "sizeOnDisk": 0, "percentOfEpisodes": 75.0}
        }])
    );

    let (code, body) = get(&st, "/sonarr/L1/api/v3/series/81189").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["id"], 81189);
    assert_eq!(body["seasons"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn series_by_tvdb_unknown_is_empty() {
    let (server, st) = setup("SERIES").await;
    mock_op(&server, "TitlesByExternalIds", by_ext(vec![])).await;
    let (code, body) = get(&st, "/sonarr/L1/api/v3/series?tvdbId=1").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn movie_get_returns_movie_and_filters_library() {
    let (server, st) = setup("MOVIE").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        by_ext(vec![
            title("other", "L2", "MOVIE", &[("tmdb", "603")]),
            title("m1", "L1", "MOVIE", &[("tmdb", "603")]),
        ]),
    )
    .await;
    let (code, body) = get(&st, "/radarr/L1/api/v3/movie/603").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["id"], 603);
    assert_eq!(body["title"], "Title m1");
    assert_eq!(body["hasFile"], true);
    for (id, lib) in [("m1", "L1"), ("other", "L2")] {
        assert_eq!(
            st.store.title_library(id).await.unwrap(),
            Some((lib.to_owned(), None)),
            "{id}"
        );
    }
}

#[tokio::test]
async fn movie_get_unknown_is_404() {
    let (server, st) = setup("MOVIE").await;
    mock_op(&server, "TitlesByExternalIds", by_ext(vec![])).await;
    let (code, _) = get(&st, "/radarr/L1/api/v3/movie/999").await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn movie_get_in_other_library_is_404() {
    let (server, st) = setup("MOVIE").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        by_ext(vec![title("other", "L2", "MOVIE", &[("tmdb", "603")])]),
    )
    .await;
    let (code, _) = get(&st, "/radarr/L1/api/v3/movie/603").await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn movie_lookup_existing_returns_real_id() {
    let (server, st) = setup("MOVIE").await;
    mock_op(
        &server,
        "TitlesByExternalIds",
        by_ext(vec![title("m1", "L1", "MOVIE", &[("tmdb", "603")])]),
    )
    .await;
    let (code, body) = get(&st, "/radarr/L1/api/v3/movie/lookup?term=tmdb:603").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body[0]["id"], 603);
    assert_eq!(body[0]["tmdbId"], 603);
}

#[tokio::test]
async fn movie_lookup_new_uses_metadata() {
    let (server, st) = setup("MOVIE").await;
    mock_op(&server, "TitlesByExternalIds", by_ext(vec![])).await;
    mock_op(
        &server,
        "MetadataMovie",
        json!({"data": {"metadataMovie": {
            "name": "The Matrix", "slug": "the-matrix", "year": 1999, "overview": "Neo",
            "posterUrl": "", "tmdbId": 603, "imdbId": "tt0133093", "runtimeMinutes": 136
        }}}),
    )
    .await;
    let (code, body) = get(&st, "/radarr/L1/api/v3/movie/lookup?term=tmdb:603").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body[0]["id"], 0);
    assert_eq!(body[0]["tmdbId"], 603);
    assert_eq!(body[0]["title"], "The Matrix");
    assert_eq!(body[0]["titleSlug"], "the-matrix");
    assert_eq!(body[0]["year"], 1999);
    assert_eq!(body[0]["monitored"], false);
    assert_eq!(body[0]["hasFile"], false);
}

#[tokio::test]
async fn lookup_without_prefix_is_empty() {
    let (_s, st) = setup("MOVIE").await;
    let (code, body) = get(&st, "/radarr/L1/api/v3/movie/lookup?term=matrix").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn series_lookup_new_uses_metadata() {
    let (server, st) = setup("SERIES").await;
    mock_op(&server, "TitlesByExternalIds", by_ext(vec![])).await;
    mock_op(
        &server,
        "MetadataSeries",
        json!({"data": {"metadataSeries": {
            "name": "Show", "slug": "show", "year": 2010, "overview": "x", "posterUrl": "",
            "tvdbId": "81189", "tmdbId": 1396, "network": "AMC",
            "seasons": [
                {"number": 1, "label": "S1", "episodeType": "STANDARD", "tvdbId": "1", "tmdbId": null},
                {"number": 2, "label": "S2", "episodeType": "STANDARD", "tvdbId": "2", "tmdbId": null}
            ]
        }}}),
    )
    .await;
    let (code, body) = get(&st, "/sonarr/L1/api/v3/series/lookup?term=tvdb:81189").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body[0]["id"], 0);
    assert_eq!(body[0]["tvdbId"], 81189);
    assert_eq!(body[0]["title"], "Show");
    assert_eq!(body[0]["seasons"].as_array().unwrap().len(), 2);
    assert_eq!(body[0]["seasons"][1]["seasonNumber"], 2);
    assert_eq!(body[0]["seasons"][1]["monitored"], false);
}

#[tokio::test]
async fn movie_list_stops_on_empty_page() {
    let (server, st) = setup("MOVIE").await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({"operationName": "TitlesPage"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": {"titles": {"items": [], "hasMore": true}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (code, body) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        get(&st, "/radarr/L1/api/v3/movie"),
    )
    .await
    .expect("paging stops on an empty page");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, json!([]));
    server.verify().await;
}
