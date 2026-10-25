"""Polar caps for the planets' equirectangular maps (albedo.jpg, clouds.png), so the poles do not pinch.

An equirectangular map squeezes a whole row of texels into a point at each pole; sampled there on the GPU it
shows a starburst. This writes `poles.png` next to them (albedo RGB, cloud coverage A): two azimuthal equidistant
caps side by side (north left, south right), each covering `CAP_DEG` degrees from its pole, resampled from the
full map with each row first box-filtered along longitude to the width a cap texel covers there. planet.json
gets `poles: {cap_deg, albedo, clouds}`; the client (planet.wgsl `pole_uv`) blends the caps in near the poles.

Cap texel (x, y) in -1..1 maps to colatitude r * CAP_DEG and the same azimuth as the client's `equi_uv`
(atan2(-z, x)), i.e. x along +x and y along -z in the model frame.

Usage: python tools/export_poles.py [--planets assets/web/planets] [--bodies Kerbin,Mun]
"""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

CAP_DEG = 30.0
CAP_PX = 1024


def prefilter_rows(img, cap_px: int):
    """Box-filter each row near the poles along longitude (wrapping) to the angular size of one cap texel."""
    import numpy as np

    h, w = img.shape[:2]
    out = img.copy()
    texel_rad = math.radians(CAP_DEG) / (cap_px / 2)
    for row in range(h):
        lat = 90.0 - (row + 0.5) / h * 180.0
        colat = math.radians(90.0 - abs(lat))
        if math.degrees(colat) > CAP_DEG + 2.0:
            continue
        # longitude span of one cap texel at this colatitude, in equirect texels
        span = texel_rad / max(math.sin(colat), 1e-4) / (2 * math.pi) * w
        k = int(min(max(round(span), 1), w))
        if k <= 1:
            continue
        r = img[row].astype(np.float64)
        c = np.concatenate([r, r[:k]], axis=0).cumsum(axis=0)
        c = np.concatenate([np.zeros((1,) + r.shape[1:]), c], axis=0)
        box = (c[k:k + w] - c[0:w]) / k
        # centre the box
        out[row] = np.roll(box, k // 2, axis=0).astype(img.dtype)
    return out


def bilinear(img, x, y):
    """Sample img at float pixel coordinates (x wraps, y clamps)."""
    import numpy as np

    h, w = img.shape[:2]
    x0 = np.floor(x - 0.5).astype(np.int64)
    y0 = np.floor(y - 0.5).astype(np.int64)
    fx = (x - 0.5) - x0
    fy = (y - 0.5) - y0
    x1 = x0 + 1
    y1 = y0 + 1
    x0 %= w
    x1 %= w
    y0 = np.clip(y0, 0, h - 1)
    y1 = np.clip(y1, 0, h - 1)
    a = img[y0, x0].astype(np.float64)
    b = img[y0, x1].astype(np.float64)
    c = img[y1, x0].astype(np.float64)
    d = img[y1, x1].astype(np.float64)
    fx = fx[..., None]
    fy = fy[..., None]
    return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy


def caps(img, cap_px: int = CAP_PX):
    """Both caps side by side (2 * cap_px x cap_px)."""
    import numpy as np

    h, w = img.shape[:2]
    pre = prefilter_rows(img, cap_px)
    j, i = np.mgrid[0:cap_px, 0:cap_px]
    x = (i + 0.5) / cap_px * 2.0 - 1.0
    y = (j + 0.5) / cap_px * 2.0 - 1.0
    r = np.sqrt(x * x + y * y)
    colat = np.minimum(r, 1.2) * CAP_DEG
    u = np.arctan2(y, x) / (2 * np.pi)
    tx = np.mod(u, 1.0) * w
    out = []
    for north in (True, False):
        lat = (90.0 - colat) if north else (colat - 90.0)
        ty = (0.5 - lat / 180.0) * h
        out.append(bilinear(pre, tx, ty))
    res = np.concatenate(out, axis=1)
    return np.clip(np.round(res), 0, 255).astype(np.uint8)


def export(body_dir: Path) -> dict | None:
    import numpy as np
    from PIL import Image

    info_path = body_dir / "planet.json"
    if not info_path.exists() or not (body_dir / "albedo.jpg").exists():
        return None
    meta = {"cap_deg": CAP_DEG, "albedo": True, "clouds": False}
    alb = np.asarray(Image.open(body_dir / "albedo.jpg").convert("RGB"))
    rgb = caps(alb)
    a = np.zeros(rgb.shape[:2], dtype=np.uint8)
    if (body_dir / "clouds.png").exists():
        cl = np.asarray(Image.open(body_dir / "clouds.png").convert("RGBA"))
        a = caps(cl)[..., 3]
        meta["clouds"] = True
    # one texture (the client is at its sampled-texture limit): albedo in RGB, cloud coverage in alpha
    Image.fromarray(np.dstack([rgb, a]), "RGBA").save(body_dir / "poles.png", optimize=True)
    for old in ("poles_albedo.jpg", "poles_clouds.png"):
        (body_dir / old).unlink(missing_ok=True)
    info = json.loads(info_path.read_text(encoding="utf-8"))
    info["poles"] = meta
    info_path.write_text(json.dumps(info, indent=1), encoding="utf-8")
    return meta


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--planets", default="assets/web/planets")
    ap.add_argument("--bodies", default="")
    a = ap.parse_args()
    root = Path(a.planets)
    wanted = [b for b in a.bodies.split(",") if b]
    for d in sorted(p for p in root.iterdir() if p.is_dir()):
        if wanted and d.name not in wanted:
            continue
        print(d.name, export(d))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
