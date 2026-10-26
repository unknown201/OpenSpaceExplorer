# Deploying OpenSpaceExplorer to https://ose.jfly.dev/

Live since 2026-10-23. The same shape as Lost In Time and PrivateCoPilot: a build is shipped by scp
to a container on the house Proxmox host (piper) and runs there under systemd; nginx with certbot on
prod-prox fronts it as `ose.jfly.dev`; the DNS record is DNS-only and points at prod-prox's LAN
address, so the site is reachable on the home network and over Tailscale. No Docker, no registry.

```
 push to main ──► .github/workflows/deploy.yml (build on GitHub) ──scp──► ose container (CT 120 on piper)
                                                                              /opt/ose/current, ose.service
 browser ──► https://ose.jfly.dev (nginx, prod-prox) ──► ose.lan:7777 ──► ose-server
                                                                      serves / (WASM client), /health
```

## What is built and shipped

A release tarball: `ose-server` (release build for `x86_64-unknown-linux-musl`, a static binary that
runs on any Linux), `web/` (`trunk build --release` of `crates/ose-client`), and `deploy/ose.service`
and `deploy/install.sh`. On the container, `install.sh` unpacks it into `/opt/ose/releases/<commit>`,
points `/opt/ose/current` at it, installs the unit the first time, restarts the service and checks
`/health`. The last five releases are kept.

`ose-server --listen 0.0.0.0:7777 --static-dir /opt/ose/current/web --assets-dir /var/lib/ose/assets` runs as the system user `ose`
with `/var/lib/ose` as its working directory and only writable path. The server's state files,
`ose-accounts.json` (accounts) and `ose-save.json` (the universe, saved every 30 s and restored on
start), live there, outside the release folders, so they survive redeploys and rollbacks.

## The container (CT 120 `ose`, hostname `ose.lan`)

Provisioned 2026-10-23 as the house runbook says (Debian 13, 2 cores, 2 GB, 10 GB on local-lvm,
unprivileged, nesting on, starts on boot). Inside: `curl` and `ca-certificates`, the user `ose`,
`/opt/ose` and `/var/lib/ose`. Nothing else is installed there; builds happen elsewhere.

Useful, from piper as root:
```sh
pct exec 120 -- systemctl status ose --no-pager
pct exec 120 -- journalctl -u ose -n 50 --no-pager
pct exec 120 -- curl -s http://127.0.0.1:7777/health
pct exec 120 -- ls -l /opt/ose/releases          # rollback: ln -sfn <one of these> /opt/ose/current && systemctl restart ose
```

## prod-prox

Made with the house helper (`/root/proxy.sh ose ose.lan:7777`): `/etc/nginx/sites-enabled/ose`,
certbot certificate for `ose.jfly.dev` (auto-renewed), the default SSO-or-LAN gate
(`whitelist-auth.conf`). To open it to people outside the house, swap that include for
`proxy-no-auth.conf` as the Lost In Time `/play/` site does.

Guests sign in with Google: their email goes in `/etc/oauth2/emails.cfg` (who may sign in at all;
oauth2-proxy reloads it by itself) and in the `ose.jfly.dev` entry of
`/etc/nginx/conf.d/01-authz-emails.conf` (who may reach this site), then `nginx -t && systemctl
reload nginx`. One guest was added this way on 2026-10-24.

## How a deploy happens

- **From GitHub**: `.github/workflows/deploy.yml` on every push to `main`. It needs four repository
  secrets, `OSE_DEPLOY_HOST`, `OSE_DEPLOY_PORT`, `OSE_DEPLOY_USER`, `OSE_DEPLOY_KEY`: the address and
  forwarded SSH port GitHub's runners reach the house on (PrivateCoPilot's workflow uses the same
  kind of endpoint), a user that can run `install.sh` as root on the ose container, and its key.
  The workflow stores nothing on GitHub (no artifacts, no packages).
- **From a developer machine**: `python tools/deploy.py`, with `git config ose.piper` and
  `git config ose.piperKey` set (see its docstring). It cross-compiles with `rust-lld`
  (`rustup target add x86_64-unknown-linux-musl`), builds the client with trunk, and pushes the
  tarball through piper (`pct push`, `pct exec`). This is how the first release went out.

## Local check

Release builds and asset uploads prepare gzip sidecars with `tools/precompress.py`; the server also
compresses older exports while streaming. Material PNG previews are generated and cached on the server,
so existing asset installations benefit without a re-export. See [remote loading](loading-performance.md)
for bandwidth measurements, browser caching and reproduction commands.

```
cd crates/ose-client && trunk build --release --dist ../../target/web && cd ../..
cargo run -p ose-server -- --listen 127.0.0.1:7777 --static-dir target/web
```
