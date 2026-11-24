pub mod ops;
mod views;

use graphql_client::{GraphQLQuery, Response};
use url::Url;

use crate::error::AppError;
use crate::metrics::Metrics;

pub use views::{CollectionKind, EpisodeView, ListedTitle, MediaFile, Season, Title, TitleDetail};

pub const SCRYER_SCHEMA_VERSION: &str = "0.23.1";

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
    metrics: Option<Metrics>,
}

impl ScryerClient {
    pub fn new(base: &Url, http: reqwest::Client) -> Self {
        let endpoint = Url::parse(&format!("{}/graphql", base.as_str().trim_end_matches('/')))
            .expect("base url plus /graphql is a valid url");
        Self {
            endpoint,
            http,
            metrics: None,
        }
    }

    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub async fn run<Q: GraphQLQuery>(
        &self,
        key: &ApiKey,
        vars: Q::Variables,
    ) -> Result<Q::ResponseData, AppError> {
        self.counted(
            self.send::<Q>(key, Q::build_query(vars), false, |_| false)
                .await,
        )
    }

    /// Like `run`, but accepts a response that carries `data` when every
    /// GraphQL error satisfies `tolerated`. Each tolerated error is logged as a
    /// warning. Errors without `data`, or any other error, still fail.
    pub async fn run_tolerating<Q: GraphQLQuery>(
        &self,
        key: &ApiKey,
        vars: Q::Variables,
        tolerated: impl Fn(&graphql_client::Error) -> bool,
    ) -> Result<Q::ResponseData, AppError> {
        self.counted(
            self.send::<Q>(key, Q::build_query(vars), false, tolerated)
                .await,
        )
    }

    /// Like `run`, but drops every null variable before sending. Generated input
    /// types serialize `None` as an explicit `null`, which scryer reads as "clear
    /// this field" on patch inputs; omission is what means "leave it alone".
    pub async fn run_omitting_nulls<Q: GraphQLQuery>(
        &self,
        key: &ApiKey,
        vars: Q::Variables,
    ) -> Result<Q::ResponseData, AppError> {
        self.counted(
            self.send::<Q>(key, Q::build_query(vars), true, |_| false)
                .await,
        )
    }

    fn counted<T>(&self, result: Result<T, AppError>) -> Result<T, AppError> {
        if let (Err(e), Some(m)) = (&result, &self.metrics) {
            m.record_scryer_error(e);
        }
        result
    }

    async fn send<Q: GraphQLQuery>(
        &self,
        key: &ApiKey,
        body: graphql_client::QueryBody<Q::Variables>,
        omit_nulls: bool,
        tolerated: impl Fn(&graphql_client::Error) -> bool,
    ) -> Result<Q::ResponseData, AppError> {
        let mut payload = serde_json::to_value(&body)
            .map_err(|e| AppError::Internal(format!("serializing graphql request: {e}")))?;
        if omit_nulls && let Some(vars) = payload.get_mut("variables") {
            strip_nulls(vars);
        }
        let resp = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(&key.0)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                AppError::Upstream(format!("scryer request failed: {}", e.without_url()))
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AppError::Unauthorized);
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(AppError::Forbidden("scryer refused the request".into()));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok());
            return Err(AppError::RateLimited { retry_after });
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

        let errors = body.errors.unwrap_or_default();
        if !errors.is_empty() && body.data.is_some() && errors.iter().all(tolerated) {
            for e in &errors {
                let path = e
                    .path
                    .iter()
                    .flatten()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(".");
                tracing::warn!(
                    path,
                    message = %e.message,
                    "scryer returned a partial response; using its data"
                );
            }
        } else if let Some(first) = errors.first() {
            let ext = |k: &str| {
                first
                    .extensions
                    .as_ref()
                    .and_then(|m| m.get(k))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            };
            let code = ext("code");
            let reason = ext("reason");
            let rate_limited = [&code, &reason]
                .into_iter()
                .flatten()
                .any(|s| s.contains("RATE_LIMITED"));
            if rate_limited {
                let retry_after = first
                    .extensions
                    .as_ref()
                    .and_then(|m| m.get("retryAfterSeconds"))
                    .and_then(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()));
                return Err(AppError::RateLimited { retry_after });
            }
            return Err(match code.as_deref() {
                Some("UNAUTHORIZED") if reason.is_some() => {
                    AppError::Forbidden(first.message.clone())
                }
                Some("UNAUTHORIZED") => AppError::Unauthorized,
                Some("NOT_FOUND") => AppError::NotFound(first.message.clone()),
                _ => AppError::Scryer {
                    code,
                    reason,
                    message: first.message.clone(),
                },
            });
        }
        body.data.ok_or_else(|| {
            AppError::Upstream(format!("scryer returned HTTP {status} with no data"))
        })
    }
}

fn strip_nulls(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|_, x| !x.is_null());
            map.values_mut().for_each(strip_nulls);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}
