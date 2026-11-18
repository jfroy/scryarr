use std::sync::Arc;

use scryarr::AppState;
use scryarr::config::Config;

pub fn test_state() -> AppState {
    let config = Config::from_vars(|k| match k {
        "SCRYER_URL" => Some("http://scryer.invalid:8080".to_string()),
        "SCRYER_EVENTS_TOKEN" => Some("test-token".to_string()),
        _ => None,
    })
    .unwrap();
    AppState {
        config: Arc::new(config),
    }
}
