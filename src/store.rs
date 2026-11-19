use std::path::Path;
use std::str::FromStr;

use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use crate::domain::{Facet, Flavor};

type Result<T> = std::result::Result<T, sqlx::Error>;

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    Episode,
    EpisodeFile,
    Queue,
    History,
    QualityProfile,
    Tag,
    RootFolder,
}

impl IdKind {
    fn as_str(self) -> &'static str {
        match self {
            IdKind::Episode => "episode",
            IdKind::EpisodeFile => "episodefile",
            IdKind::Queue => "queue",
            IdKind::History => "history",
            IdKind::QualityProfile => "qualityprofile",
            IdKind::Tag => "tag",
            IdKind::RootFolder => "rootfolder",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub id: i32,
    pub library_id: String,
    pub flavor: Flavor,
    pub body: Value,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

fn flavor_str(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Radarr => "radarr",
        Flavor::Sonarr => "sonarr",
    }
}

fn facet_str(facet: Facet) -> &'static str {
    match facet {
        Facet::Movie => "movie",
        Facet::Series => "series",
        Facet::Anime => "anime",
    }
}

fn decode(e: impl std::error::Error + Send + Sync + 'static) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(e))
}

fn field(body: &Value, name: &str) -> Option<String> {
    body.get("fields")?
        .as_array()?
        .iter()
        .find(|f| f.get("name").and_then(Value::as_str) == Some(name))?
        .get("value")?
        .as_str()
        .map(str::to_owned)
}

fn notification_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Notification> {
    let body: Value = serde_json::from_str(row.try_get::<&str, _>("body")?).map_err(decode)?;
    let flavor = match row.try_get::<&str, _>("flavor")? {
        "radarr" => Flavor::Radarr,
        "sonarr" => Flavor::Sonarr,
        other => {
            return Err(sqlx::Error::Decode(
                format!("unknown flavor {other:?}").into(),
            ));
        }
    };
    Ok(Notification {
        id: row.try_get("id")?,
        library_id: row.try_get("library_id")?,
        flavor,
        url: field(&body, "url").unwrap_or_default(),
        username: field(&body, "username"),
        password: field(&body, "password"),
        body,
    })
}

fn with_id(mut body: Value, id: i32) -> Value {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("id".into(), Value::from(id));
    }
    body
}

fn is_path_prefix(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

impl Store {
    pub async fn open(path: &Path) -> Result<Store> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await?;
        Self::migrated(pool).await
    }

    pub async fn open_in_memory() -> Result<Store> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        Self::migrated(pool).await
    }

    async fn migrated(pool: SqlitePool) -> Result<Store> {
        sqlx::migrate!().run(&pool).await?;
        Ok(Store { pool })
    }

    pub async fn arr_id(&self, kind: IdKind, library_id: &str, scryer_key: &str) -> Result<i32> {
        sqlx::query(
            "INSERT INTO arr_ids (kind, library_id, scryer_key) VALUES (?, ?, ?) \
             ON CONFLICT (kind, library_id, scryer_key) DO NOTHING",
        )
        .bind(kind.as_str())
        .bind(library_id)
        .bind(scryer_key)
        .execute(&self.pool)
        .await?;
        sqlx::query_scalar(
            "SELECT arr_id FROM arr_ids WHERE kind = ? AND library_id = ? AND scryer_key = ?",
        )
        .bind(kind.as_str())
        .bind(library_id)
        .bind(scryer_key)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn scryer_key(
        &self,
        kind: IdKind,
        library_id: &str,
        arr_id: i32,
    ) -> Result<Option<String>> {
        sqlx::query_scalar(
            "SELECT scryer_key FROM arr_ids WHERE kind = ? AND library_id = ? AND arr_id = ?",
        )
        .bind(kind.as_str())
        .bind(library_id)
        .bind(arr_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn insert_notification(
        &self,
        library_id: &str,
        flavor: Flavor,
        body: Value,
    ) -> Result<Notification> {
        let mut tx = self.pool.begin().await?;
        let id: i32 = sqlx::query_scalar(
            "INSERT INTO notifications (library_id, flavor, body) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(library_id)
        .bind(flavor_str(flavor))
        .bind(body.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let body = with_id(body, id);
        sqlx::query("UPDATE notifications SET body = ? WHERE id = ?")
            .bind(body.to_string())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Notification {
            id,
            library_id: library_id.to_owned(),
            flavor,
            url: field(&body, "url").unwrap_or_default(),
            username: field(&body, "username"),
            password: field(&body, "password"),
            body,
        })
    }

    pub async fn update_notification(
        &self,
        library_id: &str,
        id: i32,
        body: Value,
    ) -> Result<Option<Notification>> {
        let body = with_id(body, id);
        let row = sqlx::query(
            "UPDATE notifications SET body = ? WHERE id = ? AND library_id = ? \
             RETURNING id, library_id, flavor, body",
        )
        .bind(body.to_string())
        .bind(id)
        .bind(library_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(notification_from_row).transpose()
    }

    pub async fn notification(&self, library_id: &str, id: i32) -> Result<Option<Notification>> {
        sqlx::query(
            "SELECT id, library_id, flavor, body FROM notifications \
             WHERE library_id = ? AND id = ?",
        )
        .bind(library_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .as_ref()
        .map(notification_from_row)
        .transpose()
    }

    pub async fn notifications(&self, library_id: &str) -> Result<Vec<Notification>> {
        sqlx::query(
            "SELECT id, library_id, flavor, body FROM notifications \
             WHERE library_id = ? ORDER BY id",
        )
        .bind(library_id)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(notification_from_row)
        .collect()
    }

    pub async fn all_notifications(&self) -> Result<Vec<Notification>> {
        sqlx::query("SELECT id, library_id, flavor, body FROM notifications ORDER BY id")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(notification_from_row)
            .collect()
    }

    pub async fn delete_notification(&self, library_id: &str, id: i32) -> Result<bool> {
        let res = sqlx::query("DELETE FROM notifications WHERE id = ? AND library_id = ?")
            .bind(id)
            .bind(library_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn set_library_roots(
        &self,
        library_id: &str,
        facet: Facet,
        roots: &[String],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM library_roots WHERE library_id = ?")
            .bind(library_id)
            .execute(&mut *tx)
            .await?;
        for root in roots {
            sqlx::query(
                "INSERT INTO library_roots (library_id, facet, path) VALUES (?, ?, ?) \
                 ON CONFLICT (library_id, path) DO NOTHING",
            )
            .bind(library_id)
            .bind(facet_str(facet))
            .bind(root)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// Records which library each scryer title belongs to. A title that moved
    /// to another library loses its remembered path.
    pub async fn remember_titles(&self, titles: &[(&str, &str)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for &(title_id, library_id) in titles {
            sqlx::query(
                "INSERT INTO titles (title_id, library_id) VALUES (?, ?) \
                 ON CONFLICT (title_id) DO UPDATE SET \
                 path = CASE WHEN library_id = excluded.library_id THEN path END, \
                 library_id = excluded.library_id",
            )
            .bind(title_id)
            .bind(library_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// Records a title's library together with its folder.
    pub async fn remember_title_path(
        &self,
        title_id: &str,
        library_id: &str,
        path: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO titles (title_id, library_id, path) VALUES (?, ?, ?) \
             ON CONFLICT (title_id) DO UPDATE SET \
             library_id = excluded.library_id, path = excluded.path",
        )
        .bind(title_id)
        .bind(library_id)
        .bind(path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The library a scryer title belongs to and its last known folder.
    pub async fn title_library(&self, title_id: &str) -> Result<Option<(String, Option<String>)>> {
        sqlx::query_as("SELECT library_id, path FROM titles WHERE title_id = ?")
            .bind(title_id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn forget_title(&self, title_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM titles WHERE title_id = ?")
            .bind(title_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn library_for_path(&self, facet: Facet, path: &str) -> Result<Option<String>> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT library_id, path FROM library_roots WHERE facet = ?")
                .bind(facet_str(facet))
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .filter(|(_, root)| is_path_prefix(root, path))
            .max_by_key(|(_, root)| root.trim_end_matches('/').len())
            .map(|(library_id, _)| library_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn arr_id_is_stable() {
        let s = Store::open_in_memory().await.unwrap();
        let a = s.arr_id(IdKind::Episode, "L1", "k1").await.unwrap();
        let b = s.arr_id(IdKind::Episode, "L1", "k1").await.unwrap();
        let c = s.arr_id(IdKind::Episode, "L1", "k2").await.unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[tokio::test]
    async fn scryer_key_round_trips() {
        let s = Store::open_in_memory().await.unwrap();
        let id = s.arr_id(IdKind::Queue, "L1", "abc").await.unwrap();
        assert_eq!(
            s.scryer_key(IdKind::Queue, "L1", id)
                .await
                .unwrap()
                .as_deref(),
            Some("abc")
        );
        assert_eq!(
            s.scryer_key(IdKind::Queue, "L1", id + 100).await.unwrap(),
            None
        );
        assert_eq!(s.scryer_key(IdKind::History, "L1", id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn notification_fields_are_parsed() {
        let s = Store::open_in_memory().await.unwrap();
        let body = json!({"name":"Tofa (x)","fields":[
            {"name":"url","value":"http://t/api/v1/hooks/radarr/3"},
            {"name":"username","value":"tofa"},
            {"name":"password","value":"s3cret"}]});
        let n = s
            .insert_notification("L1", Flavor::Radarr, body)
            .await
            .unwrap();
        assert_eq!(n.url, "http://t/api/v1/hooks/radarr/3");
        assert_eq!(n.username.as_deref(), Some("tofa"));
        assert_eq!(n.password.as_deref(), Some("s3cret"));
        assert_eq!(n.body["id"], json!(n.id));

        let listed = s.notifications("L1").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].body["id"], json!(n.id));
        assert!(s.notifications("L2").await.unwrap().is_empty());

        let updated = s
            .update_notification(
                "L1",
                n.id,
                json!({"fields":[{"name":"url","value":"http://u"}]}),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.url, "http://u");
        assert_eq!(updated.password, None);
        assert_eq!(updated.body["id"], json!(n.id));
        assert!(
            s.update_notification("L1", n.id + 9, json!({}))
                .await
                .unwrap()
                .is_none()
        );

        assert!(!s.delete_notification("L2", n.id).await.unwrap());
        assert!(s.delete_notification("L1", n.id).await.unwrap());
        assert!(s.notifications("L1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn library_for_path_uses_component_prefix() {
        let s = Store::open_in_memory().await.unwrap();
        s.set_library_roots("L1", Facet::Series, &["/media2/library/tv".into()])
            .await
            .unwrap();
        s.set_library_roots("L2", Facet::Series, &["/media2/library/tv2/".into()])
            .await
            .unwrap();
        assert_eq!(
            s.library_for_path(Facet::Series, "/media2/library/tv2/Show/S01/e.mkv")
                .await
                .unwrap()
                .as_deref(),
            Some("L2")
        );
        assert_eq!(
            s.library_for_path(Facet::Series, "/media2/library/tv/Show/x.mkv")
                .await
                .unwrap()
                .as_deref(),
            Some("L1")
        );
        assert_eq!(
            s.library_for_path(Facet::Series, "/other/x").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn library_for_path_longest_root_wins_and_roots_replace() {
        let s = Store::open_in_memory().await.unwrap();
        s.set_library_roots("A", Facet::Movie, &["/m".into()])
            .await
            .unwrap();
        s.set_library_roots("B", Facet::Movie, &["/m/4k".into()])
            .await
            .unwrap();
        assert_eq!(
            s.library_for_path(Facet::Movie, "/m/4k/x.mkv")
                .await
                .unwrap()
                .as_deref(),
            Some("B")
        );
        s.set_library_roots("B", Facet::Movie, &["/n".into()])
            .await
            .unwrap();
        assert_eq!(
            s.library_for_path(Facet::Movie, "/m/4k/x.mkv")
                .await
                .unwrap()
                .as_deref(),
            Some("A")
        );
    }

    #[tokio::test]
    async fn library_for_path_respects_facet() {
        let s = Store::open_in_memory().await.unwrap();
        s.set_library_roots("L1", Facet::Movie, &["/media/movies".into()])
            .await
            .unwrap();
        assert_eq!(
            s.library_for_path(Facet::Series, "/media/movies/x.mkv")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            s.library_for_path(Facet::Movie, "/media/movies/x.mkv")
                .await
                .unwrap()
                .as_deref(),
            Some("L1")
        );
    }

    #[tokio::test]
    async fn title_library_tracks_moves_and_paths() {
        let s = Store::open_in_memory().await.unwrap();
        assert_eq!(s.title_library("t1").await.unwrap(), None);
        s.remember_titles(&[("t1", "L1"), ("t2", "L1")])
            .await
            .unwrap();
        assert_eq!(
            s.title_library("t1").await.unwrap(),
            Some(("L1".into(), None))
        );
        s.remember_title_path("t1", "L1", "/m/A").await.unwrap();
        s.remember_titles(&[("t1", "L1")]).await.unwrap();
        assert_eq!(
            s.title_library("t1").await.unwrap(),
            Some(("L1".into(), Some("/m/A".into())))
        );
        s.remember_titles(&[("t1", "L2")]).await.unwrap();
        assert_eq!(
            s.title_library("t1").await.unwrap(),
            Some(("L2".into(), None))
        );
        s.forget_title("t1").await.unwrap();
        assert_eq!(s.title_library("t1").await.unwrap(), None);
        assert!(s.title_library("t2").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn open_creates_file_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let id = {
            let s = Store::open(&path).await.unwrap();
            s.arr_id(IdKind::Tag, "L1", "t").await.unwrap()
        };
        let s = Store::open(&path).await.unwrap();
        assert_eq!(s.arr_id(IdKind::Tag, "L1", "t").await.unwrap(), id);
    }
}
