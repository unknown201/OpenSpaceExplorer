"""Convex collision hulls for every part, from the part models (`assets/web/parts/<id>.glb`).

Vessel contacts test parts as spheres while they are apart and switch to each part's model up close
(`ose_sim::collide`). The model a contact uses is the convex hull of the part's meshes: for each of 162
directions spread over the sphere this keeps the mesh vertex farthest along it, which is a subset of the hull's
vertices, so the hull of the kept points is a close approximation of the hull of the whole mesh (within a few
percent of its size). Points are written in the part's own Unity frame (glTF mirrors X; it is mirrored back),
metres, to the millimetre, as `assets/own/collision-hulls.json`: `{part id: [[x, y, z], ...]}`.

The GLBs are exported from the user's install by tools/export_web_assets.py. They can be read from a folder
or from a running server's `/assets/parts/` (`--base-url`); each part's hull is checked against the part's
authored bounds from `assets/own/parts-catalog.json`, and parts whose hull disagrees are listed and left out
(a contact then uses the part's box).

    python tools/export_collision_hulls.py --parts-dir D:/Projects/OpenSpaceExplorer/assets/web/parts
    python tools/export_collision_hulls.py --base-url http://localhost:7790/assets/parts
"""
from __future__ import annotations

import argparse
import json
import math
import struct
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# meshes that are effects, not the part (re-entry glow, plumes, lights)
SKIP_NODE_WORDS = ("reentry", "plume", "flame", "exhaust", "fx_", "_fx", "glow", "light", "smoke", "shock")

COMPONENT = {5126: ("f", 4), 5120: ("b", 1), 5121: ("B", 1), 5122: ("h", 2), 5123: ("H", 2)}
NORMALISE = {5120: 127.0, 5121: 255.0, 5122: 32767.0, 5123: 65535.0}


def directions(subdivisions: int = 2) -> list[tuple[float, float, float]]:
    """Unit vectors on a subdivided icosahedron (162 for two subdivisions)."""
    t = (1 + 5**0.5) / 2
    verts = [(-1, t, 0), (1, t, 0), (-1, -t, 0), (1, -t, 0), (0, -1, t), (0, 1, t), (0, -1, -t), (0, 1, -t),
             (t, 0, -1), (t, 0, 1), (-t, 0, -1), (-t, 0, 1)]
    faces = [(0, 11, 5), (0, 5, 1), (0, 1, 7), (0, 7, 10), (0, 10, 11), (1, 5, 9), (5, 11, 4), (11, 10, 2),
             (10, 7, 6), (7, 1, 8), (3, 9, 4), (3, 4, 2), (3, 2, 6), (3, 6, 8), (3, 8, 9), (4, 9, 5), (2, 4, 11),
             (6, 2, 10), (8, 6, 7), (9, 8, 1)]
    verts = [norm(v) for v in verts]
    for _ in range(subdivisions):
        cache: dict[tuple[int, int], int] = {}

        def mid(a: int, b: int) -> int:
            key = (min(a, b), max(a, b))
            if key not in cache:
                va, vb = verts[a], verts[b]
                verts.append(norm(((va[0] + vb[0]) / 2, (va[1] + vb[1]) / 2, (va[2] + vb[2]) / 2)))
                cache[key] = len(verts) - 1
            return cache[key]

        new = []
        for a, b, c in faces:
            ab, bc, ca = mid(a, b), mid(b, c), mid(c, a)
            new += [(a, ab, ca), (b, bc, ab), (c, ca, bc), (ab, bc, ca)]
        faces = new
    return verts


def norm(v):
    n = math.sqrt(sum(x * x for x in v))
    return (v[0] / n, v[1] / n, v[2] / n)


def mat_mul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(4)) for j in range(4)] for i in range(4)]


def node_matrix(n: dict):
    if "matrix" in n:
        m = n["matrix"]  # column-major
        return [[m[c * 4 + r] for c in range(4)] for r in range(4)]
    tx, ty, tz = n.get("translation", [0, 0, 0])
    x, y, z, w = n.get("rotation", [0, 0, 0, 1])
    sx, sy, sz = n.get("scale", [1, 1, 1])
    r = [[1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
         [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
         [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]]
    return [[r[0][0] * sx, r[0][1] * sy, r[0][2] * sz, tx],
            [r[1][0] * sx, r[1][1] * sy, r[1][2] * sz, ty],
            [r[2][0] * sx, r[2][1] * sy, r[2][2] * sz, tz],
            [0, 0, 0, 1]]


def read_glb(data: bytes) -> tuple[dict, bytes]:
    magic, _, _ = struct.unpack_from("<III", data, 0)
    if magic != 0x46546C67:
        raise ValueError("not a GLB")
    off, doc, binary = 12, None, b""
    while off < len(data):
        length, kind = struct.unpack_from("<II", data, off)
        chunk = data[off + 8: off + 8 + length]
        if kind == 0x4E4F534A:
            doc = json.loads(chunk)
        elif kind == 0x004E4942:
            binary = chunk
        off += 8 + length
    return doc, binary


def positions(doc: dict, binary: bytes, accessor: int) -> list[tuple[float, float, float]]:
    acc = doc["accessors"][accessor]
    if "bufferView" not in acc:
        return []
    view = doc["bufferViews"][acc["bufferView"]]
    fmt, size = COMPONENT[acc["componentType"]]
    stride = view.get("byteStride", size * 3)
    base = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    scale = NORMALISE.get(acc["componentType"]) if acc.get("normalized") else None
    out = []
    for i in range(acc["count"]):
        v = struct.unpack_from("<3" + fmt, binary, base + i * stride)
        out.append(tuple(c / scale for c in v) if scale else v)
    return out


def model_points(data: bytes) -> list[tuple[float, float, float]]:
    """Every mesh vertex of the part's model, in the glTF root frame."""
    doc, binary = read_glb(data)
    nodes = doc.get("nodes", [])
    roots = doc["scenes"][doc.get("scene", 0)]["nodes"]
    pts: list[tuple[float, float, float]] = []
    eye = [[1 if i == j else 0 for j in range(4)] for i in range(4)]
    stack = [(i, eye) for i in roots]
    while stack:
        i, parent = stack.pop()
        n = nodes[i]
        name = (n.get("name") or "").lower()
        if any(w in name for w in SKIP_NODE_WORDS):
            continue
        m = mat_mul(parent, node_matrix(n))
        if "mesh" in n:
            for prim in doc["meshes"][n["mesh"]]["primitives"]:
                if prim.get("mode", 4) != 4 or "POSITION" not in prim["attributes"]:
                    continue
                for x, y, z in positions(doc, binary, prim["attributes"]["POSITION"]):
                    pts.append((m[0][0] * x + m[0][1] * y + m[0][2] * z + m[0][3],
                                m[1][0] * x + m[1][1] * y + m[1][2] * z + m[1][3],
                                m[2][0] * x + m[2][1] * y + m[2][2] * z + m[2][3]))
        stack += [(c, m) for c in n.get("children", [])]
    return pts


def hull_points(pts, dirs) -> list[list[float]]:
    """The support point of the cloud along each direction, in Unity's frame (X mirrored back), mm."""
    kept = set()
    for d in dirs:
        best = max(pts, key=lambda p: p[0] * d[0] + p[1] * d[1] + p[2] * d[2])
        kept.add((round(-best[0], 3), round(best[1], 3), round(best[2], 3)))
    return [list(p) for p in sorted(kept)]


def agrees(hull, bounds) -> bool:
    """The hull's box against the part's authored bounds: within 25% of the largest size, each way."""
    if not bounds:
        return True
    c, s = bounds["center_m"], bounds["size_m"]
    big = max(s["x"], s["y"], s["z"], 0.1)
    for k, axis in enumerate("xyz"):
        lo = min(p[k] for p in hull)
        hi = max(p[k] for p in hull)
        if abs(lo - (c[axis] - s[axis] / 2)) > 0.25 * big or abs(hi - (c[axis] + s[axis] / 2)) > 0.25 * big:
            return False
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--parts-dir", type=Path, help="folder of <part id>.glb")
    parser.add_argument("--base-url", help="URL of a server's /assets/parts folder")
    parser.add_argument("--catalog", type=Path, default=ROOT / "assets/own/parts-catalog.json")
    parser.add_argument("--out", type=Path, default=ROOT / "assets/own/collision-hulls.json")
    args = parser.parse_args()
    if not args.parts_dir and not args.base_url:
        parser.error("give --parts-dir or --base-url")
    catalog = json.loads(args.catalog.read_text(encoding="utf-8"))
    dirs = directions()
    hulls: dict[str, list[list[float]]] = {}
    missing, disagree = [], []
    for part in catalog:
        pid = part["id"]
        try:
            if args.parts_dir:
                data = (args.parts_dir / f"{pid}.glb").read_bytes()
            else:
                with urllib.request.urlopen(f"{args.base_url.rstrip('/')}/{pid}.glb", timeout=60) as r:
                    data = r.read()
            pts = model_points(data)
        except Exception as e:  # noqa: BLE001 - a part without a model keeps its box
            missing.append(f"{pid} ({e})")
            continue
        if len(pts) < 4:
            missing.append(f"{pid} (no mesh)")
            continue
        hull = hull_points(pts, dirs)
        if not agrees(hull, part.get("bounds")):
            disagree.append(pid)
            continue
        hulls[pid] = hull
    args.out.write_text(json.dumps(hulls, separators=(",", ":"), sort_keys=True) + "\n", encoding="utf-8")
    print(f"{len(hulls)} hulls -> {args.out}")
    if missing:
        print(f"{len(missing)} without a model: " + ", ".join(missing[:20]), file=sys.stderr)
    if disagree:
        print(f"{len(disagree)} disagree with their bounds (left out): " + ", ".join(disagree), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
