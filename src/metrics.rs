use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use prometheus::{Encoder, IntCounterVec, IntGauge, Opts, Registry, TextEncoder};

use crate::AppState;
use crate::error::AppError;
use crate::scryer::SCRYER_SCHEMA_VERSION;

#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    http_requests: IntCounterVec,
    scryer_errors: IntCounterVec,
    webhook_deliveries: IntCounterVec,
    version_mismatch: IntGauge,
    warned_versions: Arc<Mutex<HashSet<String>>>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let http_requests = IntCounterVec::new(
            Opts::new("scryarr_http_requests_total", "HTTP requests served"),
            &["flavor", "status"],
        )
        .expect("valid metric");
        let scryer_errors = IntCounterVec::new(
            Opts::new("scryarr_scryer_errors_total", "Errors from scryer calls"),
            &["kind"],
        )
        .expect("valid metric");
        let webhook_deliveries = IntCounterVec::new(
            Opts::new(
                "scryarr_webhook_deliveries_total",
                "Webhook deliveries to tofa by result",
            ),
            &["result"],
        )
        .expect("valid metric");
        let version_mismatch = IntGauge::new(
            "scryarr_scryer_version_mismatch",
            "1 when scryer reports a different major.minor than the vendored schema",
        )
        .expect("valid metric");
        registry
            .register(Box::new(http_requests.clone()))
            .expect("register");
        registry
            .register(Box::new(scryer_errors.clone()))
            .expect("register");
        registry
            .register(Box::new(webhook_deliveries.clone()))
            .expect("register");
        registry
            .register(Box::new(version_mismatch.clone()))
            .expect("register");
        Self {
            registry,
            http_requests,
            scryer_errors,
            webhook_deliveries,
            version_mismatch,
            warned_versions: Default::default(),
        }
    }

    pub fn record_request(&self, flavor: &str, status: u16) {
        self.http_requests
            .with_label_values(&[flavor, &status.to_string()])
            .inc();
    }

    pub fn record_scryer_error(&self, e: &AppError) {
        let kind = match e {
            AppError::Unauthorized => "unauthorized",
            AppError::Forbidden(_) => "forbidden",
            AppError::RateLimited { .. } => "rate_limited",
            AppError::NotFound(_) => "not_found",
            AppError::Scryer { .. } => "scryer",
            AppError::Upstream(_) => "upstream",
            _ => return,
        };
        self.scryer_errors.with_label_values(&[kind]).inc();
    }

    pub fn record_delivery(&self, result: &str) {
        self.webhook_deliveries.with_label_values(&[result]).inc();
    }

    pub fn observe_scryer_version(&self, reported: &str) {
        if check_version(reported) {
            self.version_mismatch.set(0);
            return;
        }
        self.version_mismatch.set(1);
        let first = self
            .warned_versions
            .lock()
            .map(|mut s| s.insert(reported.to_string()))
            .unwrap_or(false);
        if first {
            tracing::warn!(
                reported,
                expected = SCRYER_SCHEMA_VERSION,
                "scryer version differs from the vendored schema's major.minor"
            );
        }
    }

    pub fn render(&self) -> String {
        let mut buf = Vec::new();
        TextEncoder::new()
            .encode(&self.registry.gather(), &mut buf)
            .expect("text encoding cannot fail");
        String::from_utf8(buf).unwrap_or_default()
    }
}

fn major_minor(v: &str) -> Option<(&str, &str)> {
    let mut parts = v.trim().trim_start_matches('v').split('.');
    Some((parts.next()?, parts.next()?))
}

pub fn check_version(reported: &str) -> bool {
    match (major_minor(reported), major_minor(SCRYER_SCHEMA_VERSION)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn flavor_of(path: &str) -> &'static str {
    match path.trim_start_matches('/').split('/').next() {
        Some("radarr") => "radarr",
        Some("sonarr") => "sonarr",
        Some("scryer") => "events",
        _ => "other",
    }
}

pub async fn count_requests(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let flavor = flavor_of(req.uri().path());
    let resp = next.run(req).await;
    state.metrics.record_request(flavor, resp.status().as_u16());
    resp
}

pub async fn endpoint(State(state): State<AppState>) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4"),
        )],
        state.metrics.render(),
    )
        .into_response()
}
