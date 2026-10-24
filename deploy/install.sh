#!/bin/sh
# Runs on the ose container (CT 120 on piper) as root. Installs one release tarball
# (ose-server + web/, as .github/workflows/deploy.yml or tools/deploy.py make it) and restarts the
# service. Usage: install.sh /tmp/ose-release.tar.gz <release-id>
set -eu
tar="$1"; id="$2"
dir="/opt/ose/releases/$id"
mkdir -p "$dir"
tar --no-same-owner -xzf "$tar" -C "$dir"
chmod 755 "$dir/ose-server"
ln -sfn "$dir" /opt/ose/current
mkdir -p /var/lib/ose/assets
chown -R ose:ose /var/lib/ose 2>/dev/null || true
if ! cmp -s "$dir/ose.service" /etc/systemd/system/ose.service; then
  cp "$dir/ose.service" /etc/systemd/system/ose.service
  systemctl daemon-reload
  systemctl enable ose >/dev/null
fi
systemctl restart ose
# keep the last five releases for rollback (ln -sfn /opt/ose/releases/<id> /opt/ose/current && systemctl restart ose)
ls -1dt /opt/ose/releases/* | tail -n +6 | xargs -r rm -rf
rm -f "$tar"
# the server reads the terrain packs from the assets directory before it listens: give it up to 30 s
i=0
until curl -fsS http://127.0.0.1:7777/health; do
  i=$((i + 1))
  [ "$i" -ge 30 ] && exit 1
  sleep 1
done
echo " <- $id"
