#!/usr/bin/env python
"""Shrink the part photos from e2e/export-part-icons.mjs into the VAB picker's icons:
assets/web/ui/parts/<part id>.png, 128 px. Push with `push_assets.py ui`.

    .venv/Scripts/python tools/shrink_part_icons.py <dir of 600 px photos>
"""
from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SIZE = 128


def main() -> int:
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    src = Path(sys.argv[1])
    dest = ROOT / "assets" / "web" / "ui" / "parts"
    dest.mkdir(parents=True, exist_ok=True)
    n = 0
    for f in sorted(src.glob("*.png")):
        img = Image.open(f).convert("RGB").resize((SIZE, SIZE), Image.LANCZOS)
        img.save(dest / f.name, optimize=True)
        n += 1
    print(f"part icons: {n} -> {dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
