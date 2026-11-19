use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::ManagedConn;
use crate::AppState;
use crate::error::AppError;
use crate::store::Notification;
use crate::tofa::{self, DeliveryError, TofaPayload};

/// What Radarr and Sonarr show in place of a stored secret.
const MASK: &str = "********";

async fn send_test(state: &AppState, n: &Notification) {
    match tofa::deliver(&state.http, n, &TofaPayload::event("Test")).await {
        Ok(()) => state.metrics.record_delivery("ok"),
        Err(e) => {
            state.metrics.record_delivery(match e {
                DeliveryError::Retryable(_) => "retryable",
                DeliveryError::Rejected(_) => "rejected",
            });
            tracing::warn!(notification_id = n.id, url = %n.url, error = %e, "tofa test delivery failed");
        }
    }
}

/// tofa saves the registration inside its own request, so the Test delivery
/// back to tofa must not hold that request open.
fn spawn_test(state: &AppState, n: &Notification) {
    let (state, n) = (state.clone(), n.clone());
    tokio::spawn(async move { send_test(&state, &n).await });
}

fn password_fields(body: &mut Value) -> impl Iterator<Item = &mut Value> {
    body.get_mut("fields")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter(|f| f.get("name").and_then(Value::as_str) == Some("password"))
}

fn masked(mut body: Value) -> Value {
    for f in password_fields(&mut body) {
        if f.get("value")
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty())
        {
            f["value"] = Value::from(MASK);
        }
    }
    body
}

fn unmasked(mut body: Value, stored: Option<&str>) -> Value {
    let Some(stored) = stored else {
        return body;
    };
    for f in password_fields(&mut body) {
        if f.get("value").and_then(Value::as_str) == Some(MASK) {
            f["value"] = Value::from(stored);
        }
    }
    body
}

pub async fn list(
    State(state): State<AppState>,
    ManagedConn(conn): ManagedConn,
) -> Result<Json<Vec<Value>>, AppError> {
    let items = state.store.notifications(&conn.library_id).await?;
    Ok(Json(items.into_iter().map(|n| masked(n.body)).collect()))
}

pub async fn schema() -> Json<Vec<Value>> {
    Json(vec![])
}

pub async fn create(
    State(state): State<AppState>,
    ManagedConn(conn): ManagedConn,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let n = state
        .store
        .insert_notification(&conn.library_id, conn.flavor, body)
        .await?;
    spawn_test(&state, &n);
    Ok((StatusCode::CREATED, Json(masked(n.body))))
}

pub async fn update(
    State(state): State<AppState>,
    ManagedConn(conn): ManagedConn,
    Path((_, id)): Path<(String, i32)>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let not_found = || AppError::NotFound("notification not found".into());
    let stored = state
        .store
        .notification(&conn.library_id, id)
        .await?
        .ok_or_else(not_found)?;
    let body = unmasked(body, stored.password.as_deref());
    let n = state
        .store
        .update_notification(&conn.library_id, id, body)
        .await?
        .ok_or_else(not_found)?;
    spawn_test(&state, &n);
    Ok((StatusCode::ACCEPTED, Json(masked(n.body))))
}

pub async fn delete(
    State(state): State<AppState>,
    ManagedConn(conn): ManagedConn,
    Path((_, id)): Path<(String, i32)>,
) -> Result<Json<Value>, AppError> {
    if state
        .store
        .delete_notification(&conn.library_id, id)
        .await?
    {
        Ok(Json(json!({})))
    } else {
        Err(AppError::NotFound("notification not found".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_round_trips_only_the_mask() {
        let body = json!({"fields": [
            {"name": "url", "value": "http://t"},
            {"name": "password", "value": "s3cret"}
        ]});
        let m = masked(body.clone());
        assert_eq!(m["fields"][1]["value"], MASK);
        assert_eq!(m["fields"][0]["value"], "http://t");
        assert_eq!(unmasked(m, Some("s3cret")), body);
        let fresh = json!({"fields": [{"name": "password", "value": "new"}]});
        assert_eq!(unmasked(fresh.clone(), Some("s3cret")), fresh);
        let empty = json!({"fields": [{"name": "password", "value": ""}]});
        assert_eq!(masked(empty.clone()), empty);
    }
}
