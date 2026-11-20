mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::scryer_mock::{libraries_response, library, mock_op};
use http_body_util::BodyExt;
use scryarr::AppState;
use scryarr::store::Notification;
use scryarr::tofa::{self, DeliveryError, MoviePayload, TofaPayload};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{body_json, body_partial_json, header, method, path};
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

async fn setup() -> AppState {
    let server = MockServer::start().await;
    mock_op(
        &server,
        "Libraries",
        libraries_response(vec![library("L1", "MOVIE", "Main", &[])]),
    )
    .await;
    let state = common::test_state_with(&server.uri()).await;
    std::mem::forget(server);
    state
}

fn registration(url: &str) -> Value {
    json!({
        "name": "Tofa",
        "implementation": "Webhook",
        "fields": [
            {"name": "url", "value": url},
            {"name": "username", "value": "tofa"},
            {"name": "password", "value": "s3cret"},
        ],
    })
}

const URI: &str = "/radarr/L1/api/v3/notification";

async fn wait_for_requests(server: &MockServer, n: usize) -> Vec<wiremock::Request> {
    for _ in 0..100 {
        let reqs = server.received_requests().await.unwrap();
        if reqs.len() >= n {
            return reqs;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("expected {n} requests");
}

#[tokio::test]
async fn post_notification_stores_and_sends_test() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(header("authorization", "Basic dG9mYTpzM2NyZXQ="))
        .and(header("x-tofa-webhook-secret", "s3cret"))
        .and(body_json(json!({"eventType": "Test"})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&tofa)
        .await;
    let state = setup().await;

    let (status, body) = send(
        &state,
        Method::POST,
        &format!("{URI}?forceSave=true"),
        Some(registration(&format!("{}/hook", tofa.uri()))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["id"].as_i64().unwrap();

    let (status, list) = send(&state, Method::GET, URI, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"].as_i64().unwrap(), id);
    wait_for_requests(&tofa, 1).await;
    tofa.verify().await;
}

fn field<'a>(body: &'a Value, name: &str) -> &'a Value {
    &body["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .unwrap()["value"]
}

#[tokio::test]
async fn notification_password_is_masked_and_kept_on_put() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(header("x-tofa-webhook-secret", "s3cret"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&tofa)
        .await;
    let state = setup().await;
    let url = format!("{}/hook", tofa.uri());
    let (status, created) = send(&state, Method::POST, URI, Some(registration(&url))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(field(&created, "password"), "********");
    let id = created["id"].as_i64().unwrap();

    let (_, list) = send(&state, Method::GET, URI, None).await;
    assert_eq!(field(&list[0], "password"), "********");
    assert_eq!(field(&list[0], "url"), url.as_str());
    assert_eq!(field(&list[0], "username"), "tofa");

    let mut echoed = list[0].clone();
    echoed["name"] = json!("Tofa (renamed)");
    let (status, updated) = send(&state, Method::PUT, &format!("{URI}/{id}"), Some(echoed)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(field(&updated, "password"), "********");
    let stored = state.store.notifications("L1").await.unwrap();
    assert_eq!(stored[0].password.as_deref(), Some("s3cret"));
    assert_eq!(stored[0].body["name"], "Tofa (renamed)");
    wait_for_requests(&tofa, 2).await;
    tofa.verify().await;
}

#[tokio::test]
async fn notification_responds_before_the_test_delivery() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(3)))
        .mount(&tofa)
        .await;
    let state = setup().await;
    let started = std::time::Instant::now();
    let (status, _) = send(
        &state,
        Method::POST,
        URI,
        Some(registration(&format!("{}/hook", tofa.uri()))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    wait_for_requests(&tofa, 1).await;
}

#[tokio::test]
async fn notification_routes_need_manage_titles() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({
            "operationName": "Libraries", "variables": {"permission": "MANAGE_TITLES"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(libraries_response(vec![])))
        .with_priority(1)
        .mount(&server)
        .await;
    mock_op(
        &server,
        "Libraries",
        libraries_response(vec![library("L1", "MOVIE", "Main", &["/m"])]),
    )
    .await;
    let state = common::test_state_with(&server.uri()).await;
    let (status, body) = send(&state, Method::GET, URI, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body["message"].is_string());
    let (status, _) = send(
        &state,
        Method::POST,
        URI,
        Some(registration("http://127.0.0.1:1/x")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(state.store.notifications("L1").await.unwrap().is_empty());
    let (status, _) = send(&state, Method::GET, "/radarr/L1/api/v3/rootfolder", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &state,
        Method::GET,
        "/radarr/L1/api/v3/languageprofile",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn post_notification_survives_unreachable_tofa() {
    let state = setup().await;
    let (status, body) = send(
        &state,
        Method::POST,
        &format!("{URI}?forceSave=true"),
        Some(registration("http://127.0.0.1:1/x")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body["id"].is_i64());
}

#[tokio::test]
async fn put_notification_updates_and_unknown_is_404() {
    let state = setup().await;
    let (_, body) = send(
        &state,
        Method::POST,
        URI,
        Some(registration("http://127.0.0.1:1/x")),
    )
    .await;
    let id = body["id"].as_i64().unwrap();

    let (status, updated) = send(
        &state,
        Method::PUT,
        &format!("{URI}/{id}"),
        Some(registration("http://127.0.0.1:1/y")),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(updated["id"].as_i64().unwrap(), id);

    let (status, _) = send(
        &state,
        Method::PUT,
        &format!("{URI}/999"),
        Some(registration("http://127.0.0.1:1/y")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_notification_removes_it() {
    let state = setup().await;
    let (_, body) = send(
        &state,
        Method::POST,
        URI,
        Some(registration("http://127.0.0.1:1/x")),
    )
    .await;
    let id = body["id"].as_i64().unwrap();

    let (status, _) = send(&state, Method::DELETE, &format!("{URI}/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = send(&state, Method::GET, URI, None).await;
    assert!(list.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn schema_is_empty_list() {
    let state = setup().await;
    let (status, body) = send(&state, Method::GET, &format!("{URI}/schema"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
}

#[test]
fn payload_skips_none_fields() {
    let p = TofaPayload {
        event_type: "Grab".into(),
        movie: Some(MoviePayload {
            tmdb_id: 1,
            folder_path: Some("/m".into()),
        }),
        ..Default::default()
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(
        v,
        json!({"eventType": "Grab", "movie": {"tmdbId": 1, "folderPath": "/m"}})
    );
    assert!(v.get("series").is_none());
}

fn notification(url: &str, password: Option<&str>) -> Notification {
    let mut fields = vec![json!({"name": "url", "value": url})];
    if let Some(p) = password {
        fields.push(json!({"name": "password", "value": p}));
    }
    Notification {
        id: 1,
        library_id: "L1".into(),
        flavor: scryarr::domain::Flavor::Radarr,
        body: json!({"fields": fields}),
        url: url.into(),
        username: None,
        password: password.map(str::to_owned),
    }
}

#[tokio::test]
async fn deliver_classifies_failures_and_empty_url() {
    let http = scryarr::http_client().build().unwrap();
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bad"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&tofa)
        .await;
    Mock::given(method("POST"))
        .and(path("/down"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&tofa)
        .await;
    let ev = TofaPayload::event("Test");

    let r = tofa::deliver(
        &http,
        &notification(&format!("{}/bad", tofa.uri()), None),
        &ev,
    )
    .await;
    assert!(matches!(
        r,
        Err(DeliveryError::Rejected(Some(s))) if s == StatusCode::UNAUTHORIZED
    ));
    let r = tofa::deliver(
        &http,
        &notification(&format!("{}/down", tofa.uri()), None),
        &ev,
    )
    .await;
    assert!(matches!(r, Err(DeliveryError::Retryable(_))));
    let r = tofa::deliver(&http, &notification("", Some("x")), &ev).await;
    assert!(matches!(r, Err(DeliveryError::Rejected(None))));
}

#[tokio::test]
async fn deliver_without_password_sends_no_auth_headers() {
    let tofa = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&tofa)
        .await;
    let http = scryarr::http_client().build().unwrap();
    let n = notification(&format!("{}/h", tofa.uri()), None);
    tofa::deliver(&http, &n, &TofaPayload::event("Test"))
        .await
        .unwrap();
    let reqs = tofa.received_requests().await.unwrap();
    assert!(reqs[0].headers.get("authorization").is_none());
    assert!(reqs[0].headers.get("x-tofa-webhook-secret").is_none());
}
