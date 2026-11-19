use serde::Serialize;

use crate::domain::Facet;
use crate::scryer::ops::{
    add_title, set_title_monitored, title_detail, titles_by_external_ids, titles_page, update_title,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionKind {
    Season,
    Movie,
    Arc,
    Specials,
    Other(String),
}

#[derive(Debug, Clone)]
pub struct Title {
    pub id: String,
    pub library_id: String,
    pub name: String,
    pub facet: Facet,
    pub monitored: bool,
    pub year: Option<i64>,
    pub slug: Option<String>,
    pub overview: Option<String>,
    pub tags: Vec<String>,
    pub external_ids: Vec<(String, String)>,
    pub imdb_id: Option<String>,
    pub quality_profile_id: Option<String>,
    pub root_folder_path: Option<String>,
    pub min_availability: Option<String>,
    pub monitor_type: Option<String>,
    pub size_bytes: Option<i64>,
    pub episodes_owned: Option<i64>,
    pub episodes_monitored: Option<i64>,
    pub episodes_total: Option<i64>,
    pub created_at: String,
    pub poster_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MediaFile {
    pub id: String,
    pub path: String,
    pub episode_id: Option<String>,
    pub size_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct EpisodeView {
    pub id: String,
    pub season_number: Option<String>,
    pub episode_number: Option<String>,
    pub title: Option<String>,
    pub air_date: Option<String>,
    pub monitored: bool,
    pub media_file_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Season {
    pub id: String,
    pub index: String,
    pub kind: CollectionKind,
    pub monitored: bool,
    pub owned: i64,
    pub monitored_count: i64,
    pub total: i64,
    pub episodes: Vec<EpisodeView>,
}

/// A title from a catalog page, with its collections but no episodes.
#[derive(Debug, Clone)]
pub struct ListedTitle {
    pub title: Title,
    pub seasons: Vec<Season>,
}

#[derive(Debug, Clone)]
pub struct TitleDetail {
    pub title: Title,
    pub seasons: Vec<Season>,
    pub media_files: Vec<MediaFile>,
}

fn enum_string<T: Serialize>(v: &T) -> Option<String> {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => Some(s),
        _ => None,
    }
}

fn facet_of<T: Serialize>(v: &T) -> Facet {
    enum_string(v)
        .and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok())
        .unwrap_or_else(|| {
            tracing::warn!("unknown scryer facet; treating as MOVIE");
            Facet::Movie
        })
}

fn collection_kind<T: Serialize>(t: &T) -> CollectionKind {
    match enum_string(t).as_deref() {
        Some("SEASON") => CollectionKind::Season,
        Some("MOVIE") => CollectionKind::Movie,
        Some("ARC") => CollectionKind::Arc,
        Some("SPECIALS") => CollectionKind::Specials,
        other => CollectionKind::Other(other.unwrap_or_default().to_string()),
    }
}

macro_rules! title_from_fields {
    ($($module:ident),+ $(,)?) => {$(
        impl From<$module::TitleFields> for Title {
            fn from(t: $module::TitleFields) -> Self {
                Title {
                    id: t.id,
                    library_id: t.library_id,
                    name: t.name,
                    facet: facet_of(&t.facet),
                    monitored: t.monitored,
                    year: t.year,
                    slug: t.slug,
                    overview: t.overview,
                    tags: t.tags,
                    external_ids: t.external_ids.into_iter().map(|e| (e.source, e.value)).collect(),
                    imdb_id: t.imdb_id,
                    quality_profile_id: t.quality_profile_id,
                    root_folder_path: t.root_folder_path,
                    min_availability: t.min_availability,
                    monitor_type: t.monitor_type.as_ref().and_then(enum_string),
                    size_bytes: t.size_bytes,
                    episodes_owned: t.episodes_owned,
                    episodes_monitored: t.episodes_monitored,
                    episodes_total: t.episodes_total,
                    created_at: t.created_at,
                    poster_url: t.poster_url,
                }
            }
        }
    )+};
}

title_from_fields!(
    titles_page,
    titles_by_external_ids,
    title_detail,
    add_title,
    update_title,
    set_title_monitored
);

impl From<title_detail::TitleDetailTitle> for TitleDetail {
    fn from(t: title_detail::TitleDetailTitle) -> Self {
        let seasons = t
            .collections
            .into_iter()
            .map(|c| Season {
                id: c.id,
                index: c.collection_index,
                kind: collection_kind(&c.collection_type),
                monitored: c.monitored,
                owned: c.episodes_owned.unwrap_or(0),
                monitored_count: c.episodes_monitored.unwrap_or(0),
                total: c.episodes_total.unwrap_or(0),
                episodes: c
                    .episodes
                    .into_iter()
                    .map(|e| EpisodeView {
                        id: e.id,
                        season_number: e.season_number,
                        episode_number: e.episode_number,
                        title: e.title,
                        air_date: e.air_date,
                        monitored: e.monitored,
                        media_file_ids: e.media_files.into_iter().map(|m| m.id).collect(),
                    })
                    .collect(),
            })
            .collect();
        let media_files = t
            .media_files
            .into_iter()
            .map(|m| MediaFile {
                id: m.id,
                path: m.file_path,
                episode_id: m.episode_id,
                size_bytes: m.size_bytes,
            })
            .collect();
        TitleDetail {
            title: t.title_fields.into(),
            seasons,
            media_files,
        }
    }
}

impl From<titles_page::TitlesPageTitlesItems> for ListedTitle {
    fn from(t: titles_page::TitlesPageTitlesItems) -> Self {
        ListedTitle {
            title: t.title_fields.into(),
            seasons: t
                .collections
                .into_iter()
                .map(|c| Season {
                    id: c.id,
                    index: c.collection_index,
                    kind: collection_kind(&c.collection_type),
                    monitored: c.monitored,
                    owned: c.episodes_owned.unwrap_or(0),
                    monitored_count: c.episodes_monitored.unwrap_or(0),
                    total: c.episodes_total.unwrap_or(0),
                    episodes: Vec::new(),
                })
                .collect(),
        }
    }
}
