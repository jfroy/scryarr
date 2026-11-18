use std::collections::BTreeSet;

use scryer_plugin_sdk::{NotificationEventType as E, PluginNotificationRequest};

use crate::domain::Flavor;
use crate::tofa::{
    EpisodeFilePayload, EpisodePayload, FilePayload, MoviePayload, RenamedFilePayload,
    SeriesPayload, TofaPayload,
};

pub fn to_tofa(req: &PluginNotificationRequest, flavor: Flavor) -> Option<TofaPayload> {
    let movie = flavor == Flavor::Radarr;
    if req.event_type == E::Test {
        return Some(TofaPayload::event("Test"));
    }

    let mut p = match (req.event_type, movie) {
        (E::Grab, _) => TofaPayload::event("Grab"),
        (E::Download | E::ImportComplete | E::Upgrade, _) => TofaPayload::event("Download"),
        (E::Rename, _) => TofaPayload::event("Rename"),
        (E::FileDeleted, true) => TofaPayload::event("MovieFileDelete"),
        (E::FileDeleted, false) => TofaPayload::event("EpisodeFileDelete"),
        (E::TitleDeleted, true) => TofaPayload::event("MovieDelete"),
        (E::TitleDeleted, false) => TofaPayload::event("SeriesDelete"),
        (E::TitleAdded, true) => TofaPayload::event("MovieAdded"),
        (E::TitleAdded, false) => TofaPayload::event("SeriesAdd"),
        _ => return None,
    };

    let title = req.title.as_ref()?;
    let path = title.path.clone()?;
    if movie {
        let tmdb_id = title.external_ids.tmdb_id.as_deref()?.parse().ok()?;
        p.movie = Some(MoviePayload {
            tmdb_id,
            folder_path: path,
        });
    } else {
        let tvdb_id = title.external_ids.tvdb_id.as_deref()?.parse().ok()?;
        p.series = Some(SeriesPayload { tvdb_id, path });
    }

    let seasons = seasons(req);
    let file_path = || {
        req.media_files
            .first()
            .map(|f| f.path.clone())
            .or_else(|| req.file.as_ref()?.primary_path.clone())
    };

    match req.event_type {
        E::Grab if !movie => {
            p.episodes = Some(
                seasons
                    .iter()
                    .map(|&season_number| EpisodePayload { season_number })
                    .collect(),
            );
        }
        E::Download | E::ImportComplete | E::Upgrade => {
            set_file(&mut p, req, movie, file_path()?)?;
            if req.event_type == E::Upgrade {
                p.is_upgrade = Some(true);
                let deleted: Vec<FilePayload> = req
                    .media_files
                    .iter()
                    .filter_map(|f| f.previous_path.clone())
                    .map(|path| FilePayload { path })
                    .collect();
                p.deleted_files = Some(deleted);
            }
        }
        E::Rename => {
            let renamed: Vec<RenamedFilePayload> = req
                .media_files
                .iter()
                .filter_map(|f| {
                    Some(RenamedFilePayload {
                        path: f.path.clone(),
                        previous_path: f.previous_path.clone()?,
                    })
                })
                .collect();
            if movie {
                p.renamed_movie_files = Some(renamed);
            } else {
                p.renamed_episode_files = Some(renamed);
            }
        }
        E::FileDeleted => set_file(&mut p, req, movie, file_path()?)?,
        _ => {}
    }
    Some(p)
}

fn set_file(
    p: &mut TofaPayload,
    req: &PluginNotificationRequest,
    movie: bool,
    path: String,
) -> Option<()> {
    if movie {
        p.movie_file = Some(FilePayload { path });
    } else {
        let season_number = file_season(req, &path)?;
        p.episode_file = Some(EpisodeFilePayload {
            path,
            season_number,
        });
    }
    Some(())
}

fn file_season(req: &PluginNotificationRequest, path: &str) -> Option<i32> {
    let own = req
        .episodes
        .iter()
        .chain(req.episode.iter())
        .filter(|e| e.media_file_path.as_deref() == Some(path))
        .find_map(|e| e.season_number.as_deref()?.parse().ok());
    if own.is_some() {
        return own;
    }
    let all = seasons(req);
    if all.len() == 1 {
        all.into_iter().next()
    } else {
        None
    }
}

fn seasons(req: &PluginNotificationRequest) -> BTreeSet<i32> {
    req.episodes
        .iter()
        .chain(req.episode.iter())
        .filter_map(|e| e.season_number.as_deref()?.parse().ok())
        .collect()
}
