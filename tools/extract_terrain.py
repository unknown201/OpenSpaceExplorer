"""Export each body's global terrain height map and surface sites from the KSP2 install.

Writes `assets/own/terrain/index.json` and `assets/own/terrain/terrain.bin` (all maps concatenated, raw
little-endian u16, row 0 = south pole, column 0 = longitude -180, equirectangular). The server embeds both.

Height above sea level at a texel = value / 65535 * scale_m + offset_m, where scale_m is the body's
PQSData `heightMapInfo.heightMapScale` and offset_m is minus its `oceanAltitude` (the original's
`GetSurfaceHeight` = radius - oceanAltitude + sample). Only the global map is exported; the original adds
tiled detail layers and decals on top (see docs/reference-notes/terrain.md).

Sites (launch pads, the KSC anchor) come from the body JSON `LocalSimObjectsData`, composed through their
`RelativeTo` parent and converted to latitude, longitude and altitude.

Usage: .venv/Scripts/python tools/extract_terrain.py [--install PATH] [--data assets/ksp2/data]
       [--out assets/own/terrain] [--width 512]
"""
from __future__ import annotations

import argparse
import array
import json
import math
import sys
from pathlib import Path

DEFAULT_INSTALL = r"C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2"


def qrot(q: dict, v: tuple[float, float, float]) -> tuple[float, float, float]:
    x, y, z, w = q["x"], q["y"], q["z"], q["w"]
    vx, vy, vz = v
    tx, ty, tz = 2 * (y * vz - z * vy), 2 * (z * vx - x * vz), 2 * (x * vy - y * vx)
    return (vx + w * tx + (y * tz - z * ty), vy + w * ty + (z * tx - x * tz), vz + w * tz + (x * ty - y * tx))


def sites_for(body: dict) -> list[dict]:
    objs = {s["Name"]: s for s in body.get("LocalSimObjectsData") or []}
    radius = body["radius"]
    out = []

    def world(name: str, depth: int = 0) -> tuple[float, float, float] | None:
        s = objs.get(name)
        if s is None or depth > 8:
            return None
        lp = (s["LocalPosition"]["x"], s["LocalPosition"]["y"], s["LocalPosition"]["z"])
        parent = s.get("RelativeTo") or ""
        if not parent:
            return lp
        pp = world(parent, depth + 1)
        if pp is None:
            return None
        off = qrot(objs[parent]["LocalRotation"], lp)
        return (pp[0] + off[0], pp[1] + off[1], pp[2] + off[2])

    for name in objs:
        p = world(name)
        if p is None:
            continue
        r = math.sqrt(sum(c * c for c in p))
        if r < radius * 0.5:
            continue  # origin-anchored helpers such as CommNet origins
        # Unity Y-up -> Z-up: (x, z, y)
        lat = math.degrees(math.asin(p[1] / r))
        lon = math.degrees(math.atan2(p[2], p[0]))
        out.append({"name": name, "lat_deg": lat, "lon_deg": lon, "altitude_m": r - radius})
    return out


def find_pqs(env) -> dict | None:
    for o in env.objects:
        if o.type.name != "MonoBehaviour":
            continue
        try:
            tt = o.read_typetree()
        except Exception:
            continue
        if "heightMapInfo" in tt:
            return tt
    return None


def downsample(raw: bytes, w: int, h: int, W: int, H: int) -> array.array:
    src = array.array("H")
    src.frombytes(raw[: w * h * 2])
    if sys.byteorder != "little":
        src.byteswap()
    out = array.array("H", [0]) * (W * H)
    fx, fy = w / W, h / H
    for j in range(H):
        y0, y1 = int(j * fy), max(int((j + 1) * fy), int(j * fy) + 1)
        for i in range(W):
            x0, x1 = int(i * fx), max(int((i + 1) * fx), int(i * fx) + 1)
            # box average, stepping so a 4096 map costs about 16 reads per output texel
            sx = max(1, (x1 - x0) // 4)
            sy = max(1, (y1 - y0) // 4)
            acc = n = 0
            for y in range(y0, y1, sy):
                row = y * w
                for x in range(x0, x1, sx):
                    acc += src[row + x]
                    n += 1
            out[j * W + i] = acc // n
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--data", default="assets/ksp2/data")
    ap.add_argument("--out", default="assets/own/terrain")
    ap.add_argument("--width", type=int, default=512)
    a = ap.parse_args()
    import UnityPy

    bundles = Path(a.install) / "KSP2_x64_Data/StreamingAssets/aa/StandaloneWindows64"
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    bin_path = out / "terrain.bin"
    index = {"version": 1, "bodies": []}
    offset = 0
    with open(bin_path, "wb") as binf:
        for jf in sorted((Path(a.data) / "celestialbody_json").glob("*.json")):
            body = json.load(open(jf, encoding="utf-8"))["data"]
            bid = body["bodyName"]
            if bid.startswith("Asteroid"):
                continue
            entry = {
                "id": bid,
                "has_ocean": bool(body.get("hasOcean")),
                "sites": sites_for(body),
                "map": None,
            }
            # the PQSData asset lives in the shared bundle for some bodies, the simulation bundle for others
            env = pqs = None
            for kind in ("shared", "simulation"):
                candidate = bundles / f"celestialbody-{kind}-{bid.lower()}_assets_all.bundle"
                if not candidate.exists():
                    continue
                env = UnityPy.load(str(candidate))
                pqs = find_pqs(env)
                if pqs is not None:
                    break
            if True:
                if pqs is not None:
                    hmi = pqs["heightMapInfo"]
                    pid = hmi["globalHeightMap"]["m_PathID"]
                    tex = next((o.read() for o in env.objects if o.path_id == pid), None)
                    if tex is not None and tex.m_TextureFormat == 9:  # R16
                        w, h = tex.m_Width, tex.m_Height
                        W, H = a.width, a.width // 2
                        data = downsample(tex.image_data, w, h, W, H)
                        blob = data.tobytes()
                        binf.write(blob)
                        entry["map"] = {
                            "offset": offset,
                            "width": W,
                            "height": H,
                            "scale_m": float(hmi["heightMapScale"]),
                            "offset_m": -float(body.get("oceanAltitude") or 0.0),
                            "source": f"{tex.m_Name} {w}x{h}",
                        }
                        offset += len(blob)
            index["bodies"].append(entry)
            m = entry["map"]
            print(f"{bid:8s} sites={len(entry['sites'])} map={m['source'] + ' scale ' + str(m['scale_m']) if m else '-'}")
    (out / "index.json").write_text(json.dumps(index, indent=1), encoding="utf-8")
    print(f"wrote {bin_path} ({offset} bytes) and index.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
