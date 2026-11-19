use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    pub app_name: String,
    pub version: String,
    pub instance_name: String,
    pub is_production: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityProfile {
    pub id: i32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootFolder {
    pub id: i32,
    pub path: String,
    pub free_space: Option<i64>,
    pub accessible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: i32,
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovieStatistics {
    pub movie_file_count: i64,
    pub size_on_disk: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadarrMovie {
    pub id: i32,
    pub title: String,
    pub sort_title: String,
    pub year: i64,
    pub tmdb_id: i32,
    pub imdb_id: Option<String>,
    pub title_slug: String,
    pub overview: Option<String>,
    pub monitored: bool,
    pub has_file: bool,
    pub path: Option<String>,
    pub folder_name: Option<String>,
    pub root_folder_path: Option<String>,
    pub quality_profile_id: i32,
    pub minimum_availability: String,
    pub tags: Vec<i32>,
    pub added: String,
    pub size_on_disk: i64,
    pub status: String,
    pub is_available: bool,
    pub images: Vec<Value>,
    pub statistics: MovieStatistics,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonStatistics {
    pub episode_file_count: i64,
    pub episode_count: i64,
    pub total_episode_count: i64,
    pub size_on_disk: i64,
    pub percent_of_episodes: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesStatistics {
    pub season_count: i64,
    pub episode_file_count: i64,
    pub episode_count: i64,
    pub total_episode_count: i64,
    pub size_on_disk: i64,
    pub percent_of_episodes: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SonarrSeason {
    pub season_number: i32,
    pub monitored: bool,
    pub statistics: SeasonStatistics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SonarrSeries {
    pub id: i32,
    pub title: String,
    pub sort_title: String,
    pub year: i64,
    pub tvdb_id: i32,
    pub tmdb_id: Option<i32>,
    pub imdb_id: Option<String>,
    pub title_slug: String,
    pub overview: Option<String>,
    pub monitored: bool,
    pub path: Option<String>,
    pub root_folder_path: Option<String>,
    pub quality_profile_id: i32,
    pub language_profile_id: i32,
    pub series_type: String,
    pub season_folder: bool,
    pub tags: Vec<i32>,
    pub added: String,
    pub network: Option<String>,
    pub status: String,
    pub images: Vec<Value>,
    pub seasons: Vec<SonarrSeason>,
    pub statistics: SeriesStatistics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub id: i32,
    pub series_id: i32,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: Option<String>,
    pub air_date_utc: Option<String>,
    pub monitored: bool,
    pub has_file: bool,
    pub episode_file_id: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeFile {
    pub id: i32,
    pub series_id: i32,
    pub season_number: i32,
    pub path: String,
    pub size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: i32,
    pub title: String,
    pub status: String,
    pub tracked_download_state: String,
    pub size: i64,
    pub sizeleft: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeleft: Option<String>,
    pub movie_id: Option<i32>,
    pub series_id: Option<i32>,
    pub episode_id: Option<i32>,
    pub season_number: Option<i32>,
    pub download_id: String,
    pub protocol: String,
    pub download_client: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuePage {
    pub page: i32,
    pub page_size: i32,
    pub total_records: i32,
    pub records: Vec<QueueItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: i32,
    pub event_type: String,
    pub date: String,
    pub download_id: Option<String>,
    pub source_title: String,
    pub movie_id: Option<i32>,
    pub series_id: Option<i32>,
    pub episode_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub id: i32,
    pub name: String,
    pub status: String,
    pub queued: String,
    pub started: String,
    pub ended: String,
}
