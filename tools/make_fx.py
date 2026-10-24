"""Write the small procedural effect meshes the client draws (no game assets involved).

    assets/web/fx/flame.glb   an engine flame: a cone 1 m wide at its base (y = 0), tapering to a point at
                              y = -1, open at the top, alpha blended; the client scales it per engine by
                              throttle and nozzle size and tints it

    .venv/Scripts/python tools/make_fx.py
"""
from __future__ import annotations

import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_web_assets import ROOT, Glb  # noqa: E402


def cone(segments: int = 24, rings: int = 6):
    """A cone with rings so the client's per-vertex lighting has something to work with; counter-clockwise
    seen from outside (glTF front faces)."""
    pos, nrm, uv, tris = [], [], [], []
    for k in range(rings + 1):
        t = k / rings  # 0 at the nozzle, 1 at the tip
        y = -t
        r = 0.5 * (1.0 - t) ** 0.8
        for s in range(segments + 1):
            a = 2 * math.pi * s / segments
            x, z = r * math.cos(a), r * math.sin(a)
            pos.append((x, y, z))
            n = (math.cos(a), 0.5, math.sin(a))
            m = math.sqrt(sum(c * c for c in n))
            nrm.append(tuple(c / m for c in n))
            uv.append((s / segments, t))
    row = segments + 1
    for k in range(rings):
        for s in range(segments):
            a = k * row + s
            b = a + 1
            c = a + row
            d = c + 1
            tris += [a, b, c, b, d, c]
    return pos, nrm, uv, tris


def main() -> int:
    out = ROOT / "assets" / "web" / "fx"
    out.mkdir(parents=True, exist_ok=True)
    pos, nrm, uv, tris = cone()
    g = Glb()
    mat = g.material("flame", {})
    g.materials[mat]["alphaMode"] = "BLEND"
    g.materials[mat]["doubleSided"] = True
    g.materials[mat]["pbrMetallicRoughness"]["baseColorFactor"] = [1.0, 0.75, 0.35, 0.8]
    m = g.mesh("flame", pos, nrm, uv, tris, mat)
    g.roots.append(g.node("flame", (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), m, []))
    size = g.write(out / "flame.glb", {"source": "tools/make_fx.py"})
    print(f"fx/flame.glb: {len(pos)} vertices, {size} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
