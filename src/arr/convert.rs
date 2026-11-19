use serde_json::{Value, json};

use super::model::{
    MovieStatistics, RadarrMovie, SeasonStatistics, SeriesStatistics, SonarrSeason, SonarrSeries,
};
use crate::scryer;
use crate::scryer::CollectionKind;

pub fn external_id(title: &scryer::Title, source: &str) -> Option<i32> {
    title
        .external_ids
        .iter()
        .find(|(s, _)| s.eq_ignore_ascii_case(source))
        .and_then(|(_, v)| v.trim().parse().ok())
}

fn images(poster_url: Option<&str>) -> Vec<Value> {
    match poster_url.filter(|u| !u.is_empty()) {
        Some(url) => vec![json!({"coverType": "poster", "url": url, "remoteUrl": url})],
        None => Vec::new(),
    }
}

fn percent(owned: i64, monitored: i64) -> f64 {
    if monitored > 0 {
        owned as f64 / monitored as f64 * 100.0
    } else {
        0.0
    }
}

pub fn movie(title: &scryer::Title, qp_id: Option<i32>) -> Option<RadarrMovie> {
    let tmdb_id = external_id(title, "tmdb")?;
    let size = title.size_bytes.unwrap_or(0);
    let has_file = size > 0;
    Some(RadarrMovie {
        id: tmdb_id,
        title: title.name.clone(),
        sort_title: title.name.to_lowercase(),
        year: title.year.unwrap_or(0),
        tmdb_id,
        imdb_id: title.imdb_id.clone().filter(|s| !s.is_empty()),
        title_slug: title.slug.clone().unwrap_or_else(|| tmdb_id.to_string()),
        overview: title.overview.clone(),
        monitored: title.monitored,
        has_file,
        path: None,
        folder_name: None,
        root_folder_path: title.root_folder_path.clone(),
        quality_profile_id: qp_id.unwrap_or(0),
        minimum_availability: match title.min_availability.as_deref() {
            Some("in_cinemas") => "inCinemas".into(),
            Some(other) if !other.is_empty() => other.to_string(),
            _ => "released".into(),
        },
        tags: Vec::new(),
        added: title.created_at.clone(),
        size_on_disk: size,
        status: "released".into(),
        is_available: true,
        images: images(title.poster_url.as_deref()),
        statistics: MovieStatistics {
            movie_file_count: i64::from(has_file),
            size_on_disk: size,
        },
    })
}

pub fn series(
    title: &scryer::Title,
    collections: &[scryer::Season],
    qp_id: Option<i32>,
) -> Option<SonarrSeries> {
    let tvdb_id = external_id(title, "tvdb")?;
    let owned = title.episodes_owned.unwrap_or(0);
    let monitored = title.episodes_monitored.unwrap_or(0);
    let seasons: Vec<SonarrSeason> = collections
        .iter()
        .filter(|s| s.kind == CollectionKind::Season)
        .filter_map(|s| {
            let number: i32 = s.index.trim().parse().ok()?;
            Some(SonarrSeason {
                season_number: number,
                monitored: s.monitored,
                statistics: SeasonStatistics {
                    episode_file_count: s.owned,
                    episode_count: s.monitored_count,
                    total_episode_count: s.total,
                    size_on_disk: 0,
                    percent_of_episodes: percent(s.owned, s.monitored_count),
                },
            })
        })
        .collect();
    Some(SonarrSeries {
        id: tvdb_id,
        title: title.name.clone(),
        sort_title: title.name.to_lowercase(),
        year: title.year.unwrap_or(0),
        tvdb_id,
        tmdb_id: external_id(title, "tmdb"),
        imdb_id: title.imdb_id.clone().filter(|s| !s.is_empty()),
        title_slug: title.slug.clone().unwrap_or_else(|| tvdb_id.to_string()),
        overview: title.overview.clone(),
        monitored: title.monitored,
        path: None,
        root_folder_path: title.root_folder_path.clone(),
        quality_profile_id: qp_id.unwrap_or(0),
        language_profile_id: 1,
        series_type: if title.facet == crate::domain::Facet::Anime {
            "anime".into()
        } else {
            "standard".into()
        },
        season_folder: true,
        tags: Vec::new(),
        added: title.created_at.clone(),
        network: None,
        status: "continuing".into(),
        images: images(title.poster_url.as_deref()),
        statistics: SeriesStatistics {
            season_count: seasons.len() as i64,
            episode_file_count: owned,
            episode_count: monitored,
            total_episode_count: title.episodes_total.unwrap_or(0),
            size_on_disk: title.size_bytes.unwrap_or(0),
            percent_of_episodes: percent(owned, monitored),
        },
        seasons,
    })
}
