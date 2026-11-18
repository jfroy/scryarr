use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Conn;
use super::convert;
use super::model::{Episode, EpisodeFile};
use super::titles::{detail, find_title, require};
use crate::AppState;
use crate::domain::Flavor;
use crate::error::AppError;
use crate::scryer;
use crate::scryer::ops::{
    DeleteEpisodeFiles, DeleteEpisodeFilesPreview, SetEpisodeMonitored, TitleDetail,
    delete_episode_files, delete_episode_files_preview, set_episode_monitored, title_detail,
};
use crate::store::IdKind;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesQuery {
    series_id: Option<i32>,
}

fn series_arr_id(title: &scryer::Title) -> Option<i32> {
    convert::external_id(title, "tvdb")
}

fn parse_num(s: &Option<String>) -> Option<i32> {
    s.as_deref().and_then(|s| s.trim().parse().ok())
}

fn file_key(title_id: &str, file_id: &str) -> String {
    format!("{title_id}|{file_id}")
}

pub async fn list_episodes(
    State(st): State<AppState>,
    conn: Conn,
    Query(q): Query<SeriesQuery>,
) -> Result<Json<Vec<Episode>>, AppError> {
    require(&conn, Flavor::Sonarr)?;
    let Some(series_id) = q.series_id else {
        return Ok(Json(Vec::new()));
    };
    let title = find_title(&st, &conn, series_id).await?;
    let d = detail(&st, &conn, &title).await?;
    let mut out = Vec::new();
    for season in &d.seasons {
        for e in &season.episodes {
            let (Some(sn), Some(en)) = (parse_num(&e.season_number), parse_num(&e.episode_number))
            else {
                continue;
            };
            let id = st
                .store
                .arr_id(IdKind::Episode, &conn.library_id, &e.id)
                .await?;
            let episode_file_id = match e.media_file_ids.first() {
                Some(f) => {
                    st.store
                        .arr_id(
                            IdKind::EpisodeFile,
                            &conn.library_id,
                            &file_key(&title.id, f),
                        )
                        .await?
                }
                None => 0,
            };
            out.push(Episode {
                id,
                series_id,
                season_number: sn,
                episode_number: en,
                title: e.title.clone(),
                air_date_utc: e.air_date.as_ref().map(|d| format!("{d}T00:00:00Z")),
                monitored: e.monitored,
                has_file: !e.media_file_ids.is_empty(),
                episode_file_id,
            });
        }
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorBody {
    episode_ids: Vec<i32>,
    monitored: bool,
}

pub async fn monitor_episodes(
    State(st): State<AppState>,
    conn: Conn,
    Json(body): Json<MonitorBody>,
) -> Result<Json<Value>, AppError> {
    require(&conn, Flavor::Sonarr)?;
    let mut keys = Vec::new();
    for id in &body.episode_ids {
        let key = st
            .store
            .scryer_key(IdKind::Episode, &conn.library_id, *id)
            .await?
            .ok_or_else(|| AppError::NotFound("episode not found".into()))?;
        keys.push(key);
    }
    for key in keys {
        st.scryer
            .run::<SetEpisodeMonitored>(
                &conn.key,
                set_episode_monitored::Variables {
                    input: set_episode_monitored::SetEpisodeMonitoredInput {
                        episode_id: key,
                        monitored: body.monitored,
                    },
                },
            )
            .await?;
    }
    Ok(Json(json!([])))
}

async fn episode_file(
    st: &AppState,
    conn: &Conn,
    title: &scryer::Title,
    d: &scryer::TitleDetail,
    file_id: &str,
    series_id: i32,
) -> Result<Option<EpisodeFile>, AppError> {
    let Some(f) = d.media_files.iter().find(|f| f.id == file_id) else {
        return Ok(None);
    };
    let Some(episode_id) = f.episode_id.as_deref() else {
        return Ok(None);
    };
    let Some(sn) = d
        .seasons
        .iter()
        .flat_map(|s| &s.episodes)
        .find(|e| e.id == episode_id)
        .and_then(|e| parse_num(&e.season_number))
    else {
        return Ok(None);
    };
    let id = st
        .store
        .arr_id(
            IdKind::EpisodeFile,
            &conn.library_id,
            &file_key(&title.id, &f.id),
        )
        .await?;
    Ok(Some(EpisodeFile {
        id,
        series_id,
        season_number: sn,
        path: f.path.clone(),
        size: 0,
    }))
}

pub async fn list_episode_files(
    State(st): State<AppState>,
    conn: Conn,
    Query(q): Query<SeriesQuery>,
) -> Result<Json<Vec<EpisodeFile>>, AppError> {
    require(&conn, Flavor::Sonarr)?;
    let Some(series_id) = q.series_id else {
        return Ok(Json(Vec::new()));
    };
    let title = find_title(&st, &conn, series_id).await?;
    let d = detail(&st, &conn, &title).await?;
    let mut out = Vec::new();
    for f in &d.media_files {
        if let Some(ef) = episode_file(&st, &conn, &title, &d, &f.id, series_id).await? {
            out.push(ef);
        }
    }
    Ok(Json(out))
}

struct Resolved {
    title: scryer::Title,
    detail: scryer::TitleDetail,
    file_id: String,
    episode_id: String,
}

async fn resolve(st: &AppState, conn: &Conn, id: i32) -> Result<Resolved, AppError> {
    let not_found = || AppError::NotFound("episode file not found".into());
    let key = st
        .store
        .scryer_key(IdKind::EpisodeFile, &conn.library_id, id)
        .await?
        .ok_or_else(not_found)?;
    let (title_id, file_id) = key.split_once('|').ok_or_else(not_found)?;
    let data = st
        .scryer
        .run::<TitleDetail>(
            &conn.key,
            title_detail::Variables {
                id: title_id.to_string(),
            },
        )
        .await?;
    let d: scryer::TitleDetail = data.title.ok_or_else(not_found)?.into();
    if d.title.library_id != conn.library_id {
        return Err(not_found());
    }
    let episode_id = d
        .media_files
        .iter()
        .find(|f| f.id == file_id)
        .and_then(|f| f.episode_id.clone())
        .ok_or_else(not_found)?;
    Ok(Resolved {
        title: d.title.clone(),
        detail: d,
        file_id: file_id.to_string(),
        episode_id,
    })
}

pub async fn get_episode_file(
    State(st): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
) -> Result<Json<EpisodeFile>, AppError> {
    require(&conn, Flavor::Sonarr)?;
    let r = resolve(&st, &conn, id).await?;
    let series_id = series_arr_id(&r.title)
        .ok_or_else(|| AppError::NotFound("episode file not found".into()))?;
    episode_file(&st, &conn, &r.title, &r.detail, &r.file_id, series_id)
        .await?
        .map(Json)
        .ok_or_else(|| AppError::NotFound("episode file not found".into()))
}

pub async fn delete_episode_file(
    State(st): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
) -> Result<Json<Value>, AppError> {
    require(&conn, Flavor::Sonarr)?;
    let r = resolve(&st, &conn, id).await?;
    let preview = st
        .scryer
        .run::<DeleteEpisodeFilesPreview>(
            &conn.key,
            delete_episode_files_preview::Variables {
                input: delete_episode_files_preview::DeleteEpisodeFilesPreviewInput {
                    title_id: r.title.id.clone(),
                    episode_ids: vec![r.episode_id.clone()],
                },
            },
        )
        .await?
        .delete_episode_files_preview
        .preview;
    let typed = preview
        .requires_typed_confirmation
        .then_some(preview.typed_confirmation_prompt)
        .flatten();
    st.scryer
        .run_omitting_nulls::<DeleteEpisodeFiles>(
            &conn.key,
            delete_episode_files::Variables {
                input: delete_episode_files::DeleteEpisodeFilesInput {
                    title_id: r.title.id,
                    episode_ids: vec![r.episode_id],
                    delete_from_disk: Some(true),
                    preview_fingerprint: Some(preview.fingerprint),
                    typed_confirmation: typed,
                },
            },
        )
        .await?;
    Ok(Json(json!({})))
}
