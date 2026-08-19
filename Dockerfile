# syntax=docker/dockerfile:1.7

# ────────────────────────────────────────────────────────────────────────────
# Stage 1 — frontend-builder
# Build the SvelteKit static bundle into longbox-web/frontend-dist/.
# ────────────────────────────────────────────────────────────────────────────
FROM node:20-alpine AS frontend-builder
RUN npm install -g pnpm@10
WORKDIR /build

# Copy lockfiles first so dependency installs cache across source-only edits.
COPY longbox-frontend/package.json longbox-frontend/pnpm-lock.yaml ./longbox-frontend/
WORKDIR /build/longbox-frontend
RUN pnpm install --frozen-lockfile

# Copy the rest of the frontend source and build.
COPY longbox-frontend/ ./
RUN pnpm build
# Output is at /build/longbox-web/frontend-dist/ (adapter-static writes one
# directory up per svelte.config.js).


# ────────────────────────────────────────────────────────────────────────────
# Stage 2 — backend-builder
# Compile longbox-web for aarch64-unknown-linux-musl with the frontend
# bundle in place so rust-embed bakes it into the binary.
# ────────────────────────────────────────────────────────────────────────────
FROM rust:1.95-alpine AS backend-builder
# musl-dev/sqlite-* cover the SQLite static link; g++ builds the bundled
# libunrar C++ (unrar-ng-sys) that gives the scanner + Phase B CBR support.
RUN apk add --no-cache musl-dev sqlite-dev sqlite-static pkgconfig perl make g++
WORKDIR /build

# Resolve the Rust musl target from BuildKit's TARGETARCH so this Dockerfile
# builds natively on both amd64 and arm64 runners (multi-arch CI). Each
# matrix runner builds only its own arch — no QEMU, no cross-linking.
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      amd64) echo "x86_64-unknown-linux-musl"  > /tmp/rust-target ;; \
      arm64) echo "aarch64-unknown-linux-musl" > /tmp/rust-target ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac
RUN rustup target add "$(cat /tmp/rust-target)"

# Copy the workspace + offline sqlx cache.
ENV SQLX_OFFLINE=true
COPY Cargo.toml Cargo.lock ./
COPY .sqlx/ ./.sqlx/
COPY longbox-core/ ./longbox-core/
COPY longbox-archive/ ./longbox-archive/
COPY longbox-db/ ./longbox-db/
COPY longbox-comicvine/ ./longbox-comicvine/
COPY longbox-metron/ ./longbox-metron/
COPY longbox-newznab/ ./longbox-newznab/
COPY longbox-downloader/ ./longbox-downloader/
COPY longbox-scanner/ ./longbox-scanner/
COPY longbox-postprocess/ ./longbox-postprocess/
COPY longbox-pull/ ./longbox-pull/
COPY longbox-scan-scheduler/ ./longbox-scan-scheduler/
COPY longbox-cv-enrichment/ ./longbox-cv-enrichment/
COPY longbox-webhooks/ ./longbox-webhooks/
COPY longbox-opds/ ./longbox-opds/
COPY longbox-web/ ./longbox-web/

# Replace any tracked frontend-dist placeholder with the freshly-built
# bundle from stage 1.
COPY --from=frontend-builder /build/longbox-web/frontend-dist/ ./longbox-web/frontend-dist/

# Static-link libsqlite3 from sqlite-static so the runtime image needs no
# libsqlite3 package.
ENV RUSTFLAGS="-C target-feature=+crt-static -L native=/usr/lib"

# Build for the resolved target, then stage the binary at a fixed,
# arch-independent path so the runtime COPY doesn't need the target triple.
RUN cargo build --release --target "$(cat /tmp/rust-target)" --package longbox-web \
 && cp "target/$(cat /tmp/rust-target)/release/longbox" /build/longbox


# ────────────────────────────────────────────────────────────────────────────
# Stage 3 — runtime
# Minimal Alpine image. Keeps sh + wget so we can shell in for debugging.
# ────────────────────────────────────────────────────────────────────────────
FROM alpine:3.20
# poppler-utils supplies `pdfinfo` and `pdftoppm`, which longbox-archive's
# PDF reader SHELLS OUT TO — it does not link a PDF crate. Without them the
# binary still starts, still scans, still imports a PDF into the library,
# and only fails when someone opens one in the reader. Nothing in
# `cargo test` can catch that: the tests run on a host where poppler is
# installed (CI installs it explicitly), so the image is the only place
# this dependency exists or can go missing.
RUN apk add --no-cache ca-certificates poppler-utils

RUN addgroup -g 1000 -S longbox \
 && adduser -u 1000 -S -G longbox -h /home/longbox longbox \
 && mkdir -p /data /library \
 && chown -R longbox:longbox /data /home/longbox

COPY --from=backend-builder /build/longbox /usr/local/bin/longbox
RUN chmod +x /usr/local/bin/longbox

USER longbox
WORKDIR /home/longbox

EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD wget -q --spider http://127.0.0.1:3000/api/health || exit 1

ENTRYPOINT ["/usr/local/bin/longbox"]
