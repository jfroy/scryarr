use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use serde::Serialize;

use crate::store::Notification;

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TofaPayload {
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie: Option<MoviePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<SeriesPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes: Option<Vec<EpisodePayload>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_file: Option<FilePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_file: Option<EpisodeFilePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_files: Option<Vec<FilePayload>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed_movie_files: Option<Vec<RenamedFilePayload>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed_episode_files: Option<Vec<RenamedFilePayload>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_upgrade: Option<bool>,
}

impl TofaPayload {
    pub fn event(event_type: &str) -> Self {
        TofaPayload {
            event_type: event_type.to_owned(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoviePayload {
    pub tmdb_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesPayload {
    pub tvdb_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodePayload {
    pub season_number: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePayload {
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeFilePayload {
    pub path: String,
    pub season_number: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamedFilePayload {
    pub path: String,
    pub previous_path: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("retryable delivery failure: {0}")]
    Retryable(String),
    #[error("{}", match .0 {
        Some(s) => format!("tofa rejected the webhook: {s}"),
        None => "notification has no usable url".to_owned(),
    })]
    Rejected(Option<StatusCode>),
}

pub async fn deliver(
    http: &reqwest::Client,
    n: &Notification,
    payload: &TofaPayload,
) -> Result<(), DeliveryError> {
    let url = match reqwest::Url::parse(&n.url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => u,
        _ => {
            tracing::warn!(notification_id = n.id, "notification has no usable url");
            return Err(DeliveryError::Rejected(None));
        }
    };
    let mut req = http.post(url).timeout(TIMEOUT).json(payload);
    if let Some(password) = &n.password {
        let creds = format!("{}:{}", n.username.as_deref().unwrap_or(""), password);
        req = req
            .header("authorization", format!("Basic {}", STANDARD.encode(creds)))
            .header("x-tofa-webhook-secret", password);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| DeliveryError::Retryable(e.without_url().to_string()))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else if status.is_server_error() {
        Err(DeliveryError::Retryable(format!("status {status}")))
    } else {
        Err(DeliveryError::Rejected(Some(status)))
    }
}
