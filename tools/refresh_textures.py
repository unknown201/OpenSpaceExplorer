#!/usr/bin/env python
"""Rewrite the exported material textures that were downsized, without re-exporting any model.

Earlier exports resized RGBA textures with Pillow's default (premultiplied alpha), which zeroed the colour
channels where alpha is 0 and broke maps whose alpha carries data (DXT5nm normals, metallic smoothness,
masks). export_texture_object now resizes band by band and rewrites resized textures; this walks the
materials of every part, the KSC, the VAB and the main menu scene so each such texture is written again.

    .venv/Scripts/python tools/refresh_textures.py
    .venv/Scripts/python tools/push_assets.py textures
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter  # noqa: E402

SCENES = ["kerbin-ksc_assets_all.bundle", "oab_assets_all.bundle", "boot-ksp_scenes_all.bundle"]


def textures_of(ex: Exporter, env) -> int:
    n = 0
    for o in env.objects:
        if o.type.name != "Material":
            continue
        try:
            m = o.read()
        except Exception:  # noqa: BLE001
            continue
        for name, te in m.m_SavedProperties.m_TexEnvs:
            if name not in ex.MATERIAL_SLOTS or not te.m_Texture.path_id:
                continue
            try:
                tex = te.m_Texture.read()
            except Exception:  # noqa: BLE001
                continue
            if tex.__class__.__name__ != "Texture2D":
                continue
            big = tex.m_Width > ex.max_texture or tex.m_Height > ex.max_texture
            if big and tex.m_Name not in ex.texture_files:
                ex.export_texture_object(tex)
                n += 1
    return n


def main() -> int:
    ex = Exporter(Path(DEFAULT_INSTALL), ROOT / "assets" / "web", 1024, False)
    catalog = json.loads((ROOT / "assets" / "own" / "parts-catalog.json").read_text(encoding="utf-8"))
    parts = catalog if isinstance(catalog, list) else catalog.get("parts", catalog)
    ids = [p["id"] for p in parts] if isinstance(parts, list) else list(parts)
    total = 0
    for i, pid in enumerate(ids):
        env, _ = ex.env_for_key(f"{pid}.prefab")
        if env is None:
            continue
        total += textures_of(ex, env)
        if i % 50 == 0:
            print(f"parts {i}/{len(ids)}: {total} textures rewritten", flush=True)
    for bundle in SCENES:
        total += textures_of(ex, ex.env(bundle))
        print(f"{bundle}: {total} textures rewritten", flush=True)
    print(f"done: {total} downsized textures rewritten band by band")
    return 0


if __name__ == "__main__":
    sys.exit(main())
