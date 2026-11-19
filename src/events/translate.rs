use std::collections::BTreeSet;

use scryer_plugin_sdk::{
    NotificationEventType as E, NotificationMediaUpdateType as U, PluginNotificationRequest,
};

use crate::domain::Flavor;
use crate::tofa::{
    EpisodeFilePayload, EpisodePayload, FilePayload, MoviePayload, RenamedFilePayload,
    SeriesPayload, TofaPayload,
};

/// The scryer event types the bridge forwards to tofa. scryer's `download`
/// event is a download *failure* (its dispatcher builds it from
/// `DownloadFailed`), so only `import_complete` and `upgrade` become tofa
/// `Download`s, and an `import_complete` that an `upgrade` duplicates is
/// skipped (see [`superseded_by_upgrade`]).
pub fn relayed(event_type: E) -> bool {
    matches!(
        event_type,
        E::Test
            | E::Grab
            | E::ImportComplete
            | E::Upgrade
            | E::Rename
            | E::FileDeleted
            | E::TitleDeleted
            | E::TitleAdded
    )
}

pub fn to_tofa(req: &PluginNotificationRequest, flavor: Flavor) -> Option<TofaPayload> {
    let movie = flavor == Flavor::Radarr;
    if req.event_type == E::Test {
        return Some(TofaPayload::event("Test"));
    }

    if superseded_by_upgrade(req) {
        return None;
    }

    let mut p = match (req.event_type, movie) {
        (E::Grab, _) => TofaPayload::event("Grab"),
        (E::ImportComplete | E::Upgrade, _) => TofaPayload::event("Download"),
        (E::Rename, _) => TofaPayload::event("Rename"),
        (E::FileDeleted, true) => TofaPayload::event("MovieFileDelete"),
        (E::FileDeleted, false) => TofaPayload::event("EpisodeFileDelete"),
        (E::TitleDeleted, true) => TofaPayload::event("MovieDelete"),
        (E::TitleDeleted, false) => TofaPayload::event("SeriesDelete"),
        (E::TitleAdded, true) => TofaPayload::event("MovieAdded"),
        (E::TitleAdded, false) => TofaPayload::event("SeriesAdd"),
        _ => return None,
    };

    // scryer sends `title_added` and `title_deleted` without a path; tofa
    // treats the folder as optional and keys the title by its external id.
    let title = req.title.as_ref()?;
    let path = title.path.clone();
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

    match req.event_type {
        E::Grab if !movie => {
            p.episodes = Some(
                seasons(req)
                    .into_iter()
                    .map(|season_number| EpisodePayload { season_number })
                    .collect(),
            );
        }
        E::ImportComplete | E::Upgrade => {
            set_file(&mut p, req, movie, imported_path(req)?)?;
            if req.event_type == E::Upgrade {
                p.is_upgrade = Some(true);
                p.deleted_files = Some(
                    updates(req, U::Deleted)
                        .map(|path| FilePayload {
                            path: path.to_owned(),
                        })
                        .collect(),
                );
            }
        }
        E::Rename => {
            let renamed = renamed_files(req);
            if movie {
                p.renamed_movie_files = Some(renamed);
            } else {
                p.renamed_episode_files = Some(renamed);
            }
        }
        E::FileDeleted => set_file(&mut p, req, movie, deleted_path(req)?)?,
        _ => {}
    }
    Some(p)
}

/// scryer emits an `upgrade` event next to the `import_complete` for a file
/// that replaced another, and both would become a tofa `Download`. The
/// `upgrade` one is kept because it carries `deletedFiles`.
///
/// A series import sets `import.upgrade` when any one file in the batch was an
/// upgrade, so only a single-file import is skipped: a batch can also hold new
/// episodes that no `upgrade` event covers.
fn superseded_by_upgrade(req: &PluginNotificationRequest) -> bool {
    req.event_type == E::ImportComplete
        && req
            .import
            .as_ref()
            .is_some_and(|i| i.upgrade && i.imported_count.unwrap_or(1) <= 1)
}

/// Paths in `file.media_updates` of one kind, in order. These are library
/// paths; `media_files[].previous_path` is the download source instead.
fn updates(req: &PluginNotificationRequest, kind: U) -> impl Iterator<Item = &str> {
    req.file
        .iter()
        .flat_map(|f| &f.media_updates)
        .filter(move |u| u.update_type == kind)
        .map(|u| u.path.as_str())
}

/// The file that now exists: scryer's `primary_path`, else the created
/// update, else the last media file that is not being replaced (an upgrade
/// lists the previous file before the current one).
fn imported_path(req: &PluginNotificationRequest) -> Option<String> {
    if let Some(p) = req.file.as_ref().and_then(|f| f.primary_path.clone()) {
        return Some(p);
    }
    if let Some(p) = updates(req, U::Created).next() {
        return Some(p.to_owned());
    }
    let replaced: Vec<&str> = updates(req, U::Deleted).collect();
    req.media_files
        .iter()
        .rev()
        .find(|f| !replaced.contains(&f.path.as_str()))
        .map(|f| f.path.clone())
}

/// For a delete, scryer's `primary_path` falls back to the deleted path.
fn deleted_path(req: &PluginNotificationRequest) -> Option<String> {
    req.file
        .as_ref()
        .and_then(|f| f.primary_path.clone())
        .or_else(|| updates(req, U::Deleted).next().map(str::to_owned))
        .or_else(|| req.media_files.first().map(|f| f.path.clone()))
}

/// scryer's rename emits each file as a `deleted` old path immediately followed
/// by the `created` new one.
fn renamed_files(req: &PluginNotificationRequest) -> Vec<RenamedFilePayload> {
    let list = req
        .file
        .as_ref()
        .map(|f| f.media_updates.as_slice())
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < list.len() {
        let (old, new) = (&list[i], &list[i + 1]);
        if old.update_type == U::Deleted && new.update_type == U::Created {
            out.push(RenamedFilePayload {
                path: new.path.clone(),
                previous_path: old.path.clone(),
            });
            i += 2;
        } else {
            i += 1;
        }
    }
    out
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
        let Some(season_number) = file_season(req, &path) else {
            tracing::debug!(
                event_type = req.event_type.as_str(),
                path,
                "sonarr file event has no season for its file; dropped"
            );
            return None;
        };
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
