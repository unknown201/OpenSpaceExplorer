# syntax=docker/dockerfile:1
# OpenSpaceExplorer as one Linux container: ose-server serving the trunk-built WASM client at /
# and the game endpoints beside it. Built and published by .github/workflows/deploy.yml on every
# push to main; run on docker.lan with docker/compose.yaml (see docs/DEPLOY.md). Locally:
#   docker build -f docker/Dockerfile -t openspaceexplorer . && docker run -p 7777:7777 openspaceexplorer

FROM rust:1-bookworm AS build
ARG TRUNK_VERSION=0.21.14
RUN rustup target add wasm32-unknown-unknown \
 && curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-gnu.tar.gz" \
    | tar -xz -C /usr/local/bin trunk
WORKDIR /src
COPY . .
# the server, then the web client (trunk fetches the wasm-bindgen-cli matching Cargo.lock itself)
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p ose-server \
 && install -D target/release/ose-server /out/ose-server \
 && cd crates/ose-client \
 && trunk build --release --dist /out/web

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin ose \
 && mkdir -p /data && chown ose /data
COPY --from=build /out /app
USER ose
WORKDIR /data
VOLUME /data
EXPOSE 7777/tcp
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD curl -fsS http://127.0.0.1:7777/health || exit 1
ENTRYPOINT ["/app/ose-server"]
CMD ["--listen", "0.0.0.0:7777", "--static-dir", "/app/web"]
