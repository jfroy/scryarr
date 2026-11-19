pub mod translate;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use scryer_plugin_sdk::{
    NotificationEventType, PluginNotificationRequest, PluginNotificationTitle,
};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use crate::AppState;
use crate::domain::Facet;
use crate::store::{Notification, Store};
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
    let Ok(raw) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(event_type) = raw
        .get("event_type")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(kind) =
        serde_json::from_value::<NotificationEventType>(Value::from(event_type.as_str()))
    else {
        tracing::debug!(event_type, "unknown scryer event type dropped");
        return dropped(&state);
    };
    if !translate::relayed(kind) {
        return dropped(&state);
    }
    let mut req = match serde_json::from_value::<PluginNotificationRequest>(raw) {
        Ok(req) => req,
        Err(e) => {
            tracing::warn!(event_type, error = %e, "malformed scryer event");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let targets: Vec<Notification> = if req.event_type == NotificationEventType::Test {
        match state.store.all_notifications().await {
            Ok(n) => n,
            Err(e) => return internal(e),
        }
    } else {
        let Some(title) = req.title.as_mut() else {
            return dropped(&state);
        };
        let Some(facet) = parse_facet(&title.facet) else {
            return dropped(&state);
        };
        let library = match route(&state.store, facet, title).await {
            Ok(Some(l)) => l,
            Ok(None) => {
                tracing::debug!(
                    event_type,
                    title_id = title.id.as_deref(),
                    "scryer event matches no library; dropped"
                );
                return dropped(&state);
            }
            Err(e) => return internal(e),
        };
        if req.event_type == NotificationEventType::TitleDeleted
            && let Some(id) = title.id.as_deref()
            && let Err(e) = state.store.forget_title(id).await
        {
            tracing::warn!(title_id = id, error = %e, "failed to forget deleted title");
        }
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

/// Picks the library an event belongs to: by root-folder prefix of the title's
/// folder, else by the title id the bridge last saw the title under. scryer
/// sends `title_added` (the folder does not exist yet) and `title_deleted` (the
/// row is already gone) without a path, so those take the second route.
///
/// Routing by facet alone would be wrong: an HD and a 4K Radarr connection can
/// both hold the same TMDB id, and both would get the event.
///
/// A path-routed event teaches the bridge the title's folder; an id-routed one
/// without a path gets that folder back, so a delete still carries
/// `folderPath` / `path`.
async fn route(
    store: &Store,
    facet: Facet,
    title: &mut PluginNotificationTitle,
) -> Result<Option<String>, sqlx::Error> {
    if let Some(path) = title.path.as_deref()
        && let Some(library) = store.library_for_path(facet, path).await?
    {
        if let Some(id) = title.id.as_deref()
            && let Err(e) = store.remember_title_path(id, &library, path).await
        {
            tracing::warn!(title_id = id, error = %e, "failed to record title library");
        }
        return Ok(Some(library));
    }
    let Some(id) = title.id.as_deref() else {
        return Ok(None);
    };
    let Some((library, path)) = store.title_library(id).await? else {
        return Ok(None);
    };
    if title.path.is_none() {
        title.path = path;
    }
    Ok(Some(library))
}

fn internal(e: sqlx::Error) -> Response {
    tracing::error!(error = %e, "event ingest store failure");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}
