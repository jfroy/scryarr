use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Conn;
use super::model::{Command, HistoryRecord, QueueItem, QueuePage};
use super::titles::{find_title, require};
use crate::AppState;
use crate::domain::Flavor;
use crate::error::AppError;
use crate::scryer;
use crate::scryer::CollectionKind;
use crate::scryer::ops::{
    DeleteDownload, IgnoreDownload, MarkFailed, QueuePage as QueuePageOp, TitleDetail,
    TitleDownloads, TitleHistory, TriggerSearch, delete_download, ignore_download, mark_failed,
    queue_page, title_detail, title_downloads, title_history, trigger_search,
};
use crate::store::IdKind;

const QUEUE_FETCH_LIMIT: i64 = 200;
const HISTORY_LIMIT: i64 = 100;
const DEFAULT_PAGE_SIZE: i32 = 10;

static COMMAND_ID: AtomicI32 = AtomicI32::new(0);

fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn source(conn: &Conn) -> &'static str {
    match conn.flavor {
        Flavor::Radarr => "tmdb",
        Flavor::Sonarr => "tvdb",
    }
}

fn state_for(display: &str) -> (&'static str, &'static str) {
    match display {
        "QUEUED" => ("queued", "downloading"),
        "DOWNLOADING" => ("downloading", "downloading"),
        "PAUSED" => ("paused", "downloading"),
        "POST_PROCESSING" | "IMPORTING" | "IMPORT_PENDING" => ("completed", "importPending"),
        "COMPLETED" | "IMPORTED_SEEDING" => ("completed", "imported"),
        "IMPORT_BLOCKED" | "WARNING" => ("warning", "importBlocked"),
        "FAILED" | "IMPORT_FAILED" => ("failed", "failed"),
        "IGNORED" => ("completed", "ignored"),
        "REMOVING" | "REMOVE_FAILED" => ("deleted", "failed"),
        _ => ("unknown", "downloading"),
    }
}

fn timeleft(seconds: i64) -> String {
    let s = seconds.max(0);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

fn sizeleft(size: i64, progress: f64) -> i64 {
    let done = (progress.round() as i64).clamp(0, 100);
    (size.max(0) * (100 - done) / 100).max(0)
}

fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn parse_num(s: &Option<String>) -> Option<i32> {
    s.as_deref().and_then(|s| s.trim().parse().ok())
}

/// Where a title's episodes and SEASON collections sit, for resolving the
/// season of a queue item or history event that does not name one episode.
#[derive(Default)]
struct SeasonIndex {
    episodes: HashMap<String, i32>,
    collections: HashMap<String, i32>,
}

impl SeasonIndex {
    fn new(d: &scryer::TitleDetail) -> Self {
        let mut idx = SeasonIndex::default();
        for s in &d.seasons {
            if s.kind == CollectionKind::Season
                && let Ok(n) = s.index.trim().parse()
            {
                idx.collections.insert(s.id.clone(), n);
            }
            for e in &s.episodes {
                if let Some(n) = parse_num(&e.season_number) {
                    idx.episodes.insert(e.id.clone(), n);
                }
            }
        }
        idx
    }

    /// The episode's season, else the one season shared by `episode_ids`,
    /// else the SEASON collection's index.
    fn season(
        &self,
        episode_id: Option<&str>,
        episode_ids: &[String],
        collection_id: Option<&str>,
    ) -> Option<i32> {
        if let Some(&n) = episode_id.and_then(|e| self.episodes.get(e)) {
            return Some(n);
        }
        let shared: BTreeSet<i32> = episode_ids
            .iter()
            .filter_map(|e| self.episodes.get(e).copied())
            .collect();
        if shared.len() == 1 {
            return shared.into_iter().next();
        }
        collection_id.and_then(|c| self.collections.get(c).copied())
    }
}

async fn fetch_season_index(
    st: &AppState,
    conn: &Conn,
    title_id: &str,
) -> Result<Option<SeasonIndex>, AppError> {
    let data = st
        .scryer
        .run::<TitleDetail>(
            &conn.key,
            title_detail::Variables {
                id: title_id.to_string(),
            },
        )
        .await?;
    Ok(data.title.map(|t| SeasonIndex::new(&t.into())))
}

/// Cached per title. A title whose detail cannot be read resolves no season
/// rather than failing the whole queue.
async fn season_index<'c>(
    st: &AppState,
    conn: &Conn,
    cache: &'c mut HashMap<String, Option<SeasonIndex>>,
    title_id: &str,
) -> Option<&'c SeasonIndex> {
    if !cache.contains_key(title_id) {
        let idx = fetch_season_index(st, conn, title_id)
            .await
            .inspect_err(|e| {
                tracing::warn!(title_id, error = %e, "title detail failed; queue season left unknown")
            })
            .ok()
            .flatten();
        cache.insert(title_id.to_string(), idx);
    }
    cache.get(title_id).and_then(Option::as_ref)
}

#[derive(Default)]
struct Scope {
    episode_id: Option<String>,
    episode_ids: Vec<String>,
    collection_id: Option<String>,
}

fn scope_of(s: Option<&queue_page::QueuePageDownloadQueuePageItemsQueueScope>) -> Scope {
    use queue_page::QueuePageDownloadQueuePageItemsQueueScope as Q;
    match s {
        Some(Q::EpisodeScopePayload(e)) => Scope {
            episode_id: Some(e.episode_id.clone()),
            ..Scope::default()
        },
        Some(Q::EpisodeSetScopePayload(e)) => Scope {
            episode_ids: e.episode_ids.clone(),
            ..Scope::default()
        },
        Some(Q::CollectionScopePayload(c)) => Scope {
            collection_id: Some(c.collection_id.clone()),
            ..Scope::default()
        },
        _ => Scope::default(),
    }
}

fn queue_key(client_type: &str, client_id: Option<&str>, item_id: &str) -> String {
    format!("{client_type}|{}|{item_id}", client_id.unwrap_or_default())
}

struct Entry {
    item: QueueItem,
}

async fn queue_entries(st: &AppState, conn: &Conn) -> Result<Vec<Entry>, AppError> {
    let mut items = Vec::new();
    let mut offset = 0;
    loop {
        let data = st
            .scryer
            .run::<QueuePageOp>(
                &conn.key,
                queue_page::Variables {
                    limit: QUEUE_FETCH_LIMIT,
                    offset,
                },
            )
            .await?;
        let page = data.download_queue_page;
        items.extend(page.items);
        if !page.has_more {
            break;
        }
        offset += QUEUE_FETCH_LIMIT;
    }

    let mut details = HashMap::new();
    let mut out = Vec::new();
    for i in items {
        let Some(t) = i.title.as_ref() else { continue };
        if t.library_id != conn.library_id {
            continue;
        }
        let ext = t
            .external_ids
            .iter()
            .find(|e| e.source.eq_ignore_ascii_case(source(conn)))
            .and_then(|e| e.value.trim().parse::<i32>().ok());
        let Some(ext) = ext else { continue };
        let key = queue_key(
            &i.client_type,
            Some(i.client_id.as_str()),
            &i.download_client_item_id,
        );
        let id = st
            .store
            .arr_id(IdKind::Queue, &conn.library_id, &key)
            .await?;
        let (episode_id, season_number) = if conn.flavor == Flavor::Sonarr {
            let scope = scope_of(i.queue_scope.as_ref());
            let ep = i.episode_id.clone().or(scope.episode_id);
            let eid = match ep.as_deref() {
                Some(ep) => Some(
                    st.store
                        .arr_id(IdKind::Episode, &conn.library_id, ep)
                        .await?,
                ),
                None => None,
            };
            let sn =
                if ep.is_some() || !scope.episode_ids.is_empty() || scope.collection_id.is_some() {
                    season_index(st, conn, &mut details, &t.id)
                        .await
                        .and_then(|idx| {
                            idx.season(
                                ep.as_deref(),
                                &scope.episode_ids,
                                scope.collection_id.as_deref(),
                            )
                        })
                } else {
                    None
                };
            (eid, sn)
        } else {
            (None, None)
        };
        let (status, tracked) = state_for(&enum_str(&i.display_state));
        let size = i.size_bytes.unwrap_or(0);
        out.push(Entry {
            item: QueueItem {
                id,
                title: i.title_name.clone(),
                status: status.into(),
                tracked_download_state: tracked.into(),
                size,
                sizeleft: sizeleft(size, i.progress_percent as f64),
                timeleft: i.remaining_seconds.map(timeleft),
                movie_id: (conn.flavor == Flavor::Radarr).then_some(ext),
                series_id: (conn.flavor == Flavor::Sonarr).then_some(ext),
                episode_id,
                season_number,
                download_id: i.download_id.clone().unwrap_or_default(),
                protocol: "unknown".into(),
                download_client: i.client_type.clone(),
            },
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageQuery {
    page: Option<i32>,
    page_size: Option<i32>,
}

pub async fn list_queue(
    State(st): State<AppState>,
    conn: Conn,
    Query(q): Query<PageQuery>,
) -> Result<Json<QueuePage>, AppError> {
    let entries = queue_entries(&st, &conn).await?;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(DEFAULT_PAGE_SIZE).max(1);
    let total = entries.len();
    let start = ((page - 1) as usize).saturating_mul(page_size as usize);
    let records = entries
        .into_iter()
        .skip(start)
        .take(page_size as usize)
        .map(|e| e.item)
        .collect();
    Ok(Json(QueuePage {
        page,
        page_size,
        total_records: total as i32,
        records,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteQuery {
    remove_from_client: Option<bool>,
}

pub async fn delete_queue(
    State(st): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
    Query(q): Query<DeleteQuery>,
) -> Result<Json<Value>, AppError> {
    let not_found = || AppError::NotFound("queue item not found".into());
    let key = st
        .store
        .scryer_key(IdKind::Queue, &conn.library_id, id)
        .await?
        .ok_or_else(not_found)?;
    let mut parts = key.splitn(3, '|');
    let (Some(client_type), Some(client_id), Some(item_id)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return Err(not_found());
    };
    let client_id = Some(client_id.to_string()).filter(|s| !s.is_empty());
    if q.remove_from_client.unwrap_or(true) {
        st.scryer
            .run_omitting_nulls::<DeleteDownload>(
                &conn.key,
                delete_download::Variables {
                    input: delete_download::DeleteDownloadInput {
                        client_id,
                        client_type: client_type.to_string(),
                        download_client_item_id: item_id.to_string(),
                        is_history: false,
                    },
                },
            )
            .await?;
    } else {
        st.scryer
            .run_omitting_nulls::<IgnoreDownload>(
                &conn.key,
                ignore_download::Variables {
                    input: ignore_download::IgnoreTrackedDownloadInput {
                        client_id,
                        client_type: client_type.to_string(),
                        download_client_item_id: item_id.to_string(),
                    },
                },
            )
            .await?;
    }
    Ok(Json(serde_json::json!({})))
}

/// scryer reports `TitleHistoryEventType::as_str()`: lowercase snake_case.
fn event_type(scryer_event: &str, flavor: Flavor) -> &'static str {
    let movie = flavor == Flavor::Radarr;
    match scryer_event.trim().to_ascii_lowercase().as_str() {
        "grabbed" => "grabbed",
        "imported" => "downloadFolderImported",
        "download_failed" | "import_failed" => "downloadFailed",
        "file_deleted" if movie => "movieFileDeleted",
        "file_deleted" => "episodeFileDeleted",
        "file_renamed" if movie => "movieFileRenamed",
        "file_renamed" => "episodeFileRenamed",
        "download_ignored" => "downloadIgnored",
        _ => "unknown",
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryQuery {
    movie_id: Option<i32>,
    series_id: Option<i32>,
    season_number: Option<i32>,
}

async fn history_for(
    st: &AppState,
    conn: &Conn,
    arr_title_id: i32,
    season: Option<i32>,
) -> Result<Vec<HistoryRecord>, AppError> {
    let title = find_title(st, conn, arr_title_id).await?;
    let data = st
        .scryer
        .run_omitting_nulls::<TitleHistory>(
            &conn.key,
            title_history::Variables {
                filter: title_history::TitleHistoryFilterInput {
                    event_types: None,
                    title_ids: Some(vec![title.id.clone()]),
                    library_ids: None,
                    title_search: None,
                    download_id: None,
                    episode_id: None,
                    collection_id: None,
                    group_by_event: None,
                    limit: Some(HISTORY_LIMIT),
                    offset: None,
                },
            },
        )
        .await?;
    let seasons = match season {
        Some(_) => fetch_season_index(st, conn, &title.id)
            .await?
            .unwrap_or_default(),
        None => SeasonIndex::default(),
    };
    let mut out = Vec::new();
    for h in data.title_history.items {
        if let Some(want) = season
            && seasons.season(
                h.episode_id.as_deref(),
                &h.episode_ids,
                h.collection_id.as_deref(),
            ) != Some(want)
        {
            continue;
        }
        let key = format!(
            "{}|{}|{}|{}",
            title.id,
            h.client_id.as_deref().unwrap_or_default(),
            h.download_id.as_deref().unwrap_or_default(),
            h.id
        );
        let id = st
            .store
            .arr_id(IdKind::History, &conn.library_id, &key)
            .await?;
        let episode_id = match (conn.flavor, h.episode_id.as_deref()) {
            (Flavor::Sonarr, Some(ep)) => Some(
                st.store
                    .arr_id(IdKind::Episode, &conn.library_id, ep)
                    .await?,
            ),
            _ => None,
        };
        out.push(HistoryRecord {
            id,
            event_type: event_type(&h.event_type, conn.flavor).into(),
            date: h.occurred_at.clone(),
            download_id: h.download_id.clone(),
            source_title: h.source_title.clone().unwrap_or_default(),
            movie_id: (conn.flavor == Flavor::Radarr).then_some(arr_title_id),
            series_id: (conn.flavor == Flavor::Sonarr).then_some(arr_title_id),
            episode_id,
        });
    }
    Ok(out)
}

pub async fn history_movie(
    State(st): State<AppState>,
    conn: Conn,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<HistoryRecord>>, AppError> {
    require(&conn, Flavor::Radarr)?;
    let Some(id) = q.movie_id else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(history_for(&st, &conn, id, None).await?))
}

pub async fn history_series(
    State(st): State<AppState>,
    conn: Conn,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<HistoryRecord>>, AppError> {
    require(&conn, Flavor::Sonarr)?;
    let Some(id) = q.series_id else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(history_for(&st, &conn, id, q.season_number).await?))
}

pub async fn history_failed(
    State(st): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
) -> Result<Json<Value>, AppError> {
    let not_found = || AppError::NotFound("history item not found".into());
    let key = st
        .store
        .scryer_key(IdKind::History, &conn.library_id, id)
        .await?
        .ok_or_else(not_found)?;
    let mut parts = key.splitn(4, '|');
    let (Some(title_id), Some(client_id), Some(download_id)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return Err(not_found());
    };
    if download_id.is_empty() {
        return Err(not_found());
    }
    let data = st
        .scryer
        .run::<TitleDownloads>(
            &conn.key,
            title_downloads::Variables {
                id: title_id.to_string(),
            },
        )
        .await?;
    let item = data
        .title
        .ok_or_else(not_found)?
        .download_queue_items
        .into_iter()
        .find(|d| {
            d.download_client_item_id == download_id
                || d.download_id.as_deref() == Some(download_id)
        })
        .ok_or_else(not_found)?;
    let client_id = Some(client_id.to_string())
        .filter(|s| !s.is_empty())
        .or(Some(item.client_id).filter(|s| !s.is_empty()));
    st.scryer
        .run_omitting_nulls::<MarkFailed>(
            &conn.key,
            mark_failed::Variables {
                input: mark_failed::MarkTrackedDownloadFailedInput {
                    client_id,
                    client_type: item.client_type,
                    download_client_item_id: item.download_client_item_id,
                    skip_reacquire: None,
                },
            },
        )
        .await?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandBody {
    name: String,
    #[serde(default)]
    movie_ids: Vec<i32>,
    series_id: Option<i32>,
}

async fn search(st: &AppState, conn: &Conn, arr_id: i32) -> Result<(), AppError> {
    let title = find_title(st, conn, arr_id).await?;
    st.scryer
        .run_omitting_nulls::<TriggerSearch>(
            &conn.key,
            trigger_search::Variables {
                input: trigger_search::TriggerAcquisitionSearchInput {
                    intent: Some(trigger_search::AcquisitionSearchIntentValue::AUTOMATIC),
                    wanted_kind: None,
                    facet: None,
                    library_ids: None,
                    title_id: Some(title.id),
                    season_number: None,
                    wanted_item_id: None,
                },
            },
        )
        .await?;
    Ok(())
}

pub async fn post_command(
    State(st): State<AppState>,
    conn: Conn,
    Json(body): Json<CommandBody>,
) -> Result<(StatusCode, Json<Command>), AppError> {
    match body.name.as_str() {
        "MoviesSearch" => {
            require(&conn, Flavor::Radarr)?;
            for id in &body.movie_ids {
                search(&st, &conn, *id).await?;
            }
        }
        "SeriesSearch" => {
            require(&conn, Flavor::Sonarr)?;
            if let Some(id) = body.series_id {
                search(&st, &conn, id).await?;
            }
        }
        "RefreshMonitoredDownloads" => {}
        other => {
            return Err(AppError::BadRequest(format!(
                "unsupported command: {other}"
            )));
        }
    }
    let now = now_rfc3339();
    Ok((
        StatusCode::CREATED,
        Json(Command {
            id: COMMAND_ID.fetch_add(1, Ordering::Relaxed) + 1,
            name: body.name,
            status: "completed".into(),
            queued: now.clone(),
            started: now.clone(),
            ended: now,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_event_types_match_scryer_codes() {
        let cases = [
            ("grabbed", "grabbed", "grabbed"),
            (
                "imported",
                "downloadFolderImported",
                "downloadFolderImported",
            ),
            ("download_failed", "downloadFailed", "downloadFailed"),
            ("import_failed", "downloadFailed", "downloadFailed"),
            ("file_deleted", "movieFileDeleted", "episodeFileDeleted"),
            ("file_renamed", "movieFileRenamed", "episodeFileRenamed"),
            ("download_ignored", "downloadIgnored", "downloadIgnored"),
            ("GRABBED", "grabbed", "grabbed"),
            ("seeding_started", "unknown", "unknown"),
        ];
        for (scryer, radarr, sonarr) in cases {
            assert_eq!(event_type(scryer, Flavor::Radarr), radarr, "{scryer}");
            assert_eq!(event_type(scryer, Flavor::Sonarr), sonarr, "{scryer}");
        }
    }

    #[test]
    fn timeleft_does_not_wrap_hours() {
        assert_eq!(timeleft(90_000), "25:00:00");
        assert_eq!(timeleft(3725), "01:02:05");
    }

    #[test]
    fn sizeleft_is_clamped() {
        assert_eq!(sizeleft(1000, 25.0), 750);
        assert_eq!(sizeleft(1000, 150.0), 0);
        assert_eq!(sizeleft(1000, -5.0), 1000);
    }

    #[test]
    fn rfc3339_epoch_shape() {
        let s = now_rfc3339();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
    }
}
