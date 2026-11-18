use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::Conn;
use super::convert;
use super::model::{RadarrMovie, SonarrSeason, SonarrSeries};
use crate::AppState;
use crate::domain::{Facet, Flavor};
use crate::error::AppError;
use crate::scryer;
use crate::scryer::ops::{
    AddTitle, DeleteTitle, MetadataMovie, MetadataSeries, SetTitleMonitored, TitleDetail,
    TitlesByExternalIds, TitlesPage, TriggerSearch, UpdateTitle, add_title, delete_title,
    metadata_movie, metadata_series, set_title_monitored, title_detail, titles_by_external_ids,
    titles_page, trigger_search, update_title,
};
use crate::store::IdKind;

const PAGE_SIZE: i64 = 200;

fn require(conn: &Conn, flavor: Flavor) -> Result<(), AppError> {
    if conn.flavor == flavor {
        Ok(())
    } else {
        Err(AppError::NotFound("route".into()))
    }
}

fn source(conn: &Conn) -> &'static str {
    match conn.flavor {
        Flavor::Radarr => "tmdb",
        Flavor::Sonarr => "tvdb",
    }
}

fn facet_var(f: Facet) -> titles_page::MediaFacetValue {
    match f {
        Facet::Movie => titles_page::MediaFacetValue::MOVIE,
        Facet::Series => titles_page::MediaFacetValue::SERIES,
        Facet::Anime => titles_page::MediaFacetValue::ANIME,
    }
}

async fn qp_id(st: &AppState, conn: &Conn, title: &scryer::Title) -> Result<Option<i32>, AppError> {
    match title
        .quality_profile_id
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        Some(p) => Ok(Some(
            st.store
                .arr_id(IdKind::QualityProfile, &conn.library_id, p)
                .await?,
        )),
        None => Ok(None),
    }
}

pub async fn find_title(
    state: &AppState,
    conn: &Conn,
    arr_id: i32,
) -> Result<scryer::Title, AppError> {
    let data = state
        .scryer
        .run::<TitlesByExternalIds>(
            &conn.key,
            titles_by_external_ids::Variables {
                source: source(conn).to_string(),
                values: vec![arr_id.to_string()],
            },
        )
        .await?;
    data.titles_by_external_ids
        .into_iter()
        .map(scryer::Title::from)
        .find(|t| t.library_id == conn.library_id)
        .ok_or_else(|| AppError::NotFound("title not found".into()))
}

async fn all_titles(st: &AppState, conn: &Conn) -> Result<Vec<scryer::Title>, AppError> {
    let mut out = Vec::new();
    let mut offset = 0;
    loop {
        let data = st
            .scryer
            .run::<TitlesPage>(
                &conn.key,
                titles_page::Variables {
                    facet: Some(facet_var(conn.facet)),
                    library_ids: Some(vec![conn.library_id.clone()]),
                    limit: Some(PAGE_SIZE),
                    offset: Some(offset),
                },
            )
            .await?;
        out.extend(data.titles.items.into_iter().map(scryer::Title::from));
        if !data.titles.has_more {
            return Ok(out);
        }
        offset += PAGE_SIZE;
    }
}

async fn detail(
    st: &AppState,
    conn: &Conn,
    title: &scryer::Title,
) -> Result<scryer::TitleDetail, AppError> {
    let data = st
        .scryer
        .run::<TitleDetail>(
            &conn.key,
            title_detail::Variables {
                id: title.id.clone(),
            },
        )
        .await?;
    let d = data
        .title
        .ok_or_else(|| AppError::NotFound("title not found".into()))?;
    Ok(d.into())
}

pub async fn list_movies(
    State(st): State<AppState>,
    conn: Conn,
) -> Result<Json<Vec<RadarrMovie>>, AppError> {
    require(&conn, Flavor::Radarr)?;
    let mut out = Vec::new();
    for t in all_titles(&st, &conn).await? {
        let qp = qp_id(&st, &conn, &t).await?;
        out.extend(convert::movie(&t, qp));
    }
    Ok(Json(out))
}

pub async fn get_movie(
    State(st): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
) -> Result<Json<RadarrMovie>, AppError> {
    require(&conn, Flavor::Radarr)?;
    let t = find_title(&st, &conn, id).await?;
    let qp = qp_id(&st, &conn, &t).await?;
    convert::movie(&t, qp)
        .map(Json)
        .ok_or_else(|| AppError::NotFound("title not found".into()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesQuery {
    tvdb_id: Option<i32>,
}

pub async fn list_series(
    State(st): State<AppState>,
    conn: Conn,
    Query(q): Query<SeriesQuery>,
) -> Result<Json<Vec<SonarrSeries>>, AppError> {
    require(&conn, Flavor::Sonarr)?;
    let mut out = Vec::new();
    if let Some(tvdb) = q.tvdb_id {
        match find_title(&st, &conn, tvdb).await {
            Ok(t) => {
                let qp = qp_id(&st, &conn, &t).await?;
                let d = detail(&st, &conn, &t).await?;
                out.extend(convert::series(&t, Some(&d), qp));
            }
            Err(AppError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        return Ok(Json(out));
    }
    for t in all_titles(&st, &conn).await? {
        let qp = qp_id(&st, &conn, &t).await?;
        out.extend(convert::series(&t, None, qp));
    }
    Ok(Json(out))
}

pub async fn get_series(
    State(st): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
) -> Result<Json<SonarrSeries>, AppError> {
    require(&conn, Flavor::Sonarr)?;
    let t = find_title(&st, &conn, id).await?;
    let qp = qp_id(&st, &conn, &t).await?;
    let d = detail(&st, &conn, &t).await?;
    convert::series(&t, Some(&d), qp)
        .map(Json)
        .ok_or_else(|| AppError::NotFound("title not found".into()))
}

#[derive(Deserialize)]
pub struct LookupQuery {
    term: Option<String>,
}

fn lookup_id(q: &LookupQuery, prefix: &str) -> Option<String> {
    let term = q.term.as_deref()?.trim();
    let (p, v) = term.split_once(':')?;
    let v = v.trim();
    (p.trim().eq_ignore_ascii_case(prefix) && !v.is_empty()).then(|| v.to_string())
}

pub async fn lookup_movie(
    State(st): State<AppState>,
    conn: Conn,
    Query(q): Query<LookupQuery>,
) -> Result<Json<Vec<RadarrMovie>>, AppError> {
    require(&conn, Flavor::Radarr)?;
    let Some(tmdb) = lookup_id(&q, "tmdb").and_then(|v| v.parse::<i32>().ok()) else {
        return Ok(Json(Vec::new()));
    };
    match find_title(&st, &conn, tmdb).await {
        Ok(t) => {
            let qp = qp_id(&st, &conn, &t).await?;
            return Ok(Json(convert::movie(&t, qp).into_iter().collect()));
        }
        Err(AppError::NotFound(_)) => {}
        Err(e) => return Err(e),
    }
    let data = st
        .scryer
        .run::<MetadataMovie>(
            &conn.key,
            metadata_movie::Variables {
                tmdb_id: Some(i64::from(tmdb)),
            },
        )
        .await?;
    let m = data.metadata_movie;
    Ok(Json(vec![RadarrMovie {
        id: 0,
        title: m.name.clone(),
        sort_title: m.name.to_lowercase(),
        year: m.year.unwrap_or(0),
        tmdb_id: tmdb,
        imdb_id: Some(m.imdb_id).filter(|s| !s.is_empty()),
        title_slug: m.slug,
        overview: Some(m.overview),
        monitored: false,
        has_file: false,
        path: None,
        folder_name: None,
        root_folder_path: None,
        quality_profile_id: 0,
        minimum_availability: "released".into(),
        tags: Vec::new(),
        added: String::new(),
        size_on_disk: 0,
        status: "released".into(),
        is_available: true,
        images: Vec::new(),
        statistics: Default::default(),
    }]))
}

pub async fn lookup_series(
    State(st): State<AppState>,
    conn: Conn,
    Query(q): Query<LookupQuery>,
) -> Result<Json<Vec<SonarrSeries>>, AppError> {
    require(&conn, Flavor::Sonarr)?;
    let Some(tvdb_str) = lookup_id(&q, "tvdb") else {
        return Ok(Json(Vec::new()));
    };
    let Ok(tvdb) = tvdb_str.parse::<i32>() else {
        return Ok(Json(Vec::new()));
    };
    match find_title(&st, &conn, tvdb).await {
        Ok(t) => {
            let qp = qp_id(&st, &conn, &t).await?;
            let d = detail(&st, &conn, &t).await?;
            return Ok(Json(
                convert::series(&t, Some(&d), qp).into_iter().collect(),
            ));
        }
        Err(AppError::NotFound(_)) => {}
        Err(e) => return Err(e),
    }
    let data = st
        .scryer
        .run::<MetadataSeries>(
            &conn.key,
            metadata_series::Variables {
                tvdb_id: Some(tvdb_str),
            },
        )
        .await?;
    let m = data.metadata_series;
    let seasons: Vec<SonarrSeason> = m
        .seasons
        .iter()
        .map(|s| SonarrSeason {
            season_number: s.number as i32,
            monitored: false,
            statistics: Default::default(),
        })
        .collect();
    Ok(Json(vec![SonarrSeries {
        id: 0,
        title: m.name.clone(),
        sort_title: m.name.to_lowercase(),
        year: m.year.unwrap_or(0),
        tvdb_id: tvdb,
        tmdb_id: m.tmdb_id.map(|v| v as i32),
        imdb_id: None,
        title_slug: m.slug,
        overview: Some(m.overview),
        monitored: false,
        path: None,
        root_folder_path: None,
        quality_profile_id: 0,
        language_profile_id: 1,
        series_type: if conn.facet == Facet::Anime {
            "anime".into()
        } else {
            "standard".into()
        },
        season_folder: true,
        tags: Vec::new(),
        added: String::new(),
        network: Some(m.network).filter(|s| !s.is_empty()),
        status: "continuing".into(),
        images: Vec::new(),
        statistics: super::model::SeriesStatistics {
            season_count: seasons.len() as i64,
            ..Default::default()
        },
        seasons,
    }]))
}

pub async fn quality_profile_scryer_id(
    state: &AppState,
    conn: &Conn,
    arr_qp: i32,
) -> Result<Option<String>, AppError> {
    Ok(state
        .store
        .scryer_key(IdKind::QualityProfile, &conn.library_id, arr_qp)
        .await?)
}

async fn require_quality_profile(
    state: &AppState,
    conn: &Conn,
    arr_qp: i32,
) -> Result<String, AppError> {
    quality_profile_scryer_id(state, conn, arr_qp)
        .await?
        .ok_or_else(|| AppError::BadRequest("unknown quality profile".into()))
}

fn require_external_id(id: i32, what: &str) -> Result<i32, AppError> {
    if id > 0 {
        Ok(id)
    } else {
        Err(AppError::BadRequest(format!("{what} is required")))
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AddOptions {
    search_for_movie: bool,
    search_for_missing_episodes: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovieWrite {
    #[serde(default)]
    title: String,
    #[serde(default)]
    tmdb_id: i32,
    title_slug: Option<String>,
    #[serde(default)]
    quality_profile_id: i32,
    #[serde(default = "default_true")]
    monitored: bool,
    minimum_availability: Option<String>,
    year: Option<i64>,
    overview: Option<String>,
    #[serde(default)]
    add_options: AddOptions,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonWrite {
    season_number: i32,
    #[serde(default)]
    monitored: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesWrite {
    #[serde(default)]
    title: String,
    #[serde(default)]
    tvdb_id: i32,
    title_slug: Option<String>,
    #[serde(default)]
    quality_profile_id: i32,
    #[serde(default = "default_true")]
    monitored: bool,
    year: Option<i64>,
    overview: Option<String>,
    #[serde(default)]
    seasons: Vec<SeasonWrite>,
    #[serde(default)]
    add_options: AddOptions,
}

fn scryer_availability(v: Option<&str>) -> Option<String> {
    match v? {
        "inCinemas" => Some("in_cinemas".into()),
        "announced" => Some("announced".into()),
        "released" => Some("released".into()),
        _ => None,
    }
}

fn default_true() -> bool {
    true
}

fn monitored_seasons(seasons: &[SeasonWrite]) -> Vec<i64> {
    let mut v: Vec<i64> = seasons
        .iter()
        .filter(|s| s.monitored)
        .map(|s| i64::from(s.season_number))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

async fn search(st: &AppState, conn: &Conn, title_id: &str) -> Result<(), AppError> {
    st.scryer
        .run::<TriggerSearch>(
            &conn.key,
            trigger_search::Variables {
                input: trigger_search::TriggerAcquisitionSearchInput {
                    intent: Some(trigger_search::AcquisitionSearchIntentValue::AUTOMATIC),
                    wanted_kind: None,
                    facet: None,
                    library_ids: None,
                    title_id: Some(title_id.to_string()),
                    season_number: None,
                    wanted_item_id: None,
                },
            },
        )
        .await?;
    Ok(())
}

macro_rules! options_input {
    ($name:ident, $module:ident) => {
        fn $name(
            quality_profile_id: Option<String>,
            min_availability: Option<String>,
            monitor_type: Option<$module::MonitorTypeValue>,
            seasons: Option<Vec<i64>>,
        ) -> $module::TitleOptionsInput {
            $module::TitleOptionsInput {
                quality_profile_id,
                min_availability,
                monitor_type,
                monitor_selection: seasons.map(|season_numbers| $module::MonitorSelectionInput {
                    season_numbers,
                    series_movies: None,
                }),
                search_languages: None,
                search_aliases: None,
                root_folder_id: None,
                use_season_folders: None,
                metadata_language: None,
                monitor_specials: None,
                inter_season_movies: None,
                filler_policy: None,
                recap_policy: None,
                release_numbering: None,
                required_audio_languages: None,
                preferred_audio_languages: None,
            }
        }
    };
}

options_input!(add_options_input, add_title);
options_input!(update_options_input, update_title);

#[allow(clippy::too_many_arguments)]
fn add_input(
    conn: &Conn,
    facet: add_title::MediaFacetValue,
    name: String,
    monitored: bool,
    options: add_title::TitleOptionsInput,
    tmdb_id: Option<i64>,
    tvdb_id: Option<String>,
    slug: Option<String>,
    year: Option<i64>,
    overview: Option<String>,
) -> add_title::AddTitleInput {
    add_title::AddTitleInput {
        name,
        facet,
        library_id: Some(conn.library_id.clone()),
        monitored,
        tags: Vec::new(),
        options: Some(options),
        tmdb_id,
        tvdb_id,
        slug,
        year,
        overview,
        external_ids: None,
        smg_id: None,
        imdb_id: None,
        source_hint: None,
        source_kind: None,
        source_title: None,
        min_availability: None,
        sort_title: None,
        runtime_minutes: None,
        language: None,
        content_status: None,
    }
}

pub async fn add_movie(
    State(st): State<AppState>,
    conn: Conn,
    Json(body): Json<MovieWrite>,
) -> Result<(StatusCode, Json<RadarrMovie>), AppError> {
    require(&conn, Flavor::Radarr)?;
    let tmdb = require_external_id(body.tmdb_id, "tmdbId")?;
    let qp = require_quality_profile(&st, &conn, body.quality_profile_id).await?;
    let (year, overview) = match (body.year, body.overview.clone()) {
        (Some(y), Some(o)) => (Some(y), Some(o)),
        (y, o) => {
            let m = st
                .scryer
                .run::<MetadataMovie>(
                    &conn.key,
                    metadata_movie::Variables {
                        tmdb_id: Some(i64::from(tmdb)),
                    },
                )
                .await
                .map(|d| d.metadata_movie)
                .inspect_err(|e| tracing::warn!(error = %e, "movie metadata lookup failed; adding without it"))
                .ok();
            (
                y.or(m.as_ref().and_then(|m| m.year)),
                o.or(m.map(|m| m.overview).filter(|s| !s.is_empty())),
            )
        }
    };
    let data = st
        .scryer
        .run_omitting_nulls::<AddTitle>(
            &conn.key,
            add_title::Variables {
                input: add_input(
                    &conn,
                    add_title::MediaFacetValue::MOVIE,
                    body.title.clone(),
                    body.monitored,
                    add_options_input(
                        Some(qp),
                        scryer_availability(body.minimum_availability.as_deref()),
                        None,
                        None,
                    ),
                    Some(i64::from(tmdb)),
                    None,
                    body.title_slug.clone(),
                    year,
                    overview,
                ),
            },
        )
        .await?;
    let t = scryer::Title::from(data.add_title.title);
    if body.add_options.search_for_movie {
        search(&st, &conn, &t.id).await?;
    }
    let qp_arr = qp_id(&st, &conn, &t).await?;
    convert::movie(&t, qp_arr)
        .map(|m| (StatusCode::CREATED, Json(m)))
        .ok_or_else(|| AppError::Upstream("scryer title has no tmdb id".into()))
}

pub async fn add_series(
    State(st): State<AppState>,
    conn: Conn,
    Json(body): Json<SeriesWrite>,
) -> Result<(StatusCode, Json<SonarrSeries>), AppError> {
    require(&conn, Flavor::Sonarr)?;
    let tvdb = require_external_id(body.tvdb_id, "tvdbId")?;
    let qp = require_quality_profile(&st, &conn, body.quality_profile_id).await?;
    let (year, overview) = match (body.year, body.overview.clone()) {
        (Some(y), Some(o)) => (Some(y), Some(o)),
        (y, o) => {
            let m = st
                .scryer
                .run::<MetadataSeries>(
                    &conn.key,
                    metadata_series::Variables {
                        tvdb_id: Some(tvdb.to_string()),
                    },
                )
                .await
                .map(|d| d.metadata_series)
                .inspect_err(|e| tracing::warn!(error = %e, "series metadata lookup failed; adding without it"))
                .ok();
            (
                y.or(m.as_ref().and_then(|m| m.year)),
                o.or(m.map(|m| m.overview).filter(|s| !s.is_empty())),
            )
        }
    };
    let facet = match conn.facet {
        Facet::Anime => add_title::MediaFacetValue::ANIME,
        _ => add_title::MediaFacetValue::SERIES,
    };
    let selection = monitored_seasons(&body.seasons);
    let (monitor_type, monitor_selection) = if body.seasons.is_empty() {
        (None, None)
    } else {
        (Some(add_title::MonitorTypeValue::ADVANCED), Some(selection))
    };
    let data = st
        .scryer
        .run_omitting_nulls::<AddTitle>(
            &conn.key,
            add_title::Variables {
                input: add_input(
                    &conn,
                    facet,
                    body.title.clone(),
                    body.monitored,
                    add_options_input(Some(qp), None, monitor_type, monitor_selection),
                    None,
                    Some(tvdb.to_string()),
                    body.title_slug.clone(),
                    year,
                    overview,
                ),
            },
        )
        .await?;
    let t = scryer::Title::from(data.add_title.title);
    if body.add_options.search_for_missing_episodes {
        search(&st, &conn, &t.id).await?;
    }
    let qp_arr = qp_id(&st, &conn, &t).await?;
    let d = detail(&st, &conn, &t).await.ok();
    convert::series(&t, d.as_ref(), qp_arr)
        .map(|s| (StatusCode::CREATED, Json(s)))
        .ok_or_else(|| AppError::Upstream("scryer title has no tvdb id".into()))
}

async fn set_monitored(
    st: &AppState,
    conn: &Conn,
    title_id: &str,
    monitored: bool,
) -> Result<scryer::Title, AppError> {
    let data = st
        .scryer
        .run::<SetTitleMonitored>(
            &conn.key,
            set_title_monitored::Variables {
                input: set_title_monitored::SetTitleMonitoredInput {
                    title_id: title_id.to_string(),
                    monitored,
                },
            },
        )
        .await?;
    Ok(data.set_title_monitored.into())
}

async fn update_options(
    st: &AppState,
    conn: &Conn,
    title_id: &str,
    options: update_title::TitleOptionsInput,
) -> Result<scryer::Title, AppError> {
    let data = st
        .scryer
        .run_omitting_nulls::<UpdateTitle>(
            &conn.key,
            update_title::Variables {
                input: update_title::UpdateTitleInput {
                    title_id: title_id.to_string(),
                    name: None,
                    facet: None,
                    tags: None,
                    options: Some(options),
                },
            },
        )
        .await?;
    Ok(data.update_title.into())
}

async fn changed_quality_profile(
    st: &AppState,
    conn: &Conn,
    current: &scryer::Title,
    requested: i32,
) -> Result<Option<String>, AppError> {
    if requested == 0 {
        return Ok(None);
    }
    let want = require_quality_profile(st, conn, requested).await?;
    Ok((current.quality_profile_id.as_deref() != Some(want.as_str())).then_some(want))
}

pub async fn update_movie(
    State(st): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
    Json(body): Json<MovieWrite>,
) -> Result<(StatusCode, Json<RadarrMovie>), AppError> {
    require(&conn, Flavor::Radarr)?;
    let mut t = find_title(&st, &conn, id).await?;
    if let Some(qp) = changed_quality_profile(&st, &conn, &t, body.quality_profile_id).await? {
        t = update_options(
            &st,
            &conn,
            &t.id,
            update_options_input(Some(qp), None, None, None),
        )
        .await?;
    }
    if body.monitored != t.monitored {
        t = set_monitored(&st, &conn, &t.id, body.monitored).await?;
    }
    let qp = qp_id(&st, &conn, &t).await?;
    convert::movie(&t, qp)
        .map(|m| (StatusCode::ACCEPTED, Json(m)))
        .ok_or_else(|| AppError::NotFound("title not found".into()))
}

pub async fn update_series(
    State(st): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
    Json(body): Json<SeriesWrite>,
) -> Result<(StatusCode, Json<SonarrSeries>), AppError> {
    require(&conn, Flavor::Sonarr)?;
    let mut t = find_title(&st, &conn, id).await?;
    let qp_change = changed_quality_profile(&st, &conn, &t, body.quality_profile_id).await?;
    let mut d = detail(&st, &conn, &t).await?;
    let selection = monitored_seasons(&body.seasons);
    let mut current: Vec<i64> = d
        .seasons
        .iter()
        .filter(|s| s.kind == scryer::CollectionKind::Season && s.monitored)
        .filter_map(|s| s.index.trim().parse().ok())
        .collect();
    current.sort_unstable();
    let selection_change = !body.seasons.is_empty() && selection != current;
    let mut mutated = false;
    if qp_change.is_some() || selection_change {
        t = update_options(
            &st,
            &conn,
            &t.id,
            update_options_input(
                qp_change,
                None,
                selection_change.then_some(update_title::MonitorTypeValue::ADVANCED),
                selection_change.then_some(selection),
            ),
        )
        .await?;
        mutated = true;
    }
    if body.monitored != t.monitored {
        t = set_monitored(&st, &conn, &t.id, body.monitored).await?;
        mutated = true;
    }
    if mutated {
        d = detail(&st, &conn, &t).await?;
    }
    let qp = qp_id(&st, &conn, &t).await?;
    convert::series(&t, Some(&d), qp)
        .map(|s| (StatusCode::ACCEPTED, Json(s)))
        .ok_or_else(|| AppError::NotFound("title not found".into()))
}

pub async fn delete_title(
    State(st): State<AppState>,
    conn: Conn,
    Path((_, id)): Path<(String, i32)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let t = find_title(&st, &conn, id).await?;
    st.scryer
        .run::<DeleteTitle>(
            &conn.key,
            delete_title::Variables {
                input: delete_title::DeleteTitleInput {
                    title_id: t.id,
                    delete_files_on_disk: Some(false),
                    preview_fingerprint: None,
                    typed_confirmation: None,
                },
            },
        )
        .await?;
    Ok(Json(serde_json::json!({})))
}
