# LongBox — Deployment Runbook

This is the operations document. Everything below assumes you have a checkout
of this repo, Docker installed, and a ComicVine API key.

## Prerequisites

- **Docker** — Docker Desktop on macOS, or Docker Engine + Compose plugin
  on Linux. The compose file uses Compose v2 syntax. Examples in this
  document use the bundled plugin form `docker compose`. The standalone
  binary `docker-compose` (e.g. from Homebrew) accepts the same arguments
  and is a drop-in replacement on hosts where the plugin isn't installed
  (Colima users will hit this).
- **ComicVine API key** — free, get one at
  [comicvine.gamespot.com/api/](https://comicvine.gamespot.com/api/). The
  daily request budget on a free key (200/hour) is plenty for Phase A.
- **A comic library on disk** — an absolute path you'll bind-mount into the
  container read-only. Anything `.cbz` is indexable. `.cbr` and `.cb7`
  files are silently skipped in Phase A.

## First-run setup

1. Clone the repo.

2. Copy `.env.example` to `.env` and fill in:

   - `COMICVINE_API_KEY` — your CV key.
   - `LIBRARY_PATH` — absolute path on the host. macOS examples:
     `/Volumes/comics`, `/Users/you/Comics`.

3. **macOS only:** add `LIBRARY_PATH` to Docker Desktop's File Sharing
   allow-list. Docker Desktop → Settings → Resources → File Sharing. If
   `LIBRARY_PATH` isn't on this list, the bind mount appears to succeed but
   silently mounts an empty directory inside the container, and every scan
   will report zero files.

4. Bring it up:

   ```sh
   docker compose up -d
   ```

   The first build takes 5–10 minutes (downloads Rust, compiles
   dependencies, builds the frontend, statically links sqlite). Subsequent
   builds reuse the cached dependency layer and finish in well under a
   minute when only application code changed.

5. Open [http://localhost:3000/](http://localhost:3000/). You should see
   the dashboard. The catalog starts empty.

6. Add a series via the UI (`/add`), then trigger a scan from the dashboard
   or `/scans` page.

## Daily operations

```sh
# Follow logs (JSON to stderr, structured by tracing).
docker compose logs -f longbox

# Shell access (Alpine).
docker compose exec longbox sh

# Restart without rebuilding.
docker compose restart longbox

# Stop without removing the container or the data volume.
docker compose stop longbox

# Stop and remove the container; data volume is preserved.
docker compose down

# Stop and DESTROY the data volume — wipes the catalog (library files on
# the host are never touched). Use this for a clean install.
docker compose down -v
```

## Backups

The catalog state lives in the named Docker volume `longbox-data`, which
contains `longbox.db` (SQLite). Back this up to keep your watchlist,
parsing patterns, and scan history. Library files themselves are on the
host filesystem; whatever you already use for those is unchanged.

To pull the database out for manual backup:

```sh
docker compose exec longbox sh -c 'sqlite3 /data/longbox.db ".backup /data/longbox-backup.db"'
docker compose cp longbox:/data/longbox-backup.db ./longbox-backup.db
```

The DB uses WAL journal mode (per the standard pragmas applied at pool
open), so this online backup is safe with the container running.

## Updates

```sh
git pull
docker compose build
docker compose up -d
```

Schema migrations run automatically at startup. If a migration fails (rare
in Phase A; it would mean a bug in the migration file), the container
exits non-zero and `docker compose logs` shows the error.

## Apple Silicon notes

- The image is built for `aarch64-unknown-linux-musl` — native arch, no
  Rosetta emulation. Build time on an M-series Mac is the same as native.
- **Mac sleep pauses the container.** Phase A scans are manual, so this is
  harmless: when you wake the Mac, the container resumes. Phase A.5's
  scheduled scans will need a workaround (`caffeinate -i docker compose
  logs -f` while a scan runs, a host-side wake schedule, or move the
  container to a small always-on Linux box).
- **SMB-mounted libraries** must be in Docker Desktop's File Sharing list.
  Without it, the bind mount silently degrades to an empty volume — no
  error, just zero files found on scan.

## Multi-arch images

Phase A builds single-arch `linux/arm64` only. If you ever publish the
image publicly or run it on amd64 Linux, switch to `docker buildx` with
`--platform linux/amd64,linux/arm64`. That's a one-line change when you
actually need it; doubling the build time before it's needed costs more
than it saves.

## Health check

The container's `HEALTHCHECK` polls `GET /api/health` every 30s. You can
check status from the host:

```sh
docker compose ps                 # healthy / unhealthy / starting
docker inspect --format='{{.State.Health.Status}}' longbox
```

A failing health check doesn't restart the container by itself — that's a
Compose feature gated on `restart: unless-stopped` and Docker's own
liveness rules. If you want stricter recovery, layer it on with an
external supervisor (systemd, Kubernetes, etc.). Out of scope for Phase A.
