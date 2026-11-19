mod convert;
mod episodes;
pub mod model;
mod notification;
mod queue;
mod system;
mod titles;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Router};

use crate::AppState;
use crate::domain::{Facet, Flavor};
use crate::error::AppError;
use crate::scryer::ApiKey;
use crate::scryer::ops::{Libraries, libraries};

const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct LibraryInfo {
    pub id: String,
    pub name: String,
    pub facet: Facet,
    pub roots: Vec<String>,
    pub quality_profile_id: Option<String>,
    pub request_quality_profile_ids: Vec<String>,
}

/// The library permission a route needs. scryer's `libraries` lists a library
/// only to callers holding at least the requested permission, and fills its
/// root folders only for callers who manage it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Access {
    View,
    ManageTitles,
}

type CacheMap = HashMap<(u64, Access, String), (Instant, Arc<LibraryInfo>)>;

#[derive(Clone, Default)]
pub struct LibraryCache(Arc<Mutex<CacheMap>>);

fn key_hash(key: &ApiKey) -> u64 {
    let mut h = DefaultHasher::new();
    key.0.hash(&mut h);
    h.finish()
}

impl LibraryCache {
    fn get(&self, key: &ApiKey, access: Access, library_id: &str) -> Option<Arc<LibraryInfo>> {
        let map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&(key_hash(key), access, library_id.to_string()))
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, info)| info.clone())
    }

    fn put(&self, key: &ApiKey, access: Access, info: Arc<LibraryInfo>) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        map.insert(
            (key_hash(key), access, info.id.clone()),
            (Instant::now(), info),
        );
    }
}

pub struct Conn {
    pub flavor: Flavor,
    pub library_id: String,
    pub facet: Facet,
    pub key: ApiKey,
    pub library: Arc<LibraryInfo>,
}

fn facet_from_scryer(f: &libraries::MediaFacetValue) -> Facet {
    match serde_json::to_value(f)
        .ok()
        .as_ref()
        .and_then(|v| v.as_str())
    {
        Some("SERIES") => Facet::Series,
        Some("ANIME") => Facet::Anime,
        _ => Facet::Movie,
    }
}

async fn resolve_library(
    state: &AppState,
    key: &ApiKey,
    access: Access,
    library_id: &str,
) -> Result<Option<Arc<LibraryInfo>>, AppError> {
    if let Some(l) = state.libraries.get(key, access, library_id) {
        return Ok(Some(l));
    }
    let permission = match access {
        Access::View => None,
        Access::ManageTitles => Some(libraries::LibraryPermissionValue::MANAGE_TITLES),
    };
    let data = state
        .scryer
        .run::<Libraries>(
            key,
            libraries::Variables {
                facet: None,
                permission,
            },
        )
        .await?;
    let mut found = None;
    for l in data.libraries {
        let info = Arc::new(LibraryInfo {
            id: l.id,
            name: l.name,
            facet: facet_from_scryer(&l.facet),
            roots: l.roots.into_iter().map(|r| r.path).collect(),
            quality_profile_id: Some(l.quality_profile_id).filter(|s| !s.is_empty()),
            request_quality_profile_ids: l.request_quality_profile_ids,
        });
        state.libraries.put(key, access, info.clone());
        if info.id == library_id {
            found = Some(info);
        }
    }
    Ok(found)
}

impl FromRequestParts<AppState> for Conn {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Extension(flavor) = Extension::<Flavor>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::NotFound("route".into()))?;
        let Path(params) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::NotFound("library not found".into()))?;
        let library_id = params
            .get("library_id")
            .cloned()
            .ok_or_else(|| AppError::NotFound("library not found".into()))?;
        let key = parts
            .headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(|v| ApiKey(v.to_string()))
            .ok_or(AppError::Unauthorized)?;

        let library = resolve_library(state, &key, Access::View, &library_id)
            .await?
            .ok_or_else(|| AppError::NotFound("library not found".into()))?;

        if !flavor.accepts(library.facet) {
            let name = match flavor {
                Flavor::Radarr => "radarr",
                Flavor::Sonarr => "sonarr",
            };
            return Err(AppError::BadRequest(format!(
                "library facet does not match {name}"
            )));
        }
        Ok(Conn {
            flavor,
            facet: library.facet,
            library_id,
            key,
            library,
        })
    }
}

/// A connection whose key may manage titles in the library. Bridge-local
/// state (webhook registrations, the roots that route events) is written on
/// behalf of the library, so it needs more than View.
pub struct ManagedConn(pub Conn);

impl FromRequestParts<AppState> for ManagedConn {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let conn = Conn::from_request_parts(parts, state).await?;
        let library = resolve_library(state, &conn.key, Access::ManageTitles, &conn.library_id)
            .await?
            .ok_or_else(|| {
                AppError::Forbidden(
                    "the Manage titles permission is required on this library".into(),
                )
            })?;
        Ok(ManagedConn(Conn { library, ..conn }))
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/system/status", get(system::status))
        .route("/qualityprofile", get(system::quality_profiles))
        .route("/rootfolder", get(system::root_folders))
        .route("/tag", get(system::tags).post(system::create_tag))
        .route("/languageprofile", get(system::language_profiles))
        .route("/movie", get(titles::list_movies).post(titles::add_movie))
        .route("/movie/lookup", get(titles::lookup_movie))
        .route(
            "/movie/{id}",
            get(titles::get_movie)
                .put(titles::update_movie)
                .delete(titles::delete_movie),
        )
        .route("/series", get(titles::list_series).post(titles::add_series))
        .route("/series/lookup", get(titles::lookup_series))
        .route(
            "/series/{id}",
            get(titles::get_series)
                .put(titles::update_series)
                .delete(titles::delete_series),
        )
        .route("/episode", get(episodes::list_episodes))
        .route("/episode/monitor", put(episodes::monitor_episodes))
        .route("/episodefile", get(episodes::list_episode_files))
        .route(
            "/episodefile/{id}",
            get(episodes::get_episode_file).delete(episodes::delete_episode_file),
        )
        .route("/queue", get(queue::list_queue))
        .route("/queue/{id}", delete(queue::delete_queue))
        .route("/history/movie", get(queue::history_movie))
        .route("/history/series", get(queue::history_series))
        .route("/history/failed/{id}", post(queue::history_failed))
        .route("/command", post(queue::post_command))
        .route(
            "/notification",
            get(notification::list).post(notification::create),
        )
        .route("/notification/schema", get(notification::schema))
        .route(
            "/notification/{id}",
            put(notification::update).delete(notification::delete),
        )
}
