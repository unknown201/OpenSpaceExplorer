#!/usr/bin/env python
"""Copy the exported web assets (assets/web, from tools/export_web_assets.py) into the ose container, where
ose-server serves them at /assets/ from /var/lib/ose/assets (docs/DEPLOY.md).

  python tools/push_assets.py                 # everything under assets/web
  python tools/push_assets.py bodies textures # only these subdirectories (plus index.json)

GitHub builds the site but has no game install, so this runs from a machine that has one, over the same
route as tools/deploy.py: scp the tarball to piper, `pct push` it into the container, unpack as the ose
user. The git config keys are the ones deploy.py uses:
  git config ose.piper root@<piper address>
  git config ose.piperKey <path to the private key>
  git config ose.ct 120
"""
import subprocess
import sys
import tempfile
from pathlib import Path
from precompress import prepare

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets" / "web"
REMOTE_DIR = "/var/lib/ose/assets"


def run(*cmd, cwd=ROOT):
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True)


def config(key, default=""):
    r = subprocess.run(["git", "config", "--get", key], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip() or default


def main() -> int:
    piper, key, ct = config("ose.piper"), config("ose.piperKey"), config("ose.ct", "120")
    if not piper or not key:
        sys.exit("push_assets: set `git config ose.piper root@host` and `git config ose.piperKey path` first")
    if not (ASSETS / "index.json").exists():
        sys.exit(f"push_assets: {ASSETS} has no index.json; run tools/export_web_assets.py first")
    subdirs = sys.argv[1:] or [d.name for d in ASSETS.iterdir() if d.is_dir()]
    members = ["index.json"] + subdirs
    prepare(ASSETS, members)
    members += [m + '.gz' for m in members if (ASSETS / m).is_file() and (ASSETS / (m + '.gz')).exists()]
    size = sum(f.stat().st_size for d in subdirs for f in (ASSETS / d).rglob("*") if f.is_file())
    print(f"pushing {', '.join(subdirs)} ({size / 1e6:.0f} MB) to {piper} -> ct {ct}:{REMOTE_DIR}")
    with tempfile.TemporaryDirectory() as tmp:
        tar = Path(tmp) / "ose-assets.tar.gz"
        # Python's tarfile rather than a tar binary: on Windows, subprocess may pick the system bsdtar, which
        # lacks --force-local, and the archive must carry uid/gid 0 for the container.
        import tarfile

        def anon(info: tarfile.TarInfo) -> tarfile.TarInfo:
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            return info

        print(f"+ tar {tar.name} <- {', '.join(members)}", flush=True)
        with tarfile.open(tar, "w:gz", compresslevel=3) as tf:
            for m in members:
                tf.add(ASSETS / m, arcname=m, filter=anon)
        print(f"tarball {tar.stat().st_size / 1e6:.0f} MB")
        run("scp", "-q", "-i", key, str(tar), f"{piper}:/root/ose-assets.tar.gz")
    remote = (
        f"pct push {ct} /root/ose-assets.tar.gz /tmp/ose-assets.tar.gz && rm /root/ose-assets.tar.gz && "
        f"pct exec {ct} -- sh -c 'mkdir -p {REMOTE_DIR} && tar --no-same-owner -xzf /tmp/ose-assets.tar.gz -C {REMOTE_DIR} "
        f"&& chown -R ose:ose {REMOTE_DIR} && rm /tmp/ose-assets.tar.gz && du -sh {REMOTE_DIR} && ls {REMOTE_DIR}'"
    )
    run("ssh", "-i", key, "-o", "BatchMode=yes", piper, remote)
    print("done; the server serves them at https://ose.jfly.dev/assets/index.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
