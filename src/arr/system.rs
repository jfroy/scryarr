use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use super::Conn;
use super::model::{QualityProfile, RootFolder, SystemStatus, Tag};
use crate::AppState;
use crate::domain::Flavor;
use crate::error::AppError;
use crate::scryer::ops::{
    QualityProfiles, ScryerVersion, TagDefinitions, quality_profiles, scryer_version,
    tag_definitions,
};
use crate::store::IdKind;

pub async fn status(State(st): State<AppState>, conn: Conn) -> Json<SystemStatus> {
    if let Ok(v) = st
        .scryer
        .run::<ScryerVersion>(&conn.key, scryer_version::Variables)
        .await
    {
        st.metrics.observe_scryer_version(&v.scryer_version);
    }
    let (app_name, version) = match conn.flavor {
        Flavor::Radarr => ("Radarr", "5.0.0.0"),
        Flavor::Sonarr => ("Sonarr", "4.0.0.0"),
    };
    Json(SystemStatus {
        app_name: app_name.into(),
        version: version.into(),
        instance_name: conn.library.name.clone(),
        is_production: true,
    })
}

pub async fn quality_profiles(
    State(st): State<AppState>,
    conn: Conn,
) -> Result<Json<Vec<QualityProfile>>, AppError> {
    let data = st
        .scryer
        .run::<QualityProfiles>(&conn.key, quality_profiles::Variables)
        .await?;
    let lib = &conn.library;
    let allowed: Vec<&str> = lib
        .quality_profile_id
        .iter()
        .chain(lib.request_quality_profile_ids.iter())
        .map(String::as_str)
        .collect();
    let mut out = Vec::new();
    for p in data.quality_profile_settings.profiles {
        if allowed.is_empty() || allowed.contains(&p.id.as_str()) {
            let id = st
                .store
                .arr_id(IdKind::QualityProfile, &conn.library_id, &p.id)
                .await?;
            out.push(QualityProfile { id, name: p.name });
        }
    }
    Ok(Json(out))
}

pub async fn root_folders(
    State(st): State<AppState>,
    conn: Conn,
) -> Result<Json<Vec<RootFolder>>, AppError> {
    let roots = &conn.library.roots;
    st.store
        .set_library_roots(&conn.library_id, conn.facet, roots)
        .await?;
    let mut out = Vec::new();
    for path in roots {
        let id = st
            .store
            .arr_id(IdKind::RootFolder, &conn.library_id, path)
            .await?;
        out.push(RootFolder {
            id,
            path: path.clone(),
            free_space: 0,
            accessible: true,
        });
    }
    Ok(Json(out))
}

pub async fn tags(State(st): State<AppState>, conn: Conn) -> Result<Json<Vec<Tag>>, AppError> {
    let data = st
        .scryer
        .run::<TagDefinitions>(&conn.key, tag_definitions::Variables)
        .await?;
    let mut out = Vec::new();
    for def in data.title_tag_definitions {
        let id = st
            .store
            .arr_id(IdKind::Tag, &conn.library_id, &def.id)
            .await?;
        out.push(Tag {
            id,
            label: def.label,
        });
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct NewTag {
    label: String,
}

pub async fn create_tag(
    State(st): State<AppState>,
    conn: Conn,
    Json(body): Json<NewTag>,
) -> Result<Json<Tag>, AppError> {
    let id = st
        .store
        .arr_id(
            IdKind::Tag,
            &conn.library_id,
            &format!("local:{}", body.label),
        )
        .await?;
    Ok(Json(Tag {
        id,
        label: body.label,
    }))
}

pub async fn language_profiles() -> Json<Vec<()>> {
    Json(Vec::new())
}
