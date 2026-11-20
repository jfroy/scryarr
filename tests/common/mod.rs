pub mod scryer_mock;

use std::sync::Arc;

use scryarr::AppState;
use scryarr::config::Config;
use scryarr::scryer::ScryerClient;
use scryarr::store::Store;

#[allow(dead_code)]
pub async fn test_state() -> AppState {
    test_state_with("http://scryer.invalid:8080").await
}

pub async fn test_state_with(scryer_url: &str) -> AppState {
    let url = scryer_url.to_string();
    let config = Config::from_vars(|k| match k {
        "SCRYER_URL" => Some(url.clone()),
        "SCRYER_EVENTS_TOKEN" => Some("test-token".to_string()),
        _ => None,
    })
    .unwrap();
    let http = scryarr::http_client().build().unwrap();
    let metrics = scryarr::metrics::Metrics::new();
    AppState {
        scryer: ScryerClient::new(&config.scryer_url, http.clone()).with_metrics(metrics.clone()),
        http,
        config: Arc::new(config),
        store: Store::open_in_memory().await.unwrap(),
        libraries: Default::default(),
        metrics,
    }
}
