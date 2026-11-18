pub mod ops;
mod views;

use graphql_client::{GraphQLQuery, Response};
use url::Url;

use crate::error::AppError;

pub use views::{CollectionKind, EpisodeView, MediaFile, Season, Title, TitleDetail};

pub const SCRYER_SCHEMA_VERSION: &str = "0.22.4";

#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(pub String);

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

#[derive(Clone)]
pub struct ScryerClient {
    endpoint: Url,
    http: reqwest::Client,
}

impl ScryerClient {
    pub fn new(base: &Url, http: reqwest::Client) -> Self {
        let endpoint = Url::parse(&format!("{}/graphql", base.as_str().trim_end_matches('/')))
            .expect("base url plus /graphql is a valid url");
        Self { endpoint, http }
    }

    pub async fn run<Q: GraphQLQuery>(
        &self,
        key: &ApiKey,
        vars: Q::Variables,
    ) -> Result<Q::ResponseData, AppError> {
        let resp = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(&key.0)
            .json(&Q::build_query(vars))
            .send()
            .await
            .map_err(|e| {
                AppError::Upstream(format!("scryer request failed: {}", e.without_url()))
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::Unauthorized);
        }
        if status.is_server_error() {
            return Err(AppError::Upstream(format!("scryer returned HTTP {status}")));
        }
        let body: Response<Q::ResponseData> = resp.json().await.map_err(|e| {
            AppError::Upstream(format!(
                "scryer returned HTTP {status} with an unreadable body: {}",
                e.without_url()
            ))
        })?;

        if let Some(first) = body.errors.as_ref().and_then(|e| e.first()) {
            let ext = |k: &str| {
                first
                    .extensions
                    .as_ref()
                    .and_then(|m| m.get(k))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            };
            let code = ext("code");
            return Err(match code.as_deref() {
                Some("UNAUTHORIZED") => AppError::Unauthorized,
                Some("NOT_FOUND") => AppError::NotFound(first.message.clone()),
                _ => AppError::Scryer {
                    code,
                    reason: ext("reason"),
                    message: first.message.clone(),
                },
            });
        }
        body.data.ok_or_else(|| {
            AppError::Upstream(format!("scryer returned HTTP {status} with no data"))
        })
    }
}
