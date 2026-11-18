pub mod config;
pub mod domain;
pub mod error;

use std::sync::Arc;

use axum::Router;
use axum::routing::get;

use config::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}
