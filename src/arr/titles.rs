use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use super::Conn;
use super::convert;
use super::model::{RadarrMovie, SonarrSeason, SonarrSeries};
use crate::AppState;
use crate::domain::{Facet, Flavor};
use crate::error::AppError;
use crate::scryer;
use crate::scryer::ops::{
    MetadataMovie, MetadataSeries, TitleDetail, TitlesByExternalIds, TitlesPage, metadata_movie,
    metadata_series, title_detail, titles_by_external_ids, titles_page,
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
