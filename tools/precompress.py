#!/usr/bin/env python
"""Prepare deterministic gzip sidecars for web client/assets without changing their decoded bytes.

python tools/precompress.py target/web
python tools/precompress.py assets/web ksc vab parts
PNG/JPEG/audio are already compressed. Existing terrain/LUT gzip copies are reused while fresh.
"""
import argparse
import gzip
import os
import shutil
import stat
import tempfile
from pathlib import Path

EXTENSIONS = {'.glb', '.wasm', '.js', '.json', '.html', '.wgsl', '.bin', '.f16', '.rgba', '.texture'}


def prepare(root: Path, members=None) -> tuple[int, int, int]:
    root = root.resolve()
    changed = raw_total = gzip_total = 0
    files = set()
    for member in members or ['.']:
        path = (root / member).resolve()
        if not path.is_relative_to(root):
            raise ValueError(f'asset selection escapes {root}: {member}')
        if not path.exists():
            raise FileNotFoundError(path)
        files.update(p for p in (path.rglob('*') if path.is_dir() else [path])
                     if p.is_file() and p.suffix.lower() in EXTENSIONS)
    for path in sorted(files):
        source_stat = path.stat()
        size = source_stat.st_size
        if size < 1024:
            continue
        zipped = path.with_name(path.name + '.gz')
        if zipped.exists() and zipped.stat().st_mtime_ns >= path.stat().st_mtime_ns:
            os.chmod(zipped, stat.S_IMODE(source_stat.st_mode))
            raw_total += size
            gzip_total += zipped.stat().st_size
            continue
        # Write beside the source, then atomically replace, so a running server never sees half a gzip.
        with tempfile.NamedTemporaryFile(dir=path.parent, suffix='.gz.tmp', delete=False) as handle:
            temporary = Path(handle.name)
            try:
                with gzip.GzipFile(filename='', mode='wb', fileobj=handle, compresslevel=6, mtime=0) as gz:
                    with path.open('rb') as source:
                        shutil.copyfileobj(source, gz)
                handle.flush()
                zipped_size = temporary.stat().st_size
            except BaseException:
                handle.close()
                temporary.unlink(missing_ok=True)
                raise
        if zipped_size < size * 0.95:
            os.chmod(temporary, stat.S_IMODE(source_stat.st_mode))
            os.replace(temporary, zipped)
            os.utime(zipped, ns=(source_stat.st_atime_ns, source_stat.st_mtime_ns))
            raw_total += size
            gzip_total += zipped_size
            changed += 1
        else:
            temporary.unlink()
            # Remove a stale sidecar rather than serving the preceding export's bytes.
            zipped.unlink(missing_ok=True)
    return changed, raw_total, gzip_total


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('members', nargs='*')
    args = parser.parse_args()
    count, raw, encoded = prepare(args.root, args.members)
    print(f'gzip: {count} files prepared; {raw / 1e6:.1f} MB -> {encoded / 1e6:.1f} MB')


if __name__ == '__main__':
    main()
