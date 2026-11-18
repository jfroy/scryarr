mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use scryarr::AppState;
use scryarr::domain::{Facet, Flavor};
use scryarr::events::translate::to_tofa;
use scryer_plugin_sdk::PluginNotificationRequest;
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
fn download_episode_translates_season() {
    let v = translate("download_episode", Flavor::Sonarr).unwrap();
    assert_eq!(v["eventType"], "Download");
    assert_eq!(v["series"]["tvdbId"], 81189);
    assert_eq!(v["episodeFile"]["seasonNumber"], 2);
    assert_eq!(
        v["episodeFile"]["path"],
        "/media2/library/tv/Show/Season 02/Show S02E03.mkv"
    );
}

#[test]
fn episode_file_without_season_is_none() {
    let mut req = request("download_episode");
    req.episode = None;
    assert!(to_tofa(&req, Flavor::Sonarr).is_none());
    let mut req = request("download_episode");
    req.event_type = scryer_plugin_sdk::NotificationEventType::FileDeleted;
    req.episode.as_mut().unwrap().season_number = None;
    assert!(to_tofa(&req, Flavor::Sonarr).is_none());
}

#[test]
fn multi_season_event_uses_the_files_own_season() {
    let mut req = request("download_episode");
    let file = req.media_files[0].path.clone();
    let mut a = req.episode.take().unwrap();
    a.season_number = Some("1".into());
    let mut b = a.clone();
    b.season_number = Some("3".into());
    b.media_file_path = Some(file);
    req.episodes = vec![a, b];
    let v = to_tofa(&req, Flavor::Sonarr).map(|p| serde_json::to_value(p).unwrap());
    assert_eq!(v.unwrap()["episodeFile"]["seasonNumber"], 3);

    for e in &mut req.episodes {
        e.media_file_path = None;
    }
    assert!(to_tofa(&req, Flavor::Sonarr).is_none());
}

#[test]
fn upgrade_lists_deleted_files() {
    let v = translate("upgrade_movie", Flavor::Radarr).unwrap();
    assert_eq!(v["eventType"], "Download");
    assert_eq!(v["isUpgrade"], true);
    assert_eq!(
        v["movieFile"]["path"],
        "/media2/library/movies/Heat (1995)/Heat.2160p.mkv"
    );
    assert_eq!(
        v["deletedFiles"],
        json!([{"path": "/media2/library/movies/Heat (1995)/Heat.1080p.mkv"}])
    );
}

#[test]
fn rename_pairs_paths() {
    let v = translate("rename_episode", Flavor::Sonarr).unwrap();
    assert_eq!(v["eventType"], "Rename");
    assert_eq!(
        v["renamedEpisodeFiles"],
        json!([{
            "path": "/media2/library/tv/Show/Season 02/New.mkv",
            "previousPath": "/media2/library/tv/Show/Season 02/Old.mkv",
        }])
    );
}

#[test]
fn title_deleted_series_translates() {
    let v = translate("title_deleted_series", Flavor::Sonarr).unwrap();
    assert_eq!(v["eventType"], "SeriesDelete");
    assert_eq!(v["series"]["tvdbId"], 81189);
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
    let (status, _, _) = post(&state, "test-token", "{}".into()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
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
