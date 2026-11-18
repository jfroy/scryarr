# scryarr

scryarr presents each [scryer](https://github.com/scryer-media/scryer) media library to the tofa
media server as a Radarr or Sonarr v3 instance, and relays scryer's Generic Webhook events back to
tofa. tofa requests, searches and tracks titles through the bridge; scryer stays the system of record.

The bridge holds no scryer credential and stores none of tofa's API keys. Its only state is a small
SQLite file (`state.db`) of webhook registrations, which is safe to lose.

## Connecting tofa

Add one Radarr or Sonarr connection per scryer library (Admin, Integrations):

| Field | Value |
|---|---|
| URL | `http://<host>:8080/radarr/<libraryId>` for movie libraries, `http://<host>:8080/sonarr/<libraryId>` for series and anime libraries |
| API key | A scryer API key for a user with Manage titles and Resolve imports on that library |

The flavor must match the library's facet (`radarr` for `MOVIE`, `sonarr` for `SERIES` or `ANIME`);
a mismatch fails tofa's Test button. The API key is forwarded to scryer unchanged and never stored,
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
   `http://<host>:8080/scryer/events/<token>`, where `<token>` equals `SCRYARR_EVENTS_TOKEN`.
   A wrong token returns 404.
3. Subscribe it to grab, download, import complete, upgrade, rename, title added, title deleted and
   file deleted.

The bridge answers 503 with `Retry-After` when tofa is unreachable, so scryer's own retry applies.

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
