"""The KSC campus's own ground as a height overlay on Kerbin's terrain.

The campus model's ground mesh (`ksc_campus_VertexPaint_02`, about 21 x 19 km, and `ksc_campus_final`) is a
plateau at the campus height whose skirts slope down to several hundred metres below sea level at its edges.
The original draws it and the PQS terrain together: whichever is higher shows, and the mesh is the ground
where it is on top (the PQS has no flattening there; a replace decal holds the terrain near sea level).

This rasterises those meshes, placed as the server places the campus (`ose_sim::campus::Site`: scene point
`ANCHOR` at `SITE` lat/lon/alt, the scene's +X turned `YAW_DEG` from east toward north), into a lat/lon grid of
the highest mesh altitude per cell (metres above sea level; empty cells hold -32768). `ose_data::terrain`
embeds it and takes the higher of terrain and overlay, so the server stands craft on what the client draws.

Writes assets/own/terrain/ksc_ground.bin (int16 decimetres, row 0 north) and ksc_ground.json (bounds, size).

Usage: python tools/export_campus_ground.py [--glb assets/web/ksc/ksc.glb] [--out assets/own/terrain]
"""

from __future__ import annotations

import argparse
import json
import math
import struct
from pathlib import Path

import numpy as np

RADIUS = 600_000.0
# as crates/ose-server/src/main.rs: KSC_ANCHOR, KSC_YAW_DEG and the first launch pad spawn (the anchor)
SITE = (-0.6836, -74.8198, 115.2)
YAW_DEG = 0.0410
ANCHOR = (10.088_055, -117.589_006, -2_104.040_008)
MESHES = ("ksc_campus_VertexPaint_02", "ksc_campus_final")
GRID = 256
EMPTY = -32768


def quat_mat(q):
    x, y, z, w = q
    return np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])


def read_glb(path: Path):
    b = path.read_bytes()
    n = struct.unpack("<I", b[12:16])[0]
    j = json.loads(b[20:20 + n])
    return j, b[20 + n + 8:]


def accessor(j, blob, i, comps, dtype):
    a = j["accessors"][i]
    bv = j["bufferViews"][a["bufferView"]]
    off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
    item = np.dtype(dtype).itemsize * comps
    stride = bv.get("byteStride", item)
    raw = np.frombuffer(blob, dtype=np.uint8, count=stride * a["count"], offset=off).reshape(a["count"], stride)
    return raw[:, :item].copy().view(dtype).reshape(a["count"], comps)


def campus_triangles(glb: Path):
    """World (scene) triangles of the campus ground meshes, (n, 3, 3)."""
    j, blob = read_glb(glb)
    parent = {c: i for i, nd in enumerate(j["nodes"]) for c in nd.get("children", [])}

    def local(nd):
        m = np.eye(4)
        m[:3, :3] = quat_mat(nd.get("rotation", [0, 0, 0, 1])) * np.array(nd.get("scale", [1, 1, 1]))
        m[:3, 3] = nd.get("translation", [0, 0, 0])
        return m

    def world(i):
        m = local(j["nodes"][i])
        while i in parent:
            i = parent[i]
            m = local(j["nodes"][i]) @ m
        return m

    tris = []
    for i, nd in enumerate(j["nodes"]):
        if nd.get("mesh") is None or j["meshes"][nd["mesh"]].get("name") not in MESHES:
            continue
        w = world(i)
        for pr in j["meshes"][nd["mesh"]]["primitives"]:
            p = accessor(j, blob, pr["attributes"]["POSITION"], 3, np.float32).astype(np.float64)
            p = (w[:3, :3] @ p.T).T + w[:3, 3]
            ia = j["accessors"][pr["indices"]]
            dt = {5121: np.uint8, 5123: np.uint16, 5125: np.uint32}[ia["componentType"]]
            idx = accessor(j, blob, pr["indices"], 1, dt).reshape(-1, 3)
            tris.append(p[idx])
    return np.concatenate(tris)


def place(p):
    """Scene points (n, 3) -> latitude, longitude (degrees), altitude (m), as `campus::Site::place`."""
    la, lo = math.radians(SITE[0]), math.radians(SITE[1])
    up = np.array([math.cos(la) * math.cos(lo), math.cos(la) * math.sin(lo), math.sin(la)])
    east = np.array([-math.sin(lo), math.cos(lo), 0.0])
    north = np.array([-math.sin(la) * math.cos(lo), -math.sin(la) * math.sin(lo), math.cos(la)])
    s, c = math.sin(math.radians(YAW_DEG)), math.cos(math.radians(YAW_DEG))
    ex = east * c + north * s
    en = -east * s + north * c
    o = up * (RADIUS + SITE[2])
    d = p - np.array(ANCHOR)
    w = o + np.outer(d[:, 0], ex) + np.outer(d[:, 1], up) - np.outer(d[:, 2], en)
    r = np.linalg.norm(w, axis=1)
    lat = np.degrees(np.arcsin(np.clip(w[:, 2] / r, -1, 1)))
    lon = np.degrees(np.arctan2(w[:, 1], w[:, 0]))
    return lat, lon, r - RADIUS


def rasterise(tris):
    flat = tris.reshape(-1, 3)
    lat, lon, alt = place(flat)
    lat0, lat1, lon0, lon1 = lat.min(), lat.max(), lon.min(), lon.max()
    grid = np.full((GRID, GRID), -np.inf)
    # cell centres; row 0 north
    gy = (lat1 - (np.arange(GRID) + 0.5) / GRID * (lat1 - lat0))
    gx = (lon0 + (np.arange(GRID) + 0.5) / GRID * (lon1 - lon0))
    lat, lon, alt = (a.reshape(-1, 3) for a in (lat, lon, alt))
    for t in range(len(lat)):
        y, x, h = lat[t], lon[t], alt[t]
        c0 = max(int(np.searchsorted(gx, x.min())), 0)
        c1 = min(int(np.searchsorted(gx, x.max())), GRID)
        r0 = max(int(np.searchsorted(-gy, -y.max())), 0)
        r1 = min(int(np.searchsorted(-gy, -y.min())), GRID)
        if c0 >= c1 or r0 >= r1:
            continue
        px, py = np.meshgrid(gx[c0:c1], gy[r0:r1])
        den = (y[1] - y[2]) * (x[0] - x[2]) + (x[2] - x[1]) * (y[0] - y[2])
        if abs(den) < 1e-18:
            continue
        a = ((y[1] - y[2]) * (px - x[2]) + (x[2] - x[1]) * (py - y[2])) / den
        b = ((y[2] - y[0]) * (px - x[2]) + (x[0] - x[2]) * (py - y[2])) / den
        c = 1 - a - b
        inside = (a >= -1e-6) & (b >= -1e-6) & (c >= -1e-6)
        z = a * h[0] + b * h[1] + c * h[2]
        cell = grid[r0:r1, c0:c1]
        np.maximum(cell, np.where(inside, z, -np.inf), out=cell)
    return grid, (float(lat0), float(lat1), float(lon0), float(lon1))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--glb", default="assets/web/ksc/ksc.glb")
    ap.add_argument("--out", default="assets/own/terrain")
    a = ap.parse_args()
    grid, (lat0, lat1, lon0, lon1) = rasterise(campus_triangles(Path(a.glb)))
    dm = np.where(np.isfinite(grid), np.clip(np.round(grid * 10), -32767, 32767), EMPTY).astype("<i2")
    out = Path(a.out)
    (out / "ksc_ground.bin").write_bytes(dm.tobytes())
    meta = {
        "body": "Kerbin",
        "lat_deg": [lat0, lat1],
        "lon_deg": [lon0, lon1],
        "width": GRID,
        "height": GRID,
        "units": "int16 decimetres above sea level, row 0 north, -32768 empty",
        "site": {"lat_deg": SITE[0], "lon_deg": SITE[1], "alt_m": SITE[2], "yaw_deg": YAW_DEG},
    }
    (out / "ksc_ground.json").write_text(json.dumps(meta, indent=1), encoding="utf-8")
    filled = np.isfinite(grid)
    print(f"{filled.mean() * 100:.0f}% filled, {grid[filled].min():.0f} to {grid[filled].max():.0f} m, "
          f"lat {lat0:.4f}..{lat1:.4f}, lon {lon0:.4f}..{lon1:.4f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
