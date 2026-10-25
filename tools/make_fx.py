"""Write the small procedural effect meshes the client draws (no game assets involved).

    assets/web/fx/flame.glb   an engine flame: a cone 1 m wide at its base (y = 0), tapering to a point at
                              y = -1, open at the top, alpha blended; the client scales it per engine by
                              throttle and nozzle size and tints it
    assets/web/fx/canopy.glb  a parachute canopy: a dome 1 m across at its rim (y = 0) rising to y = 0.42, in
                              alternating orange and white gores, double sided; the client hangs it above each
                              open chute, scaled by the chute's size and how far it has opened
    assets/web/fx/kerbal.glb  a Kerbal on EVA, 1.25 m tall standing on y = 0: white suit, green head, dark visor,
                              backpack, facing +Z
    assets/web/fx/flag.glb    a planted flag: a 2.4 m pole on y = 0 and its cloth towards +X

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


def ellipsoid(c, r, segments: int = 16, rings: int = 10):
    """An ellipsoid centred at c with radii r = (rx, ry, rz) and outward normals."""
    pos, nrm, uv, tris = [], [], [], []
    for k in range(rings + 1):
        th = math.pi * k / rings
        for sg in range(segments + 1):
            ph = 2 * math.pi * sg / segments
            n = (math.sin(th) * math.cos(ph), math.cos(th), math.sin(th) * math.sin(ph))
            pos.append((c[0] + r[0] * n[0], c[1] + r[1] * n[1], c[2] + r[2] * n[2]))
            nrm.append(n)
            uv.append((sg / segments, k / rings))
    row = segments + 1
    for k in range(rings):
        for sg in range(segments):
            a = k * row + sg
            tris += [a, a + 1, a + row, a + 1, a + row + 1, a + row]
    return pos, nrm, uv, tris


def lathe(profile, segments: int = 32):
    """A surface of revolution about +Y from (radius, y) points, outward normals from the profile slope."""
    pos, nrm, uv, tris = [], [], [], []
    for k, (r, y) in enumerate(profile):
        a = profile[max(k - 1, 0)]
        b = profile[min(k + 1, len(profile) - 1)]
        dr, dy = b[0] - a[0], b[1] - a[1]
        ln = math.hypot(dr, dy) or 1.0
        nr, ny = dy / ln, -dr / ln
        for sg in range(segments + 1):
            ph = 2 * math.pi * sg / segments
            c, sn = math.cos(ph), math.sin(ph)
            pos.append((r * c, y, r * sn))
            nrm.append((nr * c, ny, nr * sn))
            uv.append((sg / segments, k / (len(profile) - 1)))
    row = segments + 1
    for k in range(len(profile) - 1):
        for sg in range(segments):
            a = k * row + sg
            tris += [a, a + row, a + 1, a + 1, a + row, a + row + 1]
    return pos, nrm, uv, tris


def box(lo, hi):
    """An axis-aligned box (one winding; `solid` adds the other side)."""
    pos, nrm, uv, tris = [], [], [], []
    faces = [((1, 0, 0), 0, hi[0]), ((-1, 0, 0), 0, lo[0]), ((0, 1, 0), 1, hi[1]), ((0, -1, 0), 1, lo[1]), ((0, 0, 1), 2, hi[2]), ((0, 0, -1), 2, lo[2])]
    for n, axis, v in faces:
        u_ax, w_ax = [a for a in range(3) if a != axis]
        base = len(pos)
        for du, dw in ((0, 0), (1, 0), (1, 1), (0, 1)):
            q = [0.0, 0.0, 0.0]
            q[axis] = v
            q[u_ax] = hi[u_ax] if du else lo[u_ax]
            q[w_ax] = hi[w_ax] if dw else lo[w_ax]
            pos.append(tuple(q))
            nrm.append(n)
            uv.append((du, dw))
        tris += [base, base + 1, base + 2, base, base + 2, base + 3]
    return pos, nrm, uv, tris


def solid(g, name, rgba, mesh):
    mat = g.material(name, {})
    g.materials[mat]["pbrMetallicRoughness"]["baseColorFactor"] = rgba
    # Both faces: the renderer culls back faces, and these hand-built meshes are not worth winding-proofing.
    pos, nrm, uv, tris = both_faces(mesh)
    return g.node(name, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), g.mesh(name, pos, nrm, uv, tris, mat), [])


def both_faces(mesh):
    pos, nrm, uv, tris = (list(x) for x in mesh)
    n0, t0 = len(pos), len(tris)
    pos += pos[:n0]
    nrm += [(-x, -y, -z) for x, y, z in nrm[:n0]]
    uv += uv[:n0]
    for i in range(0, t0, 3):
        tris += [tris[i] + n0, tris[i + 2] + n0, tris[i + 1] + n0]
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

    g = Glb()
    parts = [
        solid(g, "suit", [0.9, 0.9, 0.88, 1.0], ellipsoid((0.0, 0.48, 0.0), (0.26, 0.42, 0.22))),
        solid(g, "boots", [0.25, 0.25, 0.28, 1.0], box((-0.2, 0.0, -0.12), (0.2, 0.12, 0.16))),
        solid(g, "head", [0.45, 0.78, 0.25, 1.0], ellipsoid((0.0, 1.02, 0.02), (0.24, 0.22, 0.22))),
        solid(g, "visor", [0.08, 0.1, 0.16, 1.0], ellipsoid((0.0, 1.04, 0.14), (0.17, 0.12, 0.1))),
        solid(g, "pack", [0.55, 0.57, 0.6, 1.0], box((-0.2, 0.35, -0.36), (0.2, 0.85, -0.16))),
    ]
    g.roots.append(g.node("kerbal", (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), None, parts))
    print(f"fx/kerbal.glb: {g.write(out / 'kerbal.glb', {'source': 'tools/make_fx.py'})} bytes")

    g = Glb()
    parts = [
        solid(g, "pole", [0.7, 0.7, 0.72, 1.0], box((-0.03, 0.0, -0.03), (0.03, 2.4, 0.03))),
        solid(g, "cloth", [0.95, 0.45, 0.12, 1.0], box((0.03, 1.6, -0.005), (1.23, 2.35, 0.005))),
    ]
    g.roots.append(g.node("flag", (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), None, parts))
    print(f"fx/flag.glb: {g.write(out / 'flag.glb', {'source': 'tools/make_fx.py'})} bytes")

    # Payload fairing shells (the original's are procedural): a tube of radius 1 and height 1 stacked as
    # needed, and a rounded nose of radius 1 and height 1.6, both drawn at the fairing base's radius.
    white = [0.86, 0.86, 0.84, 1.0]
    g = Glb()
    g.roots.append(solid(g, "fairing_ring", white, lathe([(1.0, 0.0), (1.0, 1.0)])))
    print(f"fx/fairing_ring.glb: {g.write(out / 'fairing_ring.glb', {'source': 'tools/make_fx.py'})} bytes")
    g = Glb()
    h = 1.6
    nose = [((1.0 - (k / 12) ** 2) ** 0.5 * 1.0 if k < 12 else 0.0, h * k / 12) for k in range(13)]
    g.roots.append(solid(g, "fairing_nose", white, lathe(nose)))
    print(f"fx/fairing_nose.glb: {g.write(out / 'fairing_nose.glb', {'source': 'tools/make_fx.py'})} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
