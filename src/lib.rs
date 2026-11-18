pub mod config;
pub mod domain;
pub mod error;
pub mod scryer;
pub mod store;

use std::sync::Arc;

use axum::Router;
use axum::routing::get;

use config::Config;
use scryer::ScryerClient;
use store::Store;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub scryer: ScryerClient,
    pub http: reqwest::Client,
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}
