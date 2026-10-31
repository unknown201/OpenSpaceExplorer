#!/usr/bin/env python
"""Render the celestial bodies as small lit spheres for the tracking station and map lists, as the original's
body icons are renders of the planets rather than one flat glyph: assets/web/ui/bodies/<Body>.png, 64 px RGBA.

Each body is its planet albedo (assets/web/planets/<Body>/albedo.jpg, equirectangular) with its clouds where
it has them, lit from the upper left with a soft terminator, and an air rim on bodies with an atmosphere
(assets/own/kerbol-system.json); the star is a limb-darkened disc. Push with `push_assets.py ui`.

    python tools/render_body_icons.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
PLANETS = ROOT / "assets" / "web" / "planets"
DEST = ROOT / "assets" / "web" / "ui" / "bodies"
SIZE = 64
SUPER = 4
# Longitude turned to face the camera (deg): Kerbin shows the KSC's continent.
FACE_LON = {"Kerbin": -75.0}
# Air colour seen at the limb (linear RGB 0..1).
AIR = {
    "Kerbin": (0.45, 0.65, 1.0),
    "Laythe": (0.45, 0.65, 1.0),
    "Eve": (0.75, 0.55, 1.0),
    "Duna": (1.0, 0.65, 0.5),
    "Jool": (0.65, 0.95, 0.55),
}
# Bodies whose albedo leaves out the sea.
SEA_FROM_BIOME = {"Laythe"}
LIGHT = np.array([-0.55, 0.45, 0.70])
LIGHT = LIGHT / np.linalg.norm(LIGHT)


def sphere(n: int):
    """Unit normals of the disc's pixels (x right, y up, z to the camera) and its coverage mask."""
    c = (np.arange(n) + 0.5) / n * 2.0 - 1.0
    x, y = np.meshgrid(c, -c)
    r2 = x * x + y * y
    inside = r2 <= 1.0
    z = np.sqrt(np.clip(1.0 - r2, 0.0, 1.0))
    return x, y, z, inside


def sample(tex: np.ndarray, lat: np.ndarray, lon: np.ndarray) -> np.ndarray:
    h, w = tex.shape[:2]
    u = ((lon / (2 * np.pi) + 0.5) % 1.0) * (w - 1)
    v = np.clip(0.5 - lat / np.pi, 0.0, 1.0) * (h - 1)
    return tex[v.astype(int), u.astype(int)]


def render_planet(body: str, atmosphere: bool) -> Image.Image:
    n = SIZE * SUPER
    albedo = np.asarray(Image.open(PLANETS / body / "albedo.jpg").convert("RGB"), dtype=np.float32) / 255.0
    clouds_path = PLANETS / body / "clouds.png"
    clouds = (
        np.asarray(Image.open(clouds_path).convert("RGBA"), dtype=np.float32) / 255.0
        if clouds_path.exists()
        else None
    )
    x, y, z, inside = sphere(n)
    lat = np.arcsin(np.clip(y, -1.0, 1.0))
    lon = np.arctan2(x, z) + np.radians(FACE_LON.get(body, 0.0))
    # sRGB texture to linear light
    col = sample(albedo, lat, lon) ** 2.2
    # Laythe's albedo is all seabed (Kerbin's and Eve's have their seas painted in): the water goes where the
    # biome map's red channel (its ocean biome) leads, in the ocean's own colour.
    ocean = json.loads((PLANETS / body / "planet.json").read_text(encoding="utf-8")).get("ocean")
    biome_path = PLANETS / body / "biome.png"
    if body in SEA_FROM_BIOME and ocean and biome_path.exists():
        biome = np.asarray(Image.open(biome_path).convert("RGB"), dtype=np.float32) / 255.0
        bm = sample(biome, lat, lon)
        water = np.clip((bm[..., 0] - np.maximum(bm[..., 1], bm[..., 2])) * 2.0, 0.0, 1.0)[..., None]
        sea = np.array(ocean.get("DeepWaterColor", [0.0, 0.3, 0.6])[:3], dtype=np.float32) ** 2.2
        col = col * (1.0 - water) + sea * water
    if clouds is not None:
        cl = sample(clouds, lat, lon)
        a = cl[..., 3:4]
        col = col * (1.0 - a) + (cl[..., :3] ** 2.2) * a
    ndl = x * LIGHT[0] + y * LIGHT[1] + z * LIGHT[2]
    light = np.clip(ndl * 1.1 + 0.1, 0.0, 1.0) ** 0.9 * 1.15 + 0.035
    col = col * light[..., None]
    alpha = inside.astype(np.float32)
    if atmosphere:
        air = np.array(AIR.get(body, (0.6, 0.75, 1.0)), dtype=np.float32)
        rim = (1.0 - z) ** 3 * np.clip(ndl + 0.35, 0.0, 1.0)
        col = col + rim[..., None] * air * 0.9
    rgb = np.clip(col, 0.0, 1.0) ** (1 / 2.2)
    out = np.dstack([rgb, alpha])
    return downsample(out)


def render_star() -> Image.Image:
    n = SIZE * SUPER
    x, y, z, inside = sphere(n)
    limb = 0.55 + 0.45 * z
    rgb = np.dstack([np.ones_like(z), 0.9 * limb + 0.1, 0.55 * limb]) * limb[..., None] ** 0.3
    return downsample(np.dstack([np.clip(rgb, 0, 1), inside.astype(np.float32)]))


def downsample(rgba: np.ndarray) -> Image.Image:
    n = SIZE
    a = rgba.reshape(n, SUPER, n, SUPER, 4)
    alpha = a[..., 3].mean(axis=(1, 3))
    # colour averaged over the covered samples only, so the edge does not darken
    rgb = (a[..., :3] * a[..., 3:4]).sum(axis=(1, 3)) / np.maximum(a[..., 3:4].sum(axis=(1, 3)), 1e-6)
    img = np.dstack([rgb, alpha])
    return Image.fromarray((np.clip(img, 0, 1) * 255 + 0.5).astype(np.uint8), "RGBA")


def main() -> int:
    system = json.loads((ROOT / "assets" / "own" / "kerbol-system.json").read_text(encoding="utf-8"))
    if not PLANETS.exists():
        sys.exit(f"render_body_icons: no exported planets under {PLANETS}")
    DEST.mkdir(parents=True, exist_ok=True)
    for b in system:
        body = b["id"]
        if b.get("is_star"):
            img = render_star()
        elif (PLANETS / body / "albedo.jpg").exists():
            img = render_planet(body, bool(b.get("atmosphere")))
        else:
            print(f"  {body}: no albedo, skipped")
            continue
        img.save(DEST / f"{body}.png", optimize=True)
    print(f"body icons: {len(list(DEST.glob('*.png')))} -> {DEST}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
