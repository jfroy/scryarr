mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use scryarr::AppState;
use scryarr::domain::{Facet, Flavor};
use scryarr::events::translate::to_tofa;
use scryer_plugin_sdk::{
    NotificationEventType, NotificationMediaUpdateType, PluginNotificationMediaUpdate,
    PluginNotificationRequest,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/scryer_events/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn request(name: &str) -> PluginNotificationRequest {
    serde_json::from_str(&fixture(name)).unwrap()
}

fn translate(name: &str, flavor: Flavor) -> Option<Value> {
    to_tofa(&request(name), flavor).map(|p| serde_json::to_value(p).unwrap())
}

#[test]
fn grab_movie_translates() {
    let v = translate("grab_movie", Flavor::Radarr).unwrap();
    assert_eq!(v["eventType"], "Grab");
    assert_eq!(v["movie"]["tmdbId"], 949);
    assert_eq!(
        v["movie"]["folderPath"],
        "/media2/library/movies/Heat (1995)"
    );
}

#[test]
fn movie_event_without_tmdb_id_is_none() {
    let mut req = request("grab_movie");
    req.title.as_mut().unwrap().external_ids.tmdb_id = None;
    assert!(to_tofa(&req, Flavor::Radarr).is_none());
}

#[test]
fn import_complete_episode_uses_library_path_and_season() {
    let v = translate("import_complete_episode", Flavor::Sonarr).unwrap();
    assert_eq!(v["eventType"], "Download");
    assert_eq!(v["series"]["tvdbId"], 81189);
    assert_eq!(v["series"]["path"], "/media2/library/tv/Show");
    assert_eq!(
        v["episodeFile"],
        json!({"path": "/media2/library/tv/Show/Season 02/Show - S02E03.mkv", "seasonNumber": 2})
    );
    assert!(v.get("isUpgrade").is_none());
    assert!(v.get("deletedFiles").is_none());
}

#[test]
fn episode_file_without_season_is_none() {
    let mut req = request("import_complete_episode");
    req.episode = None;
    req.episodes.clear();
    assert!(to_tofa(&req, Flavor::Sonarr).is_none());

    let mut req = request("file_deleted_episode");
    req.episodes[0].season_number = None;
    assert!(to_tofa(&req, Flavor::Sonarr).is_none());
}

#[test]
fn multi_season_event_uses_the_files_own_season() {
    let mut req = request("import_complete_episode");
    let file = req.file.as_ref().unwrap().primary_path.clone();
    let mut a = req.episodes[0].clone();
    a.season_number = Some("1".into());
    a.media_file_path = None;
    let mut b = req.episodes[0].clone();
    b.season_number = Some("3".into());
    b.media_file_path = file;
    req.episode = None;
    req.episodes = vec![a, b];
    let v = to_tofa(&req, Flavor::Sonarr).map(|p| serde_json::to_value(p).unwrap());
    assert_eq!(v.unwrap()["episodeFile"]["seasonNumber"], 3);

    for e in &mut req.episodes {
        e.media_file_path = None;
    }
    assert!(to_tofa(&req, Flavor::Sonarr).is_none());
}

#[test]
fn download_failure_is_not_relayed() {
    assert!(!scryarr::events::translate::relayed(
        NotificationEventType::Download
    ));
    let mut req = request("download_failed_episode");
    assert!(to_tofa(&req, Flavor::Sonarr).is_none());
    req.file = request("import_complete_episode").file;
    req.media_files = request("import_complete_episode").media_files;
    assert!(to_tofa(&req, Flavor::Sonarr).is_none());
}

#[test]
fn upgrade_reports_new_file_and_replaced_library_path() {
    let v = translate("upgrade_movie", Flavor::Radarr).unwrap();
    assert_eq!(v["eventType"], "Download");
    assert_eq!(v["isUpgrade"], true);
    assert_eq!(
        v["movieFile"]["path"],
        "/media2/library/movies/Heat (1995)/Heat (1995) - 2160p.mkv"
    );
    assert_eq!(
        v["deletedFiles"],
        json!([{"path": "/media2/library/movies/Heat (1995)/Heat (1995) - 1080p.mkv"}])
    );
}

#[test]
fn upgrade_without_primary_path_skips_the_previous_file() {
    let mut req = request("upgrade_movie");
    req.file.as_mut().unwrap().primary_path = None;
    let v = serde_json::to_value(to_tofa(&req, Flavor::Radarr).unwrap()).unwrap();
    assert_eq!(
        v["movieFile"]["path"],
        "/media2/library/movies/Heat (1995)/Heat (1995) - 2160p.mkv"
    );
    req.file = None;
    let v = serde_json::to_value(to_tofa(&req, Flavor::Radarr).unwrap()).unwrap();
    assert_eq!(
        v["movieFile"]["path"],
        "/media2/library/movies/Heat (1995)/Heat (1995) - 2160p.mkv"
    );
    assert_eq!(v["deletedFiles"], json!([]));
}

#[test]
fn upgrade_in_place_deletes_nothing() {
    let mut req = request("upgrade_movie");
    let path = "/media2/library/movies/Heat (1995)/Heat (1995).mkv";
    let file = req.file.as_mut().unwrap();
    file.primary_path = Some(path.into());
    file.media_updates = vec![PluginNotificationMediaUpdate {
        path: path.into(),
        update_type: NotificationMediaUpdateType::Modified,
    }];
    let v = serde_json::to_value(to_tofa(&req, Flavor::Radarr).unwrap()).unwrap();
    assert_eq!(v["movieFile"]["path"], path);
    assert_eq!(v["deletedFiles"], json!([]));
}

#[test]
fn upgrade_episode_keys_season_on_the_new_file() {
    let v = translate("upgrade_episode", Flavor::Sonarr).unwrap();
    assert_eq!(v["isUpgrade"], true);
    assert_eq!(
        v["episodeFile"],
        json!({"path": "/media2/library/tv/Show/Season 02/Show - S02E03 [2160p].mkv", "seasonNumber": 2})
    );
    assert_eq!(
        v["deletedFiles"],
        json!([{"path": "/media2/library/tv/Show/Season 02/Show - S02E03 [1080p].mkv"}])
    );
}

#[test]
fn rename_pairs_library_paths() {
    let v = translate("rename_episode", Flavor::Sonarr).unwrap();
    assert_eq!(v["eventType"], "Rename");
    assert_eq!(
        v["renamedEpisodeFiles"],
        json!([
            {
                "path": "/media2/library/tv/Show/Season 02/Show - S02E01 - Seven Thirty-Seven.mkv",
                "previousPath": "/media2/library/tv/Show/Season 02/Show.S02E01.mkv",
            },
            {
                "path": "/media2/library/tv/Show/Season 02/Show - S02E02 - Grilled.mkv",
                "previousPath": "/media2/library/tv/Show/Season 02/Show.S02E02.mkv",
            },
        ])
    );
}

#[test]
fn file_deleted_episode_reports_the_deleted_path() {
    let v = translate("file_deleted_episode", Flavor::Sonarr).unwrap();
    assert_eq!(v["eventType"], "EpisodeFileDelete");
    assert_eq!(
        v["episodeFile"],
        json!({"path": "/media2/library/tv/Show/Season 02/Show - S02E03.mkv", "seasonNumber": 2})
    );
}

#[test]
fn grab_series_lists_episode_seasons() {
    let mut req = request("import_complete_episode");
    req.event_type = NotificationEventType::Grab;
    let v = serde_json::to_value(to_tofa(&req, Flavor::Sonarr).unwrap()).unwrap();
    assert_eq!(v["eventType"], "Grab");
    assert_eq!(v["episodes"], json!([{"seasonNumber": 2}]));
}

#[test]
fn title_deleted_series_translates() {
    let v = translate("title_deleted_series", Flavor::Sonarr).unwrap();
    assert_eq!(v["eventType"], "SeriesDelete");
    assert_eq!(v["series"], json!({"tvdbId": 81189}));
}

#[test]
fn title_added_movie_translates_identity_only() {
    let v = translate("title_added_movie", Flavor::Radarr).unwrap();
    assert_eq!(v["eventType"], "MovieAdded");
    assert_eq!(v["movie"], json!({"tmdbId": 949}));
}

#[test]
fn import_complete_for_a_single_upgrade_is_left_to_the_upgrade_event() {
    let mut req = request("upgrade_movie");
    req.event_type = NotificationEventType::ImportComplete;
    assert!(to_tofa(&req, Flavor::Radarr).is_none());

    // A batch flags `upgrade` when any one file upgraded; its new files have
    // no `upgrade` event of their own.
    req.import.as_mut().unwrap().imported_count = Some(3);
    assert!(to_tofa(&req, Flavor::Radarr).is_some());

    req.import.as_mut().unwrap().imported_count = Some(1);
    req.import.as_mut().unwrap().upgrade = false;
    assert!(to_tofa(&req, Flavor::Radarr).is_some());

    req.event_type = NotificationEventType::Upgrade;
    req.import.as_mut().unwrap().upgrade = true;
    assert!(to_tofa(&req, Flavor::Radarr).is_some());
}

#[test]
fn unmapped_event_is_none() {
    assert!(translate("subtitle_downloaded", Flavor::Radarr).is_none());
}

async fn post(state: &AppState, token: &str, body: String) -> (StatusCode, Option<String>, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/scryer/events/{token}"))
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let resp = scryarr::app(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let retry = resp
        .headers()
        .get("retry-after")
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        retry,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn register(state: &AppState, url: &str) {
    state
        .store
        .set_library_roots("L1", Facet::Movie, &["/media2/library/movies".to_string()])
        .await
        .unwrap();
    state
        .store
        .insert_notification(
            "L1",
            Flavor::Radarr,
            json!({"fields": [{"name": "url", "value": url}]}),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn wrong_token_is_404() {
    let state = common::test_state().await;
    let (status, _, _) = post(&state, "nope", fixture("grab_movie")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_body_is_400() {
    let state = common::test_state().await;
    for body in ["{}", "not json", r#"{"event_type": 3}"#] {
        let (status, _, _) = post(&state, "test-token", body.into()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let mut grab: Value = serde_json::from_str(&fixture("grab_movie")).unwrap();
    grab.as_object_mut().unwrap().remove("app");
    let (status, _, _) = post(&state, "test-token", grab.to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

async fn dropped_count(state: &AppState) -> String {
    state
        .metrics
        .render()
        .lines()
        .find(|l| l.starts_with(r#"scryarr_webhook_deliveries_total{result="dropped"}"#))
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn unknown_or_unmapped_event_is_acknowledged_and_dropped() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&tofa)
        .await;
    let state = common::test_state().await;
    register(&state, &format!("{}/hook", tofa.uri())).await;

    let unknown = json!({"event_type": "something_new", "whatever": [1, 2, 3]});
    let (status, _, body) = post(&state, "test-token", unknown.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], 0);

    let mut subtitle: Value = serde_json::from_str(&fixture("subtitle_downloaded")).unwrap();
    subtitle.as_object_mut().unwrap().remove("app");
    let (status, _, body) = post(&state, "test-token", subtitle.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], 0);

    let (status, _, body) = post(&state, "test-token", fixture("download_failed_episode")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], 0);

    assert_eq!(
        dropped_count(&state).await,
        r#"scryarr_webhook_deliveries_total{result="dropped"} 3"#
    );
    tofa.verify().await;
}

#[tokio::test]
async fn event_without_registration_is_acknowledged() {
    let state = common::test_state().await;
    let (status, _, body) = post(&state, "test-token", fixture("grab_movie")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], 0);
}

#[tokio::test]
async fn event_is_delivered_to_registered_tofa() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(body_partial_json(
            json!({"eventType": "Grab", "movie": {"tmdbId": 949}}),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&tofa)
        .await;
    let state = common::test_state().await;
    register(&state, &format!("{}/hook", tofa.uri())).await;

    let (status, _, body) = post(&state, "test-token", fixture("grab_movie")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], 1);
}

#[tokio::test]
async fn unreachable_tofa_returns_503() {
    let state = common::test_state().await;
    register(&state, "http://127.0.0.1:1/hook").await;

    let (status, retry, _) = post(&state, "test-token", fixture("grab_movie")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(retry.as_deref(), Some("30"));
}

#[tokio::test]
async fn test_event_fans_out_to_every_registration() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(body_partial_json(json!({"eventType": "Test"})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&tofa)
        .await;
    let state = common::test_state().await;
    register(&state, &format!("{}/hook", tofa.uri())).await;

    let body = json!({
        "event_type": "test",
        "summary_title": "t",
        "summary_message": "t",
        "app": {"name": "scryer", "version": "0.22.4"},
        "is_test": true,
    });
    let (status, _, body) = post(&state, "test-token", body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], 1);
}

async fn register_at(state: &AppState, library: &str, flavor: Flavor, url: &str) {
    state
        .store
        .insert_notification(
            library,
            flavor,
            json!({"fields": [{"name": "url", "value": url}]}),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn title_deleted_without_path_is_routed_by_title_id() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/tv"))
        .and(body_partial_json(json!({
            "eventType": "SeriesDelete",
            "series": {"tvdbId": 81189, "path": "/media2/library/tv/Show"}
        })))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&tofa)
        .await;
    let state = common::test_state().await;
    register_at(&state, "TV", Flavor::Sonarr, &format!("{}/tv", tofa.uri())).await;
    state
        .store
        .remember_title_path("t-show", "TV", "/media2/library/tv/Show")
        .await
        .unwrap();

    let (status, _, body) = post(&state, "test-token", fixture("title_deleted_series")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], 1);
    assert_eq!(state.store.title_library("t-show").await.unwrap(), None);
    tofa.verify().await;
}

#[tokio::test]
async fn title_added_goes_only_to_the_titles_library() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/4k"))
        .and(body_partial_json(
            json!({"eventType": "MovieAdded", "movie": {"tmdbId": 949}}),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&tofa)
        .await;
    Mock::given(method("POST"))
        .and(path("/hd"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&tofa)
        .await;
    let state = common::test_state().await;
    register_at(&state, "HD", Flavor::Radarr, &format!("{}/hd", tofa.uri())).await;
    register_at(&state, "4K", Flavor::Radarr, &format!("{}/4k", tofa.uri())).await;
    state
        .store
        .remember_titles(&[("t-heat", "4K"), ("t-other", "HD")])
        .await
        .unwrap();

    let (status, _, body) = post(&state, "test-token", fixture("title_added_movie")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], 1);
    tofa.verify().await;
}

#[tokio::test]
async fn title_event_with_unknown_id_is_dropped() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&tofa)
        .await;
    let state = common::test_state().await;
    register(&state, &format!("{}/hook", tofa.uri())).await;
    register_at(
        &state,
        "TV",
        Flavor::Sonarr,
        &format!("{}/hook", tofa.uri()),
    )
    .await;
    state
        .store
        .remember_titles(&[("t-elsewhere", "L1")])
        .await
        .unwrap();

    for name in ["title_added_movie", "title_deleted_series"] {
        let (status, _, body) = post(&state, "test-token", fixture(name)).await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(body["delivered"], 0, "{name}");
    }
    assert_eq!(
        dropped_count(&state).await,
        r#"scryarr_webhook_deliveries_total{result="dropped"} 2"#
    );
    tofa.verify().await;
}

#[tokio::test]
async fn path_routed_event_records_the_titles_library_and_folder() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&tofa)
        .await;
    let state = common::test_state().await;
    register(&state, &format!("{}/hook", tofa.uri())).await;

    let (status, _, body) = post(&state, "test-token", fixture("grab_movie")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["delivered"], 1);
    assert_eq!(
        state.store.title_library("t-heat").await.unwrap(),
        Some((
            "L1".to_owned(),
            Some("/media2/library/movies/Heat (1995)".to_owned())
        ))
    );
}
