"""Export KSP2's engine shrouds (an engine's `Data_Fairing`) to assets/own/engine-shrouds.json.

KSP2 does not draw an engine's shroud from a mesh in the engine's prefab: `Module_Fairing` builds it as a
procedural shroud when a part hangs from the engine's floating node (`FloatingAttachNodeTag`, the bottom
node). The wall has two cross sections along `LocalUpAxis` (down): the top ring at `FairingStartHeight`
with `BaseRadius`, the bottom ring `CrossSectionHeightMax` lower with half the target diameter
(`Module_Fairing.CalculateTargetAttachmentDiameter`: the attached node's size class, clamped by
`Min/MaxAutoFairingTargetRadius`, looked up in `ObjectAssemblyPartTracker.AssemblySizeTypeDiameters`). The
stock assemblies save exactly these sections (Stock12_StartRocket's Swivel: heights 0.122 and 1.802 below
the model origin at y 0.802, radius 0.625).

Some prefabs still carry an old static "fairing" mesh (the Swivel's hangs 0.87 m too low); the client hides
those and draws this shroud instead.

The file also lists every part's stack node sizes, which the target diameter is read from.

    .venv/Scripts/python tools/export_engine_shrouds.py [--parts-dir assets/ksp2/data/parts]
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def stored(v):
    return v["storedValue"] if isinstance(v, dict) and "storedValue" in v else v


def shroud_of(part_json: dict) -> dict | None:
    data = part_json["data"]
    f = next(
        (md["DataObject"] for m in data["serializedPartModules"] for md in m["ModuleData"] if md["Name"] == "Data_Fairing"),
        None,
    )
    if f is None or not stored(f["FairingEnabled"]):
        return None
    tag = f["FloatingAttachNodeTag"]
    node = next((n for n in data.get("attachNodes", []) if n["nodeID"] == tag), None)
    if node is None:
        return None
    return {
        "node": tag,
        "node_size": int(node.get("size", 1)),
        "start_height": f["FairingStartHeight"],
        "height": f["CrossSectionHeightMax"],
        "base_radius": f["BaseRadius"],
        "side_count": int(f["FairingSideCount"]),
        "min_target": int(f["MinAutoFairingTargetRadius"]),
        "max_target": int(f["MaxAutoFairingTargetRadius"]),
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--parts-dir", type=Path, default=ROOT / "assets/ksp2/data/parts")
    args = ap.parse_args()
    catalog = json.loads((ROOT / "assets/own/parts-catalog.json").read_text(encoding="utf-8"))
    shrouds = {}
    nodes = {}
    for p in catalog:
        stack = {n["id"]: n["size"] for n in p["attach_nodes"] if n["node_type"] == "Stack"}
        if stack:
            nodes[p["id"]] = stack
        if not (p["engines"] and "PartComponentModule_Fairing" in p["module_names"]):
            continue
        part = json.loads((args.parts_dir / f"{p['id']}.json").read_text(encoding="utf-8-sig"))
        s = shroud_of(part)
        if s is not None:
            shrouds[p["id"]] = s
    out = {"shrouds": shrouds, "stack_node_sizes": nodes}
    path = ROOT / "assets/own/engine-shrouds.json"
    path.write_text(json.dumps(out, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(shrouds)} shrouds, {len(nodes)} parts with stack nodes)")


if __name__ == "__main__":
    main()
