#!/usr/bin/env python
"""Export the original's flight HUD and VAB icons (SAS modes, action bar, staging, time warp, workspace tools)
into assets/web/ui/hud/<name>.png for the client's HUD and VAB (docs/ui-parity-ksp2.md).

    .venv/Scripts/python tools/export_hud_icons.py
    .venv/Scripts/python tools/push_assets.py ui

Names are Texture2D or Sprite names from hudinstruments_assets_all.bundle and ui_assets_all.bundle; the first
match wins. Listed names that are not found are printed.
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter  # noqa: E402

BUNDLES = ["hudinstruments_assets_all.bundle", "ui_assets_all.bundle"]
ICONS = [
    # SAS modes around the navball.
    "Flight-ICO-StabilityAssist", "Flight-ICO-Prograde", "Flight-ICO-Retrograde", "Flight-ICO-Normal",
    "Flight-ICO-AntiNormal", "Flight-ICO-RadialIn", "Flight-ICO-RadialOut", "Flight-ICO-Target",
    "Flight-ICO-AntiTarget", "Flight-ICO-Maneuver",
    # Action bar.
    "ICO-Action-Abort-24x24", "ICO-Action-Brakes-24x24", "ICO-Action-LandingGear-24x24", "ICO-Action-Lights-24x24",
    "ICO-Action-Radiator-24x24", "ICO-Action-Solar-24x24", "ICO-Action-Jetpack-24x24",
    "ICO-AppBar-ResearchManager-24x24", "ICO-AppBar-MissionTracker-24x24", "ICO-AppBar-ActionGroup-24x24",
    # Staging stack.
    "Staging-ICO-Chute-Stowed", "Staging-ICO-Chute-Armed", "Staging-ICO-Chute-Deployed", "Staging-ICO-Chute-Cut",
    "Staging-ICO-Booster", "Staging-ICO-Radial", "Staging-Icon-Thruster", "Staging-Icon-Inline", "Staging-ICO-Port",
    "Staging-ICO-Structure01", "ICO-StagePart-Booster",
    # Readouts, time warp, map and view toggles.
    "Flight-Readout-Option-Level", "Flight-Readout-Option-Orbit", "Flight-Readout-Option-Surface",
    "Flight-Readout-Option-Target", "ICO-Flight-Map", "ICO-Flight-Vessel", "ICO-Flight-TrackingStation",
    "ICO-Flight-Cluster-Commnet", "ICO-FlightControl-Lock", "ICO-WarpToNode", "ICO-DeleteManeuver",
    "BG-TimeWarp-Pause-onRest", "BG-TimeWarp-Play-onRest",
    # VAB workspace tools.
    "Workspace-ICO-LaunchVessel-Normal-Active", "Workspace-ICO-LaunchVessel-Normal-Inactive", "ToolsBar-ICO-Undo-onRest",
    "WB-ICO-EngineersReport", "WB-ICO-Trash", "TrashBin-ICO-onRest", "ICO-Delete", "WB-ICO-Filter",
    "ICO-InfoPanel-Favorite-24x24-Active", "ICO-InfoPanel-Favorite-24x24-Inactive", "SearchButton-ICO-onRest",
    "PartCategories-ICO-Favorites",
]


def main() -> int:
    out = ROOT / "assets" / "web" / "ui" / "hud"
    out.mkdir(parents=True, exist_ok=True)
    ex = Exporter(Path(DEFAULT_INSTALL), ROOT / "assets" / "web", 1024, False)
    wanted = set(ICONS)
    got: set[str] = set()
    for bundle in BUNDLES:
        env = ex.env(bundle)
        for o in env.objects:
            if o.type.name not in ("Texture2D", "Sprite"):
                continue
            try:
                name = o.peek_name()
            except Exception:  # noqa: BLE001
                name = None
            if name not in wanted or name in got:
                continue
            try:
                o.read().image.save(out / f"{name}.png")
                got.add(name)
            except Exception as e:  # noqa: BLE001
                print(f"  {name}: {e}")
    print(f"hud icons: {len(got)} of {len(wanted)} into {out}")
    missing = sorted(wanted - got)
    if missing:
        print("missing:", ", ".join(missing))
    return 0


if __name__ == "__main__":
    sys.exit(main())
