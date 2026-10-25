#!/usr/bin/env python
"""Export the original's loading screen art into assets/web/loading/<key>.jpg (1280x720) for the client's
scene transitions, the way the original shows one art per destination with a loading tip.

    .venv/Scripts/python tools/export_loading.py
    .venv/Scripts/python tools/push_assets.py loading
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter  # noqa: E402

BUNDLE = "loadingscreens_assets_all.bundle"
# Client key -> the original's texture.
ART = {
    "game": "load_01_gameload_01_CROP",
    "game2": "load_05_gameload_05_CROP",
    "ksc": "load_06_ksc_CROP",
    "training": "load_07_trainingcenter_01_COMP",
    "mission": "load_08_mission_control_CROP",
    "tracking": "load_09_tracking_station_CROP",
    "rnd": "load_10_research_development_CROP",
    "vab": "load_11_vab_ksc_CROP",
    "launchpad": "load_14_launchpad_ksc_CROP",
    "runway": "load_17_runway_ksc_COMP_4K",
    "recover": "load_23_recover_kerbin_comp",
    "crash": "load_22_crash_kerbin_comp",
}


def main() -> int:
    out = ROOT / "assets" / "web" / "loading"
    out.mkdir(parents=True, exist_ok=True)
    ex = Exporter(Path(DEFAULT_INSTALL), ROOT / "assets" / "web", 2048, False)
    by_name = {v: k for k, v in ART.items()}
    done = set()
    for o in ex.env(BUNDLE).objects:
        if o.type.name != "Texture2D":
            continue
        try:
            name = o.peek_name()
        except Exception:  # noqa: BLE001
            continue
        key = by_name.get(name)
        if key is None or key in done:
            continue
        img = o.read().image.convert("RGB").resize((1280, 720))
        img.save(out / f"{key}.jpg", quality=82, optimize=True)
        done.add(key)
        print(key, name)
    missing = set(ART) - done
    if missing:
        print("missing:", sorted(missing))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
