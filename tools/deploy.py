#!/usr/bin/env python
"""Deploy HEAD to ose.jfly.dev by hand, from this machine (docs/DEPLOY.md).

  python tools/deploy.py            build, package, push into the ose container on piper, restart

What .github/workflows/deploy.yml does on every push to main, for when a build has to go out from
here. Where piper is and which key opens it come from git config, so no host or key lives in the
repo:

  git config ose.piper root@<piper address>
  git config ose.piperKey <path to the private key>
  git config ose.ct 120                      # the ose container's VMID (default 120)

Needs rustup's x86_64-unknown-linux-musl target and trunk.
"""
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def run(*cmd, cwd=ROOT, env=None):
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True, env=env)


def config(key, default=""):
    r = subprocess.run(["git", "config", "--get", key], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip() or default


def main():
    piper, key, ct = config("ose.piper"), config("ose.piperKey"), config("ose.ct", "120")
    if not piper or not key:
        sys.exit("deploy: set `git config ose.piper root@host` and `git config ose.piperKey path` first")
    rel_id = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    env = dict(os.environ, CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="rust-lld")
    run("cargo", "build", "--release", "-p", "ose-server", "--target", "x86_64-unknown-linux-musl", env=env)
    run("trunk", "build", "--release", "--dist", str(ROOT / "target" / "web"), cwd=ROOT / "crates" / "ose-client")
    with tempfile.TemporaryDirectory() as tmp:
        pkg = Path(tmp) / "rel"
        (pkg / "web").mkdir(parents=True)
        shutil.copy(ROOT / "target" / "x86_64-unknown-linux-musl" / "release" / "ose-server", pkg)
        for f in (ROOT / "target" / "web").iterdir():
            if f.suffix in (".html", ".wasm", ".js"):
                shutil.copy(f, pkg / "web")
        for f in ("ose.service", "install.sh"):
            (pkg / f).write_bytes((ROOT / "deploy" / f).read_bytes().replace(b"\r\n", b"\n"))
        tar = Path(tmp) / "ose-release.tar.gz"
        def root_owner(info):
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            return info
        # Python writes a portable archive on both Windows BSD tar and GNU tar hosts.
        with tarfile.open(tar, "w:gz") as archive:
            archive.add(pkg, arcname=".", filter=root_owner)
        run("scp", "-q", "-i", key, str(tar), f"{piper}:/root/ose-release.tar.gz")
    remote = (
        f"pct push {ct} /root/ose-release.tar.gz /tmp/ose-release.tar.gz && rm /root/ose-release.tar.gz && "
        f"pct exec {ct} -- sh -c 'mkdir -p /tmp/ose-r && tar --no-same-owner -xzf /tmp/ose-release.tar.gz -C /tmp/ose-r ./install.sh "
        f"&& sh /tmp/ose-r/install.sh /tmp/ose-release.tar.gz {rel_id}'"
    )
    run("ssh", "-i", key, "-o", "BatchMode=yes", piper, remote)
    print(f"deployed {rel_id}: https://ose.jfly.dev/")


if __name__ == "__main__":
    sys.exit(main())
