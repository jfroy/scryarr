#![recursion_limit = "256"]
use scryarr::error::AppError;
use scryarr::scryer::ops::{ScryerVersion, scryer_version};
use scryarr::scryer::{ApiKey, ScryerClient};
use serde_json::json;
use url::Url;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(base: &str) -> ScryerClient {
    ScryerClient::new(
        &Url::parse(base).unwrap(),
        scryarr::http_client().build().unwrap(),
    )
}

fn op_mock() -> wiremock::MockBuilder {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({"operationName": "ScryerVersion"})))
}

async fn run(c: &ScryerClient, key: &str) -> Result<String, AppError> {
    c.run::<ScryerVersion>(&ApiKey(key.into()), scryer_version::Variables)
        .await
        .map(|d| d.scryer_version)
}

#[tokio::test]
async fn bearer_header_is_forwarded() {
    let s = MockServer::start().await;
    op_mock()
        .and(header("authorization", "Bearer k1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"scryerVersion":"0.22.4"}})),
        )
        .expect(1)
        .mount(&s)
        .await;
    assert_eq!(run(&client(&s.uri()), "k1").await.unwrap(), "0.22.4");
}

#[tokio::test]
async fn base_url_trailing_slash_is_normalised() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"scryerVersion":"0.22.4"}})),
        )
        .expect(2)
        .mount(&s)
        .await;
    assert!(run(&client(&format!("{}/", s.uri())), "k").await.is_ok());
    assert!(run(&client(&s.uri()), "k").await.is_ok());
}

#[tokio::test]
async fn http_401_maps_to_unauthorized() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;
    assert!(matches!(
        run(&client(&s.uri()), "k").await,
        Err(AppError::Unauthorized)
    ));
}

#[tokio::test]
async fn graphql_unauthorized_code_maps_to_unauthorized() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":null,"errors":[{"message":"unauthorized: x","extensions":{"code":"UNAUTHORIZED"}}]})))
        .mount(&s)
        .await;
    assert!(matches!(
        run(&client(&s.uri()), "k").await,
        Err(AppError::Unauthorized)
    ));
}

#[tokio::test]
async fn not_found_code_maps_to_not_found() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":null,"errors":[{"message":"no such title","extensions":{"code":"NOT_FOUND"}}]})))
        .mount(&s)
        .await;
    match run(&client(&s.uri()), "k").await {
        Err(AppError::NotFound(m)) => assert_eq!(m, "no such title"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn validation_error_carries_reason() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":null,"errors":[{"message":"bad","extensions":{"code":"VALIDATION_ERROR","reason":"TITLE_X"}}]})))
        .mount(&s)
        .await;
    match run(&client(&s.uri()), "k").await {
        Err(AppError::Scryer {
            code,
            reason,
            message,
        }) => {
            assert_eq!(code.as_deref(), Some("VALIDATION_ERROR"));
            assert_eq!(reason.as_deref(), Some("TITLE_X"));
            assert_eq!(message, "bad");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn http_500_maps_to_upstream() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(ResponseTemplate::new(503))
        .mount(&s)
        .await;
    assert!(matches!(
        run(&client(&s.uri()), "k").await,
        Err(AppError::Upstream(_))
    ));
}

#[tokio::test]
async fn connection_refused_maps_to_upstream() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let r = run(&client(&format!("http://127.0.0.1:{port}")), "k").await;
    assert!(matches!(r, Err(AppError::Upstream(_))));
}

#[tokio::test]
async fn title_detail_converts_to_views() {
    use scryarr::domain::Facet;
    use scryarr::scryer::{CollectionKind, TitleDetail};
    let s = MockServer::start().await;
    let t = json!({
        "id":"t1","libraryId":"l1","name":"Show","facet":"ANIME","monitored":true,"year":2020,
        "slug":null,"overview":null,"tags":[],"externalIds":[{"source":"tvdb","value":"42"}],
        "imdbId":null,"qualityProfileId":null,"rootFolderPath":"/tv","minAvailability":null,
        "monitorType":"ALL_EPISODES","sizeBytes":10,"episodesOwned":1,"episodesMonitored":2,
        "episodesTotal":2,"createdAt":"2026-01-01T00:00:00Z","posterUrl":null,
        "collections":[{"id":"c1","collectionType":"SEASON","collectionIndex":"1","monitored":true,
            "episodesOwned":1,"episodesMonitored":2,"episodesTotal":2,
            "episodes":[{"id":"e1","seasonNumber":"1","episodeNumber":"1","title":"Pilot",
                "airDate":"2020-01-01","monitored":true,"mediaFiles":[{"id":"f1","filePath":"/tv/a.mkv"}]}]}],
        "mediaFiles":[{"id":"f1","filePath":"/tv/a.mkv","episodeId":"e1","sizeBytes":123}]
    });
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(json!({"operationName": "TitleDetail"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"title":t}})))
        .mount(&s)
        .await;
    let d = client(&s.uri())
        .run::<scryarr::scryer::ops::TitleDetail>(
            &ApiKey("k".into()),
            scryarr::scryer::ops::title_detail::Variables { id: "t1".into() },
        )
        .await
        .unwrap();
    let v: TitleDetail = d.title.unwrap().into();
    assert_eq!(v.title.facet, Facet::Anime);
    assert_eq!(
        v.title.external_ids,
        vec![("tvdb".to_string(), "42".to_string())]
    );
    assert_eq!(v.title.monitor_type.as_deref(), Some("ALL_EPISODES"));
    assert_eq!(v.seasons[0].kind, CollectionKind::Season);
    assert_eq!(v.seasons[0].episodes[0].media_file_ids, vec!["f1"]);
    assert_eq!(v.media_files[0].episode_id.as_deref(), Some("e1"));
    assert_eq!(v.media_files[0].size_bytes, 123);
}

#[tokio::test]
async fn http_403_maps_to_forbidden() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(ResponseTemplate::new(403))
        .mount(&s)
        .await;
    assert!(matches!(
        run(&client(&s.uri()), "k").await,
        Err(AppError::Forbidden(_))
    ));
}

#[tokio::test]
async fn graphql_unauthorized_with_reason_maps_to_forbidden() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":null,"errors":[{
            "message":"unauthorized: You do not have access to this library",
            "extensions":{"code":"UNAUTHORIZED","reason":"AUTHORIZATION_LIBRARY_ACCESS_DENIED"}}]})))
        .mount(&s)
        .await;
    match run(&client(&s.uri()), "k").await {
        Err(AppError::Forbidden(m)) => {
            assert_eq!(m, "unauthorized: You do not have access to this library")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn http_429_maps_to_rate_limited_with_retry_after() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7"))
        .mount(&s)
        .await;
    match run(&client(&s.uri()), "k").await {
        Err(AppError::RateLimited { retry_after }) => assert_eq!(retry_after, Some(7)),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn graphql_rate_limited_maps_to_rate_limited() {
    let s = MockServer::start().await;
    op_mock()
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":null,"errors":[{
            "message":"Too many requests","extensions":{"code":"RATE_LIMITED","retryAfterSeconds":12}}]})))
        .mount(&s)
        .await;
    match run(&client(&s.uri()), "k").await {
        Err(AppError::RateLimited { retry_after }) => assert_eq!(retry_after, Some(12)),
        other => panic!("{other:?}"),
    }
    let s = MockServer::start().await;
    op_mock()
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":null,"errors":[{
            "message":"slow down","extensions":{"code":"TEMPORARY_UNAVAILABLE",
            "reason":"LIST_ACCOUNT_AUTH_RATE_LIMITED"}}]})),
        )
        .mount(&s)
        .await;
    match run(&client(&s.uri()), "k").await {
        Err(AppError::RateLimited { retry_after }) => assert_eq!(retry_after, None),
        other => panic!("{other:?}"),
    }
}
