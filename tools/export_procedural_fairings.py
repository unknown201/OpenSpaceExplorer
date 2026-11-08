"""Export KSP2's procedural fairing definitions (`Data_Fairing`) to assets/own/procedural-fairings.json.

`ose_data::fairing` shapes a fairing from these: the base and cap radii, the widest allowed, how high a wall
section may be, the panel mass per square metre (`MassAreaRatio`), how it comes apart (`DeployType`) and how
hard (`EjectionForce`), and where the wall starts (the part's `FairingNode`). Read from the extracted part
data (tools/extract_bundles.py); nothing from the install is needed.

    .venv/Scripts/python tools/export_procedural_fairings.py
"""
from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PIECES = {"Clamshellx2": 2, "Clamshellx3": 3, "Clamshellx4": 4, "Shroud": 0}


def main() -> None:
    catalog = json.loads((ROOT / "assets/own/parts-catalog.json").read_text(encoding="utf-8"))
    out = {}
    for p in catalog:
        if not (p["id"].startswith("fairing_") and p.get("fairing")):
            continue
        data = json.loads((ROOT / f"assets/ksp2/data/parts/{p['id']}.json").read_text(encoding="utf-8-sig"))["data"]
        f = next(md["DataObject"] for m in data["serializedPartModules"] for md in m["ModuleData"] if md["Name"] == "Data_Fairing")
        stored = lambda k: f[k]["storedValue"] if isinstance(f[k], dict) else f[k]  # noqa: E731
        node = next((n for n in data.get("attachNodes", []) if n["nodeID"] == f["FairingNode"]), None)
        out[p["id"]] = {
            "mass_t": data["mass"],
            "base_radius": f["BaseRadius"],
            "cap_radius": f["CapRadius"],
            "max_radius": f["MaxRadius"],
            "mass_area_ratio": f["MassAreaRatio"],
            "height_min": f["CrossSectionHeightMin"],
            "height_max": f["CrossSectionHeightMax"],
            "side_count": f["FairingSideCount"],
            "length_snap": f["FairingLengthSnapIncrement"],
            "radius_snap": f["FairingRadiusSnapIncrement"],
            "ejection_force": stored("EjectionForce"),
            "pieces": PIECES.get(stored("DeployType"), 4),
            "base_height": node["position"]["y"] if node else 0.0,
        }
    path = ROOT / "assets/own/procedural-fairings.json"
    path.write_text(json.dumps(out, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(out)} fairings)")


if __name__ == "__main__":
    main()
