use std::sync::Arc;

use scryarr::config::Config;
use scryarr::scryer::ScryerClient;
use scryarr::store::Store;
use scryarr::{AppState, app};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    if let Err(e) = std::fs::create_dir_all(&config.data_dir) {
        tracing::error!(error = %e, dir = %config.data_dir.display(), "cannot create data dir");
        std::process::exit(1);
    }
    let db_path = config.data_dir.join("state.db");
    let store = match Store::open(&db_path).await {
        Ok(store) => store,
        Err(e) => {
            tracing::error!(error = %e, path = %db_path.display(), "cannot open state database");
            std::process::exit(1);
        }
    };

    let listener = TcpListener::bind(config.listen).await?;
    tracing::info!(listen = %config.listen, "scryarr starting");

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let metrics = scryarr::metrics::Metrics::new();
    let state = AppState {
        scryer: ScryerClient::new(&config.scryer_url, http.clone()).with_metrics(metrics.clone()),
        http,
        config: Arc::new(config),
        store,
        libraries: Default::default(),
        metrics,
    };
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    tracing::info!("shutting down");
}
