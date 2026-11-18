#![allow(clippy::all)]
#![allow(dead_code)]

use graphql_client::GraphQLQuery;

type ID = String;
type DateTime = String;
type Date = String;
type Long = i64;
type JSON = serde_json::Value;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/scryer_version.graphql",
    response_derives = "Debug, Clone"
)]
pub struct ScryerVersion;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/libraries.graphql",
    response_derives = "Debug, Clone"
)]
pub struct Libraries;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/quality_profiles.graphql",
    response_derives = "Debug, Clone"
)]
pub struct QualityProfiles;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/root_folders.graphql",
    response_derives = "Debug, Clone"
)]
pub struct RootFolders;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/tag_definitions.graphql",
    response_derives = "Debug, Clone"
)]
pub struct TagDefinitions;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/titles_page.graphql",
    response_derives = "Debug, Clone"
)]
pub struct TitlesPage;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/titles_by_external_ids.graphql",
    response_derives = "Debug, Clone"
)]
pub struct TitlesByExternalIds;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/title_detail.graphql",
    response_derives = "Debug, Clone"
)]
pub struct TitleDetail;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/metadata_movie.graphql",
    response_derives = "Debug, Clone"
)]
pub struct MetadataMovie;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/metadata_series.graphql",
    response_derives = "Debug, Clone"
)]
pub struct MetadataSeries;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/add_title.graphql",
    response_derives = "Debug, Clone"
)]
pub struct AddTitle;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/update_title.graphql",
    response_derives = "Debug, Clone"
)]
pub struct UpdateTitle;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/delete_title.graphql",
    response_derives = "Debug, Clone"
)]
pub struct DeleteTitle;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/set_episode_monitored.graphql",
    response_derives = "Debug, Clone"
)]
pub struct SetEpisodeMonitored;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/trigger_search.graphql",
    response_derives = "Debug, Clone"
)]
pub struct TriggerSearch;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/queue_page.graphql",
    response_derives = "Debug, Clone"
)]
pub struct QueuePage;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/title_downloads.graphql",
    response_derives = "Debug, Clone"
)]
pub struct TitleDownloads;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/delete_download.graphql",
    response_derives = "Debug, Clone"
)]
pub struct DeleteDownload;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/ignore_download.graphql",
    response_derives = "Debug, Clone"
)]
pub struct IgnoreDownload;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/mark_failed.graphql",
    response_derives = "Debug, Clone"
)]
pub struct MarkFailed;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/title_history.graphql",
    response_derives = "Debug, Clone"
)]
pub struct TitleHistory;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/delete_episode_files_preview.graphql",
    response_derives = "Debug, Clone"
)]
pub struct DeleteEpisodeFilesPreview;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "graphql/schema.graphql",
    query_path = "graphql/delete_episode_files.graphql",
    response_derives = "Debug, Clone"
)]
pub struct DeleteEpisodeFiles;
