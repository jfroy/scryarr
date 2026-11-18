use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::Conn;
use crate::AppState;
use crate::error::AppError;
use crate::store::Notification;
use crate::tofa::{self, DeliveryError, TofaPayload};

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

pub async fn list(State(state): State<AppState>, conn: Conn) -> Result<Json<Vec<Value>>, AppError> {
    let items = state.store.notifications(&conn.library_id).await?;
    Ok(Json(items.into_iter().map(|n| n.body).collect()))
}

pub async fn schema() -> Json<Vec<Value>> {
    Json(vec![])
}

pub async fn create(
    State(state): State<AppState>,
    conn: Conn,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let n = state
        .store
        .insert_notification(&conn.library_id, conn.flavor, body)
        .await?;
    send_test(&state, &n).await;
    Ok((StatusCode::CREATED, Json(n.body)))
}

pub async fn update(
    State(state): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let n = state
        .store
        .update_notification(&conn.library_id, id, body)
        .await?
        .ok_or_else(|| AppError::NotFound("notification not found".into()))?;
    send_test(&state, &n).await;
    Ok((StatusCode::ACCEPTED, Json(n.body)))
}

pub async fn delete(
    State(state): State<AppState>,
    conn: Conn,
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
