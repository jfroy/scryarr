# scryarr

scryarr presents each [scryer](https://github.com/scryer-media/scryer) media library to the tofa
media server as a Radarr or Sonarr v3 instance, and relays scryer's Generic Webhook events back to
tofa. tofa requests, searches and tracks titles through the bridge; scryer stays the system of record.

The bridge holds no scryer credential and stores none of tofa's API keys. Its only state is a small
SQLite file (`state.db`): tofa's webhook registrations, each library's root folders and the library
of each scryer title it has seen (both used to route scryer events), and the small integer IDs it
hands tofa for quality profiles, episodes, episode files, queue and history records, tags and root
folders. See [Losing state.db](#losing-statedb).

## Connecting tofa

Add one Radarr or Sonarr connection per scryer library (Admin, Integrations):

| Field | Value |
|---|---|
| URL | `http://<host>:8080/radarr/<libraryId>` for movie libraries, `http://<host>:8080/sonarr/<libraryId>` for series and anime libraries |
| API key | A scryer API key for a user with View, Manage titles and Resolve imports on that library |

The user needs View explicitly: scryer checks it as its own permission bit, not as something Manage
titles implies, and without it `libraries` returns `[]`, so every route answers 404.

The flavor must match the library's facet (`radarr` for `MOVIE`, `sonarr` for `SERIES` or `ANIME`);
a mismatch fails tofa's Test button. Webhook registrations and root folders change the bridge's own
state for the library, so those routes need Manage titles; a key that can only view the library gets
403 there. The API key is forwarded to scryer unchanged and never stored,
so scryer's own per-library permissions decide what each connection can do. Radarr movie IDs are
TMDB IDs and Sonarr series IDs are TVDB IDs; titles lacking that ID are not visible to tofa.

## Configuration

| Variable | Default | Description |
|---|---|---|
| `SCRYER_URL` | required | Base URL of scryer; GraphQL is reached at `$SCRYER_URL/graphql` |
| `SCRYER_EVENTS_TOKEN` | required | Non-empty secret path token for the scryer events endpoint |
| `SCRYARR_LISTEN` | `0.0.0.0:8080` | Listen address |
| `SCRYARR_DATA_DIR` | `/data` | Directory holding `state.db` |
| `RUST_LOG` | `info` | `tracing` filter |

Other endpoints: `GET /healthz` (returns `ok`) and `GET /metrics` (Prometheus).

## Scryer events channel

scryer pushes events to the bridge, which forwards them to the tofa webhooks tofa registered:

1. In scryer, install the Generic Webhook notification plugin from the first-party catalog.
2. Create a channel with method `POST`, content type `application/json` and URL
   `http://<host>:8080/scryer/events/<token>`, where `<token>` equals `SCRYER_EVENTS_TOKEN`.
   A wrong token returns 404.
3. Subscribe the channel to these events: Grabbed Release, Import Completed, Upgrade Imported,
   Rename, Title Added, Title Deleted and File Deleted. A channel without subscriptions receives
   nothing.

| scryer event | tofa event |
|---|---|
| `grab` | `Grab` |
| `import_complete`, `upgrade` | `Download` (an upgrade adds the replaced files as `deletedFiles`) |
| `rename` | `Rename` |
| `file_deleted` | `MovieFileDelete` / `EpisodeFileDelete` |
| `title_added` | `MovieAdded` / `SeriesAdd` |
| `title_deleted` | `MovieDelete` / `SeriesDelete` |
| `test` | `Test` |

scryer's `download` event reports a *failed* download, so the bridge does not relay it. scryer sends
both `import_complete` and `upgrade` for a file that replaced another; for a single-file import the
bridge relays only the `upgrade`, which carries `deletedFiles`, so tofa gets one `Download`. A
multi-file import is always relayed, since it may also hold new files that no `upgrade` covers.
Every other event type, known or not, is acknowledged with `200 {"delivered":0}` and dropped; tofa
also polls the queue and library, so a dropped event delays an update rather than losing it.

Events are routed to a library by the root folder that holds the title's folder. scryer sends
`title_added` (the folder does not exist yet) and `title_deleted` (the title is already gone) without
a folder, so those are routed by scryer title ID instead, using the library the bridge last saw the
title in: any tofa read or add of the title, or any earlier event for it, records that. A title the
bridge has never seen is dropped. Title deletes carry the folder from the title's last routed event,
when there was one.

The bridge answers 503 with `Retry-After` when tofa is unreachable. scryer retries only its
import-space health events; it logs and drops any other failed delivery, and tofa's polling catches
up.

## Losing state.db

Movie and series IDs are the TMDB and TVDB IDs themselves, so they survive. Everything else in
`state.db` does not:

- **Quality profiles and tags.** The profile IDs (default, 4K, anime) and tag IDs (default, anime)
  tofa saved in each connection may now name nothing or something else. Open each connection in
  tofa, re-pick its profiles and tags and save.
- **Event routing.** The bridge routes scryer events by the library's root folders, which it records
  when tofa reads `rootfolder`. Until tofa does that again, on the connection's Test or save, events
  for that library are dropped. Title added and deleted events are routed by the title's library,
  which the bridge learns again as tofa reads its library; until then they are dropped.
- **Webhook registrations.** tofa's webhooks are gone, so nothing reaches tofa until each connection
  is saved again, which registers it anew.

Saving each tofa connection once (after Test) covers all three. Episode, queue and history IDs are
short-lived for tofa, which re-reads them before acting.

## Container image

```sh
docker run --rm \
  -e SCRYER_URL=http://scryer:8080 \
  -e SCRYER_EVENTS_TOKEN=<token> \
  -v scryarr-data:/data \
  -p 8080:8080 \
  ghcr.io/jfroy/scryarr:latest
```

The image is multi-arch (`linux/amd64`, `linux/arm64`), runs as uid/gid 1000 on distroless, and works
with a read-only root filesystem given a writable `/data` and `/tmp`.

Releases are tagged `vX.Y.Z` and published as `ghcr.io/jfroy/scryarr:X.Y.Z`.

## Tracking scryer releases

scryarr is built against a vendored copy of scryer's GraphQL schema (`graphql/schema.graphql`) and a
pinned `scryer-plugin-sdk`. The `scryer-sync` workflow checks scryer's latest GitHub release every
four hours. When a new one appears, it:

1. Vendors that release's schema and plugin SDK version.
2. Builds and tests scryarr against them. Every GraphQL operation is checked against the schema at
   compile time.
3. Commits the result and cuts a patch release.

If scryarr no longer builds or passes its tests, the run fails and opens an issue titled
"scryer X.Y.Z breaks scryarr". scryer itself still updates; only the bridge waits for a fix.

## Safety

The bridge never deletes files except for tofa's `DELETE /api/v3/episodefile/{id}`; title deletion
always keeps files on disk. It never creates metadata requests or changes scryer settings, users or
libraries.

## More

The full operator setup, action mapping and event table are in the design spec
(`docs/superpowers/specs/2026-11-18-tofa-scryer-bridge-design.md` in the flatops repository, sections
"Connections and auth", "Events to webhooks", "Operator setup" and "Removal").

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

MIT licensed.
