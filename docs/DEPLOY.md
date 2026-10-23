# Deploying OpenSpaceExplorer to https://ose.jfly.dev/

Mirrors how Lost In Time is hosted (`LostInTime/docs/stream/DEPLOY.md`): a Docker container on VM 150
`docker.lan` (10.205.25.14), nginx with certbot on prod-prox (CT 100, 10.205.25.118) in front, and a
DNS-only Cloudflare record for the `jfly.dev` name pointing at prod-prox's LAN address, so the site is
reachable on the home network and over Tailscale.

The one difference: Lost In Time's image is built on docker.lan from a `git archive`. Here GitHub
Actions builds it, because the build needs no GPU, and a push to `main` is the deploy.

```
 push to main ──► .github/workflows/deploy.yml ──► ghcr.io/unknown201/openspaceexplorer:latest
                                                            │ pulled every 5 min by watchtower
 browser ──► ose.jfly.dev (nginx, prod-prox) ──► docker.lan:8800 ──► ose-server (docker/compose.yaml)
                                                                       serves / (WASM client), /health
```

## What is built

`docker/Dockerfile`, two stages:
- `rust:1-bookworm`: `cargo build --release -p ose-server`, then `trunk build --release` of
  `crates/ose-client` (trunk 0.21.14 pinned by `TRUNK_VERSION`; it downloads the wasm-bindgen-cli that
  matches `Cargo.lock`).
- `debian:bookworm-slim`, user `ose` (uid 10001), `/data` as the working directory and volume.
  `ose-server --listen 0.0.0.0:7777 --static-dir /app/web`: the client at `/` (unknown paths get
  `index.html`), `/health` beside it. Port 7777 inside the container.

`.github/workflows/deploy.yml` pushes it to GHCR as `:latest` and `:sha-<commit>` with the workflow's
own `GITHUB_TOKEN`; no secret to add. Build layers are cached in GitHub's action cache.

## One-time setup (Josh)

1. **GHCR package public**, once the first `deploy` run has pushed it: GitHub > unknown201 > Packages >
   `openspaceexplorer` > Package settings > Change visibility > Public (the repository is public, and
   this lets docker.lan pull with no credentials). Also "Manage Actions access": the repository with
   write role, so later pushes keep working (GitHub sets this on the first push from the workflow).

2. **docker.lan** (as `josh`, beside Frigate and `lostintime-stream`):
   ```sh
   sudo mkdir -p /opt/openspaceexplorer && sudo chown josh /opt/openspaceexplorer
   cd /opt/openspaceexplorer
   curl -fsSLo compose.yaml https://raw.githubusercontent.com/unknown201/OpenSpaceExplorer/main/docker/compose.yaml
   docker compose up -d
   curl -s http://127.0.0.1:8800/health      # ok tick=...
   ```
   Host port 8800 (7777 and 8790-8794 there are Lost In Time's, on the host network). The watchtower
   service in the same compose file is scoped to this project only: it never touches Frigate or
   Lost In Time's containers.

3. **prod-prox** (CT 100), a new site `/etc/nginx/sites-available/ose`, linked into `sites-enabled`:
   ```nginx
   server {
       listen 80;
       listen [::]:80;
       server_name ose.jfly.dev;
       location / {
           proxy_pass http://docker.lan:8800/;
           proxy_http_version 1.1;
           proxy_set_header Host $host;
           proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
           proxy_set_header X-Forwarded-Proto $scheme;
           # WebSocket (the game connection, Phase 3)
           proxy_set_header Upgrade $http_upgrade;
           proxy_set_header Connection "upgrade";
           proxy_read_timeout 1h;
       }
   }
   ```
   `nginx -t && systemctl reload nginx`, then `certbot --nginx -d ose.jfly.dev` (adds the 443 server
   and the redirect, as it did for lostintime.jfly.dev). certbot's HTTP-01 challenge needs port 80
   reachable from the internet; if lostintime.jfly.dev was issued with the Cloudflare DNS plugin
   instead, use the same: `certbot -d ose.jfly.dev --dns-cloudflare ...`.
   Where the LostInTime site uses `include /etc/nginx/snippets/proxy-no-auth.conf;`, that snippet can
   replace the `proxy_set_header` lines above.

4. **Cloudflare DNS** for `jfly.dev`: an `A` record `ose` -> `10.205.25.118`, proxy status **DNS only**
   (grey cloud), like `lostintime`.

5. Open https://ose.jfly.dev/ and https://ose.jfly.dev/health.

## Day to day

- Push to `main`; the `deploy` workflow takes a few minutes, watchtower picks the image up within five
  more. `docker logs -f openspaceexplorer` on docker.lan shows the new container start.
- Deploy a specific commit: edit the image tag in `compose.yaml` to `:sha-<commit>`, then
  `docker compose pull && docker compose up -d`; set it back to `:latest` to follow main again.
- Rollback: the same, with the previous commit's tag (every push keeps its `:sha-...` tag in GHCR).
- Game state lives in the `ose-data` volume (`/data` in the container); the image is stateless.

## Local check without Docker

```
cd crates/ose-client && trunk build --release && cd ../..
cargo run -p ose-server -- --listen 127.0.0.1:7777 --static-dir crates/ose-client/dist
```
