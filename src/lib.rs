pub mod arr;
pub mod config;
pub mod domain;
pub mod error;
pub mod events;
pub mod metrics;
pub mod scryer;
pub mod store;
pub mod tofa;

use std::sync::Arc;

use axum::routing::get;
use axum::{Extension, Router};
use domain::Flavor;

use config::Config;
use scryer::ScryerClient;
use store::Store;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub scryer: ScryerClient,
    pub http: reqwest::Client,
    pub libraries: arr::LibraryCache,
    pub metrics: metrics::Metrics,
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics::endpoint))
        .merge(events::router())
        .nest(
            "/radarr/{library_id}/api/v3",
            arr::router().layer(Extension(Flavor::Radarr)),
        )
        .nest(
            "/sonarr/{library_id}/api/v3",
            arr::router().layer(Extension(Flavor::Sonarr)),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            metrics::count_requests,
        ))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}
