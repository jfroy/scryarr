pub mod translate;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use scryer_plugin_sdk::{NotificationEventType, PluginNotificationRequest};
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::AppState;
use crate::domain::Facet;
use crate::store::Notification;
use crate::tofa::{self, DeliveryError};

pub fn router() -> Router<AppState> {
    Router::new().route("/scryer/events/{token}", post(ingest))
}

fn acknowledged(delivered: usize) -> Response {
    Json(json!({ "delivered": delivered })).into_response()
}

fn dropped(state: &AppState) -> Response {
    state.metrics.record_delivery("dropped");
    acknowledged(0)
}

fn parse_facet(s: &str) -> Option<Facet> {
    match s.to_ascii_uppercase().as_str() {
        "MOVIE" => Some(Facet::Movie),
        "SERIES" => Some(Facet::Series),
        "ANIME" => Some(Facet::Anime),
        _ => None,
    }
}

async fn ingest(State(state): State<AppState>, Path(token): Path<String>, body: Bytes) -> Response {
    let expected = state.config.events_token.as_bytes();
    if !bool::from(token.as_bytes().ct_eq(expected)) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(req) = serde_json::from_slice::<PluginNotificationRequest>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let targets: Vec<Notification> = if req.event_type == NotificationEventType::Test {
        match state.store.all_notifications().await {
            Ok(n) => n,
            Err(e) => return internal(e),
        }
    } else {
        let Some(title) = &req.title else {
            return dropped(&state);
        };
        let (Some(facet), Some(path)) = (parse_facet(&title.facet), title.path.as_deref()) else {
            return dropped(&state);
        };
        let library = match state.store.library_for_path(facet, path).await {
            Ok(Some(l)) => l,
            Ok(None) => return dropped(&state),
            Err(e) => return internal(e),
        };
        match state.store.notifications(&library).await {
            Ok(n) => n,
            Err(e) => return internal(e),
        }
    };

    let mut delivered = 0;
    let mut retryable = false;
    for n in &targets {
        let Some(payload) = translate::to_tofa(&req, n.flavor) else {
            state.metrics.record_delivery("dropped");
            continue;
        };
        match tofa::deliver(&state.http, n, &payload).await {
            Ok(()) => {
                delivered += 1;
                state.metrics.record_delivery("ok");
            }
            Err(DeliveryError::Retryable(e)) => {
                tracing::warn!(notification_id = n.id, error = %e, "tofa delivery failed, will be retried");
                retryable = true;
                state.metrics.record_delivery("retryable");
            }
            Err(DeliveryError::Rejected(status)) => {
                tracing::warn!(notification_id = n.id, ?status, "tofa rejected event");
                state.metrics.record_delivery("rejected");
            }
        }
    }

    if retryable {
        let mut resp = StatusCode::SERVICE_UNAVAILABLE.into_response();
        resp.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
        return resp;
    }
    acknowledged(delivered)
}

fn internal(e: sqlx::Error) -> Response {
    tracing::error!(error = %e, "event ingest store failure");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}
