"""Export solar panel tracking and raycast transforms from the user's KSP2 install.

For every part with a `Data_SolarPanel` it reads the part prefab (read-only) and writes a `solar` entry into the
part's `flight_geometry` in assets/own/flight-geometry.json and assets/own/parts-catalog.json:

- tracking panels (`Data_Deployable.isTracking`): the pivot (`pivotName`, e.g. bone_rotator) at rest in part
  space (Unity axes), `TrackingAlignmentOffset`, `trackingSpeed`, and, in the pivot's own frame, the transform
  the sun dot is taken on (`raycastTransform` when `UseRaycastForTrackingDot`, else the pivot) turned into its
  `PanelIncidenceDirection`, and the raycast origin;
- fixed panels: the incidence direction and raycast origin in part space.

`Module_Deployable.CalculateTracking` turns the pivot about its own Y toward the sun; `Module_SolarPanel`
takes the incidence dot on the dot transform and raycasts from `raycastTransform` (plus `RaycastOffset` toward
the sun) for parts in the way. Only the solar entries are added; everything else in both files is left as it is.

    .venv/Scripts/python -I tools/export_solar_geometry.py [--install <KSP2 folder>]
"""
from pathlib import Path
import argparse
import json
import sys

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from export_web_assets import Exporter, DEFAULT_INSTALL, ROOT  # noqa: E402
from export_menu_scene import load_scene  # noqa: E402
from export_flight_geometry import obj, quaternion  # noqa: E402


def modules(source):
    out = {}
    for m in source.get("serializedPartModules", []):
        for d in m.get("ModuleData", []):
            out[d["Name"]] = d["DataObject"]
    return out


def solar_entry(nodes, deployable, panel):
    """The `solar` entry for one part from its prefab nodes (rest pose) and module data."""
    by_name = {}
    for n in nodes:
        by_name.setdefault(n.name, n)
    raycast = by_name.get(deployable.get("secondaryTransform") or "raycastTransform")
    pivot = by_name.get(deployable.get("pivotName") or "")
    incidence = np.array([panel["PanelIncidenceDirection"][c] for c in "xyz"], dtype=float)
    tracking = bool(deployable.get("isTracking")) and pivot is not None
    entry = {
        "tracking": tracking,
        "raycast_offset_m": float(panel.get("RaycastOffset", 0.25)),
    }
    if tracking:
        p = np.array(pivot.world, dtype=float)
        inv = np.linalg.inv(p)
        dot_node = raycast if panel.get("UseRaycastForTrackingDot") and raycast is not None else pivot
        d = inv @ np.array(dot_node.world, dtype=float)
        inc = d[:3, :3] @ incidence
        entry.update({
            "pivot_m": obj(p[:3, 3]),
            "pivot_rotation": quaternion(p),
            "alignment_offset_deg": float(deployable.get("TrackingAlignmentOffset", 0.0)),
            "tracking_speed": float(deployable.get("trackingSpeed", 0.25)),
            "incidence": obj(inc / np.linalg.norm(inc)),
            "raycast_m": obj((inv @ np.array(raycast.world, dtype=float))[:3, 3]) if raycast is not None else obj([0, 0, 0]),
        })
    else:
        node = raycast
        w = np.array(node.world, dtype=float) if node is not None else np.eye(4)
        inc = w[:3, :3] @ incidence
        entry.update({
            "incidence": obj(inc / np.linalg.norm(inc)),
            "raycast_m": obj(w[:3, 3]),
        })
    return entry


def insert_after(text, anchor, start, insertion):
    """Insert `insertion` right after the first `anchor` at or after `start`."""
    at = text.index(anchor, start) + len(anchor)
    return text[:at] + insertion + text[at:]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--install", default=DEFAULT_INSTALL)
    args = parser.parse_args()
    ex = Exporter(Path(args.install), ROOT / "assets/web", 1024, False)
    catalog_path = ROOT / "assets/own/parts-catalog.json"
    geometry_path = ROOT / "assets/own/flight-geometry.json"
    catalog = catalog_path.read_text(encoding="utf-8")
    geometry = geometry_path.read_text(encoding="utf-8")
    assert '"solar":' not in catalog and '"solar":' not in geometry, "solar entries already exported"
    entries = {}
    for p in json.loads(catalog):
        part = p["id"]
        source = json.loads((ROOT / "assets/ksp2/data/parts" / f"{part}.json").read_text(encoding="utf-8"))["data"]
        mods = modules(source)
        if "Data_SolarPanel" not in mods:
            continue
        env, _ = ex.env_for_key(part + ".prefab")
        roots = [r for sf in env.assets for r in load_scene(sf) if r.name == part] if env else []
        if not roots:
            print("no prefab for", part)
            continue
        entries[part] = solar_entry(list(roots[0].walk()), mods.get("Data_Deployable", {}), mods["Data_SolarPanel"])
        ex._envs.clear()
        print(part, entries[part])
    for part, entry in entries.items():
        text = '"solar":' + json.dumps(entry, separators=(",", ":")) + ","
        catalog = insert_after(catalog, '"flight_geometry":{', catalog.index(f'"id":"{part}"'), text)
        geometry = insert_after(geometry, f'"{part}":{{', 0, text)
    json.loads(catalog)
    json.loads(geometry)
    catalog_path.write_text(catalog, encoding="utf-8")
    geometry_path.write_text(geometry, encoding="utf-8")
    print("Exported solar geometry for", len(entries), "parts")


if __name__ == "__main__":
    main()
