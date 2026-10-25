"""Write the small procedural effect meshes the client draws (no game assets involved).

    assets/web/fx/flame.glb   an engine flame: a cone 1 m wide at its base (y = 0), tapering to a point at
                              y = -1, open at the top, alpha blended; the client scales it per engine by
                              throttle and nozzle size and tints it
    assets/web/fx/canopy.glb  a parachute canopy: a dome 1 m across at its rim (y = 0) rising to y = 0.42, in
                              alternating orange and white gores, double sided; the client hangs it above each
                              open chute, scaled by the chute's size and how far it has opened

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


def canopy(gores: int = 12, rings: int = 8):
    """Two meshes of alternating gores (orange, white) of a dome, rim radius 0.5 at y = 0."""
    meshes = ([], [], [], []), ([], [], [], [])
    for g in range(gores):
        pos, nrm, uv, tris = meshes[g % 2]
        base = len(pos)
        steps = 4
        for k in range(rings + 1):
            phi = (k / rings) * (math.pi / 2) * 0.92  # 0 at the apex, short of the equator at the rim
            r = 0.5 * math.sin(phi) / math.sin(math.pi / 2 * 0.92)
            y = 0.42 * math.cos(phi) / 1.0 - 0.42 * math.cos(math.pi / 2 * 0.92)
            for s_ in range(steps + 1):
                a = 2 * math.pi * (g + s_ / steps) / gores
                x, z = r * math.cos(a), r * math.sin(a)
                pos.append((x, y, z))
                n = (math.sin(phi) * math.cos(a), math.cos(phi), math.sin(phi) * math.sin(a))
                nrm.append(n)
                uv.append((s_ / steps, k / rings))
        row = steps + 1
        for k in range(rings):
            for s_ in range(steps):
                a = base + k * row + s_
                b = a + 1
                c = a + row
                d = c + 1
                tris += [a, c, b, b, c, d]
    # The renderer culls back faces, so each gore also gets its inside: a copy facing the other way.
    for pos, nrm, uv, tris in meshes:
        n0, t0 = len(pos), len(tris)
        pos += list(pos)
        nrm += [(-x, -y, -z) for x, y, z in nrm[:n0]]
        uv += list(uv)
        for i in range(0, t0, 3):
            tris += [tris[i] + n0, tris[i + 2] + n0, tris[i + 1] + n0]
    return meshes


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

    g = Glb()
    nodes = []
    for (pos, nrm, uv, tris), (name, rgba) in zip(canopy(), [("canopy_orange", [0.95, 0.45, 0.12, 1.0]), ("canopy_white", [0.92, 0.92, 0.9, 1.0])]):
        mat = g.material(name, {})
        g.materials[mat]["doubleSided"] = True
        g.materials[mat]["pbrMetallicRoughness"]["baseColorFactor"] = rgba
        m = g.mesh(name, pos, nrm, uv, tris, mat)
        nodes.append(g.node(name, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), m, []))
    g.roots.append(g.node("canopy", (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), None, nodes))
    size = g.write(out / "canopy.glb", {"source": "tools/make_fx.py"})
    print(f"fx/canopy.glb: {size} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
