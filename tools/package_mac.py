#!/usr/bin/env python
"""Build a self-starting macOS package of OpenSpaceExplorer (docs/DEPLOY.md, "Running on a Mac").

  python tools/package_mac.py [--assets assets/web | --manifest listing.txt] [--out dist/ose-mac-<commit>.zip]

Cross-compiles ose-server as a universal (Apple Silicon + Intel) binary with cargo-zigbuild, builds the
web client with trunk, and zips them with deploy/mac/ (start.command, fetch-assets.sh, README.txt) and
an asset manifest. The KSP2 assets themselves are not packaged: fetch-assets.sh downloads the manifest's
files from ose.jfly.dev (or copies a local export) on the Mac's first start.

Needs: rustup targets aarch64-apple-darwin and x86_64-apple-darwin, `pip install ziglang cargo-zigbuild`,
trunk, and an asset export (default assets/web) to list in the manifest. Never commit or upload the zip
to GitHub; hand it over through the project's shared files.
"""
import argparse
import os
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path
from precompress import prepare

ROOT = Path(__file__).resolve().parent.parent
TARGET = "universal2-apple-darwin"


def run(*cmd, cwd=ROOT, env=None):
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True, env=env)


def manifest(assets: Path) -> str:
    """`<size>\t<path>` per exported asset, skipping the server-built package cache and gzip sidecars."""
    rows = []
    for p in sorted(assets.rglob("*")):
        rel = p.relative_to(assets).as_posix()
        if p.is_file() and not rel.startswith("packages/") and not rel.endswith(".gz"):
            rows.append(f"{p.stat().st_size}\t{rel}")
    if not rows:
        sys.exit(f"package_mac: no assets under {assets}")
    return "\n".join(rows) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--assets", type=Path, default=ROOT / "assets" / "web", help="exported assets to list")
    ap.add_argument("--manifest", type=Path, help="ready-made `<size>\\t<path>` listing to use instead of --assets")
    ap.add_argument("--out", type=Path, help="zip to write (default dist/ose-mac-<commit>.zip)")
    args = ap.parse_args()
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True,
                            check=True).stdout.strip()
    out = args.out or ROOT / "dist" / f"ose-mac-{commit}.zip"
    listing = args.manifest.read_text() if args.manifest else manifest(args.assets)

    # 11.0 is the first macOS on Apple Silicon; the zig default would be the newest SDK's version.
    env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET="11.0")
    run("cargo", "zigbuild", "--release", "-p", "ose-server", "--target", TARGET, env=env)
    web = ROOT / "target" / "web"
    run("trunk", "build", "--release", "--dist", str(web), cwd=ROOT / "crates" / "ose-client")
    prepare(web)

    name = f"ose-mac-{commit}"
    out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        def add(path: Path, arc: str, mode=0o644):
            info = zipfile.ZipInfo.from_file(path, f"{name}/{arc}")
            info.external_attr = (0o100000 | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            data = path.read_bytes()
            if path.suffix in (".command", ".sh", ".txt"):
                data = data.replace(b"\r\n", b"\n")  # a Windows checkout must not break the shell scripts
            z.writestr(info, data)
        add(ROOT / "target" / TARGET / "release" / "ose-server", "ose-server", 0o755)
        for f in sorted(web.rglob("*")):
            if f.is_file():
                add(f, f"web/{f.relative_to(web).as_posix()}")
        mac = ROOT / "deploy" / "mac"
        add(mac / "start.command", "start.command", 0o755)
        add(mac / "fetch-assets.sh", "fetch-assets.sh", 0o755)
        add(mac / "README.txt", "README.txt")
        z.writestr(f"{name}/assets-manifest.txt", listing)
        z.writestr(f"{name}/VERSION", f"{commit}\n")
    print(f"wrote {out} ({out.stat().st_size / 1e6:.1f} MB)")


if __name__ == "__main__":
    sys.exit(main())
