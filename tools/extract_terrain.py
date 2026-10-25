"""Export each body's global terrain height map and surface sites from the KSP2 install.

Writes `assets/own/terrain/index.json` and `assets/own/terrain/terrain.bin` (all maps concatenated, raw
little-endian u16, row 0 = south pole, column 0 = longitude -180, equirectangular). The server embeds both.

Height above sea level at a texel = value / 65535 * scale_m + offset_m, where scale_m is the body's
PQSData `heightMapInfo.heightMapScale` and offset_m is minus its `oceanAltitude` (the original's
`GetSurfaceHeight` = radius - oceanAltitude + sample). Only the global map is exported; the original adds
tiled detail layers and decals on top (see docs/reference-notes/terrain.md).

Sites (launch pads, the KSC anchor) come from the body JSON `LocalSimObjectsData`, composed through their
`RelativeTo` parent and converted to latitude, longitude and altitude.

With `--packs DIR` (for example `assets/web/terrain`, served by the server's `--assets-dir` and never
committed) it also writes one pack per body, `<Body>.json` + `<Body>.bin`, with what the rest of the original's
`PQSJobUtil.HeightSample` needs: the global map at 4096 x 2048 (Kerbin, Mun) or 2048 x 1024, the biome mask
(RGBA8, 1024 x 512), the eight tiled large/medium layers (at most 1024 square) with their `uvScale` and
`heightScale`, and the PQS height decals (slices at 512 square, alpha at 256) with every placed instance
resolved through `PQSDecalInstance.GetInstanceInfo`. `ose_data::terrain::TerrainMap::from_pack` reads them.

Usage: .venv/Scripts/python tools/extract_terrain.py [--install PATH] [--data assets/ksp2/data]
       [--out assets/own/terrain] [--width 512] [--packs assets/web/terrain]
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


# Launch pads 2 to 4 where the drawn KSC campus has them (the campus is fitted to pads 2-4 at a 0.041 deg yaw,
# server and client): the install's spawn objects are offset from the campus layout, and their terrain flats
# (ose-data flats_for) left small plateaus beside it. Pad 1 stays where the install puts it (the campus anchor).
KSC_PAD_SITES = {
    "kerbin_KSCLaunchpad_spawn_02": (-0.49889, -74.97621),
    "kerbin_KSCLaunchpad_spawn_03": (-0.68405, -75.13454),
    "kerbin_KSCLaunchpad_spawn_04": (-0.49911, -75.29724),
}


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
        lat, lon = KSC_PAD_SITES.get(name, (lat, lon))
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


# ---------------------------------------------------------------------------------------------------------------
# terrain packs

PACK_WIDE = {"Kerbin": 4096, "Mun": 4096}


def box_u16(a, w: int, h: int):
    """Box-downsample a (H, W) u16 array to (h, w) (integer factors)."""
    import numpy as np

    H, W = a.shape
    fy, fx = max(1, H // h), max(1, W // w)
    a = a[: h * fy, : w * fx].astype(np.float64)
    return (a.reshape(h, fy, w, fx).mean(axis=(1, 3)) + 0.5).astype(np.uint16)


def r16(tex):
    """A Texture2D's raw R16 data as (H, W), row 0 at v = 0 (the bottom, the south pole)."""
    import numpy as np

    return np.frombuffer(bytes(tex.image_data[: tex.m_Width * tex.m_Height * 2]), dtype="<u2").reshape(
        tex.m_Height, tex.m_Width
    )


def rgba8(tex):
    """A Texture2D as (H, W, 4) u8, row 0 at v = 0."""
    import numpy as np

    if tex.m_TextureFormat == 4:  # RGBA32
        return np.frombuffer(bytes(tex.image_data[: tex.m_Width * tex.m_Height * 4]), dtype=np.uint8).reshape(
            tex.m_Height, tex.m_Width, 4
        )
    return np.asarray(tex.image.convert("RGBA"))[::-1]


def write_pack(bid: str, body: dict, env_objs: dict, pqs: dict, radius: float, out: Path) -> dict | None:
    """Write `<bid>.json` and `<bid>.bin` (see the module docstring); `env_objs` maps path id to object."""
    import numpy as np

    def get(pptr):
        pid = (pptr or {}).get("m_PathID")
        o = env_objs.get(pid) if pid else None
        return o.read() if o is not None else None

    hmi = pqs["heightMapInfo"]
    g = get(hmi["globalHeightMap"])
    if g is None or g.m_TextureFormat != 9:
        return None
    blob = bytearray()

    def put(arr) -> int:
        off = len(blob)
        if arr.dtype == np.uint8:
            blob.extend(np.ascontiguousarray(arr).tobytes())
        else:
            blob.extend(np.ascontiguousarray(arr).astype("<u2").tobytes())
        return off

    W = PACK_WIDE.get(bid, 2048)
    gm = box_u16(r16(g), W, W // 2)
    pack = {
        "version": 1,
        "id": bid,
        "radius_m": radius,
        "has_ocean": bool(body.get("hasOcean")),
        "sites": sites_for(body),
        "global": {"offset": put(gm), "width": W, "height": W // 2, "scale_m": float(hmi["heightMapScale"]),
                   "offset_m": -float(body.get("oceanAltitude") or 0.0),
                   "source": f"{g.m_Name} {g.m_Width}x{g.m_Height}"},
        "mask": None,
        "contrast": float(hmi.get("heightMapContrast", 5.0)),
        "large": [],
        "medium": [],
        "decal_heights": None,
        "decal_alpha": None,
        "decals": [],
    }
    mask = get(hmi.get("mask"))
    if mask is not None:
        m = rgba8(mask)
        mh, mw = min(512, m.shape[0]), min(1024, m.shape[1])
        fy, fx = m.shape[0] // mh, m.shape[1] // mw
        m = m[: mh * fy, : mw * fx].reshape(mh, fy, mw, fx, 4).mean(axis=(1, 3)).round().astype(np.uint8)
        pack["mask"] = {"offset": put(m), "width": mw, "height": mh, "count": 1}
    for size in ("large", "medium"):
        for ch in "RGBA":
            reg = hmi.get(f"{size}{ch}") or {}
            t = get(reg.get("heightMap"))
            if t is None or t.m_TextureFormat != 9 or not reg.get("heightScale"):
                pack[size].append(None)
                continue
            a = r16(t)
            w = min(1024, a.shape[1])
            h = min(1024, a.shape[0])
            a = box_u16(a, w, h)
            pack[size].append({"grid": {"offset": put(a), "width": w, "height": h, "count": 1},
                               "uv_scale": float(reg["uvScale"]), "height_scale": float(reg["heightScale"])})
    # decals: the baked height/alpha stacks and every placed instance
    data = None
    decal_defs = {}
    instances = []
    for pid, o in env_objs.items():
        if o.type.name != "MonoBehaviour":
            continue
        try:
            tt = o.read_typetree()
        except Exception:
            continue
        if "HeightData" in tt and "BakedPqsDecalList" in tt:
            data = tt
        elif "DecalID" in tt and "HeightScale" in tt and "LatLong" not in tt:
            decal_defs[pid] = tt
        elif "LatLong" in tt and "PQSDecal" in tt and tt.get("m_Enabled", 1):
            instances.append(tt)
    if data is not None and data["HeightWidth"] and instances:
        baked = [p["m_PathID"] for p in data["BakedPqsDecalList"]]
        n = len(baked)
        hw, hh = data["HeightWidth"], data["HeightHeight"]
        hd = np.asarray(data["HeightData"], dtype=np.uint16).reshape(-1, hh, hw)[:n]
        dw = min(512, hw)
        dh = dw * hh // hw
        hd = np.stack([box_u16(x, dw, dh) for x in hd])
        pack["decal_heights"] = {"offset": put(hd), "width": dw, "height": dh, "count": n}
        if data.get("AlphaData") and data["AlphaWidth"]:
            aw, ah = data["AlphaWidth"], data["AlphaHeight"]
            ad = np.asarray(data["AlphaData"], dtype=np.uint16).reshape(-1, ah, aw)[:n]
            w2 = min(256, aw)
            h2 = w2 * ah // aw
            ad = np.stack([box_u16(x, w2, h2) for x in ad])
            pack["decal_alpha"] = {"offset": put(ad), "width": w2, "height": h2, "count": n}
        for inst in instances:
            dpid = inst["PQSDecal"]["m_PathID"]
            if dpid not in baked:
                continue
            d = decal_defs.get(dpid)

            def pick(key, inst=inst, d=d):
                if inst.get(f"Override{key}") or d is None:
                    return inst[key]
                return d.get(key, inst[key])

            pack["decals"].append({
                "lat_deg": inst["LatLong"]["x"], "lon_deg": inst["LatLong"]["y"],
                "rotation_deg": inst["Rotation"], "scale_m": inst["Scale"], "index": baked.index(dpid),
                "height_scale": pick("HeightScale"),
                "height_offset": pick("HeightOffset"),
                "blend": int(pick("FadeShape")) * int(pick("HeightBlendMode")),
                "fade_strength": pick("FadeStrength"),
                "vertical_offset": inst["VerticalOffset"],
                "use_alpha": bool(pick("UseAlphaMask")),
                "sort_order": int(pick("SortOrder")),
            })
    out.mkdir(parents=True, exist_ok=True)
    (out / f"{bid}.bin").write_bytes(bytes(blob))
    # a gzip copy the server hands to browsers (about a quarter smaller); the server itself reads the .bin
    import gzip

    (out / f"{bid}.bin.gz").write_bytes(gzip.compress(bytes(blob), compresslevel=6))
    (out / f"{bid}.json").write_text(json.dumps(pack, indent=1), encoding="utf-8")
    return {"id": bid, "bytes": len(blob), "decals": len(pack["decals"]),
            "layers": sum(1 for x in pack["large"] + pack["medium"] if x)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--data", default="assets/ksp2/data")
    ap.add_argument("--out", default="assets/own/terrain")
    ap.add_argument("--width", type=int, default=512)
    ap.add_argument("--packs", default=None, help="also write per-body terrain packs into this directory")
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
    if a.packs:
        packs = Path(a.packs)
        written = []
        for jf in sorted((Path(a.data) / "celestialbody_json").glob("*.json")):
            body = json.load(open(jf, encoding="utf-8"))["data"]
            bid = body["bodyName"]
            if bid.startswith("Asteroid"):
                continue
            paths = [bundles / f"celestialbody-{k}-{bid.lower()}_assets_all.bundle" for k in ("shared", "simulation")]
            paths = [str(p) for p in paths if p.exists()]
            if not paths:
                continue
            env = UnityPy.load(*paths)
            objs = {o.path_id: o for o in env.objects}
            pqs = find_pqs(env)
            if pqs is None:
                continue
            info = write_pack(bid, body, objs, pqs, float(body["radius"]), packs)
            if info:
                written.append(info)
                print(f"pack {bid:8s} {info['bytes'] / 1e6:6.1f} MB layers={info['layers']} decals={info['decals']}")
        (packs / "index.json").write_text(json.dumps({"version": 1, "bodies": written}, indent=1), encoding="utf-8")
    print(f"wrote {bin_path} ({offset} bytes) and index.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
