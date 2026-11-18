use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("scryer error: {message}")]
    Scryer {
        code: Option<String>,
        reason: Option<String>,
        message: String,
    },
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

fn validation_body(message: &str) -> serde_json::Value {
    json!([{ "propertyName": "", "errorMessage": message, "severity": "error" }])
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Unauthorized" })),
            )
                .into_response(),
            AppError::NotFound(m) => {
                (StatusCode::NOT_FOUND, Json(json!({ "message": m }))).into_response()
            }
            AppError::BadRequest(m) => {
                (StatusCode::BAD_REQUEST, Json(validation_body(&m))).into_response()
            }
            AppError::Scryer {
                reason, message, ..
            } => {
                let m = match reason {
                    Some(r) => format!("{r}: {message}"),
                    None => message,
                };
                (StatusCode::BAD_REQUEST, Json(validation_body(&m))).into_response()
            }
            AppError::Internal(m) => {
                tracing::error!(error = %m, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "message": "internal error" })),
                )
                    .into_response()
            }
            AppError::Upstream(m) => {
                (StatusCode::BAD_GATEWAY, Json(json!({ "message": m }))).into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn parts(e: AppError) -> (StatusCode, serde_json::Value) {
        let r = e.into_response();
        let s = r.status();
        let b = r.into_body().collect().await.unwrap().to_bytes();
        (s, serde_json::from_slice(&b).unwrap())
    }

    #[tokio::test]
    async fn status_and_bodies() {
        let (s, b) = parts(AppError::Unauthorized).await;
        assert_eq!(
            (s, b),
            (StatusCode::UNAUTHORIZED, json!({"error":"Unauthorized"}))
        );
        let (s, b) = parts(AppError::NotFound("x".into())).await;
        assert_eq!((s, b), (StatusCode::NOT_FOUND, json!({"message":"x"})));
        let (s, b) = parts(AppError::BadRequest("x".into())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(
            b,
            json!([{"propertyName":"","errorMessage":"x","severity":"error"}])
        );
        let (s, b) = parts(AppError::Upstream("x".into())).await;
        assert_eq!((s, b), (StatusCode::BAD_GATEWAY, json!({"message":"x"})));
        let (s, b) = parts(AppError::Scryer {
            code: None,
            reason: Some("r".into()),
            message: "m".into(),
        })
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(b[0]["errorMessage"], "r: m");
    }
}
