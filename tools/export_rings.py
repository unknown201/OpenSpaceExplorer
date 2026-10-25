"""Planet rings from the original's scaled bundles (Dres: `innerRadius` 414 km, `outerRadius` 440 km, a 512 x 4 radial
texture): writes `ring.png` (the texture's first row, radial colour and alpha) and planet.json `ring` {inner_m, outer_m,
alpha_edge, texture} for each body that has one. The ring lies in the body's equatorial plane.

Usage: python tools/export_rings.py [--planets assets/web/planets]
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

DEFAULT_INSTALL = r"C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2"


def export(install: Path, planets: Path) -> None:
    import UnityPy

    bdir = install / "KSP2_x64_Data/StreamingAssets/aa/StandaloneWindows64"
    for body_dir in sorted(p for p in planets.iterdir() if p.is_dir()):
        bundle = bdir / f"celestialbody-scaled-{body_dir.name.lower()}_assets_all.bundle"
        if not bundle.exists():
            continue
        env = UnityPy.load(str(bundle))
        objs = {o.path_id: o for o in env.objects}
        ring = None
        for o in env.objects:
            if o.type.name != "MonoBehaviour":
                continue
            try:
                tt = o.read_typetree()
            except Exception:  # noqa: BLE001
                continue
            if "innerRadius" in tt and "ringTexture" in tt:
                ring = tt
                break
        if ring is None:
            continue
        tex = objs.get((ring.get("ringTexture") or {}).get("m_PathID"))
        if tex is None:
            continue
        img = tex.read().image.convert("RGBA")
        img.crop((0, 0, img.width, 1)).save(body_dir / "ring.png")
        info_path = body_dir / "planet.json"
        info = json.loads(info_path.read_text(encoding="utf-8"))
        info["ring"] = {
            "inner_m": ring["innerRadius"],
            "outer_m": ring["outerRadius"],
            "alpha_edge": ring.get("alphaEdge", 0.1),
            "texture": "ring.png",
            "source": tex.read().m_Name,
        }
        info_path.write_text(json.dumps(info, indent=1), encoding="utf-8")
        print(body_dir.name, info["ring"])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--planets", default="assets/web/planets")
    a = ap.parse_args()
    export(Path(a.install), Path(a.planets))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
