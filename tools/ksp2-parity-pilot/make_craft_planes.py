"""Build craft-plane-<name>.json for the KSP2 parity pilot's "jetfly <name>" scenario from the planes OSE's
hangar editor builds (crates/ose-sim/tests/data/editor-planes.json, written by ose-client's plane_build test).

Each part is the stock jet's own part of that kind (craft-jet.json; parts it lacks, such as a solid booster,
from the pilot's other crafts) with OSE's pose, mirror flag, wing shape and
links put in, so KSP2 flies exactly the plane OSE flies. Settings OSE leaves at the part's defaults are written
as KSP2's defaults: control surfaces answer pitch, yaw and roll at full authority, every wheel steers. A plane
with a part no pilot craft has is skipped and named.

    python -I tools/ksp2-parity-pilot/make_craft_planes.py
"""

import copy
import json
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
PLANES = REPO / "crates" / "ose-sim" / "tests" / "data" / "editor-planes.json"
RIGS = REPO / "assets" / "own" / "procedural-wings.json"
CATALOG = REPO / "assets" / "own" / "parts-catalog.json"

# OSE plane name -> scenario name ("jetfly <name>", craft-plane-<name>.json)
NAMES = {"Small": "small", "Small straight": "straight", "Small swept": "swept", "Booster plane": "booster"}


def guid_of(plane: str, i: int) -> str:
    return str(uuid.uuid5(uuid.NAMESPACE_URL, f"ose-editor-plane/{plane}/{i}"))


def ref(g: str) -> dict:
    return {"Guid": g, "DebugName": None}


def modules(part: dict, name: str):
    for m in part.get("PartModulesState") or []:
        for d in m.get("ModuleData") or []:
            if d.get("Name") == name:
                yield d["DataObject"]


def slider_starts(part_id: str, option: int) -> dict:
    rig = json.loads(RIGS.read_text(encoding="utf-8"))["parts"][part_id]
    opt = rig["options"][min(option, len(rig["options"]) - 1)]
    return {s["name"]: s["start"] for s in opt["sliders"]}


def build(plane: dict, template: dict, ranges: dict, others: list) -> dict:
    craft = copy.deepcopy(template)
    asm = craft["Assemblies"][0]["Assembly"]
    kinds = {}
    for t in [template] + others:
        for p in t["Assemblies"][0]["Assembly"]["parts"]:
            kinds.setdefault(p["partName"], p)
    parts = plane["parts"]
    missing = sorted({p["part_id"] for p in parts} - set(kinds))
    if missing:
        raise KeyError(", ".join(missing))
    name = NAMES[plane["name"]]
    guids = [guid_of(name, i) for i in range(len(parts))]
    out = []
    for i, p in enumerate(parts):
        k = copy.deepcopy(kinds[p["part_id"]])
        k["PartGuid"] = ref(guids[i])
        st = k["partState"]
        pos, rot = p["local_position"], p["local_rotation"]
        st["localPosition"] = {a: pos[a] for a in "xyz"}
        st["localRotation"] = {a: rot[a] for a in "xyzw"}
        st["UserRotation"] = {"x": 0.0, "y": 0.0, "z": 0.0, "w": 1.0}
        st["isMirrored"] = bool((p.get("flight") or {}).get("mirrored"))
        st["StageIndex"] = p.get("stage_index", 0)
        st["FuelCrossfeed"] = p.get("fuel_crossfeed", True)
        if st.get("resources"):
            for r in p.get("resources", []):
                if r["resource"] in st["resources"]:
                    st["resources"][r["resource"]]["storedUnits"] = r["stored_units"]
                    st["resources"][r["resource"]]["capacityUnits"] = r["capacity_units"]
        # links: a stack child names its parent's node, the parent names the child back; a surface child names
        # its parent with no node, as KSP2 saves them
        nodes = []
        for n in st["attachNodeStates"]:
            n = copy.deepcopy(n)
            n["AttachedPartGuid"] = ref("00000000-0000-0000-0000-000000000000")
            n["attachedPartNodeID"] = ""
            nodes.append(n)
        by_id = {n["nodeId"]: n for n in nodes}
        if p.get("parent") is not None:
            own = p.get("own_node") or "srfAttach"
            n = by_id[own]
            n["AttachedPartGuid"] = ref(guids[p["parent"]])
            n["attachedPartNodeID"] = p.get("parent_node") or ""
        for j, c in enumerate(parts):
            if c.get("parent") == i and c.get("parent_node"):
                n = by_id[c["parent_node"]]
                n["AttachedPartGuid"] = ref(guids[j])
                n["attachedPartNodeID"] = c.get("own_node") or ""
        st["attachNodeStates"] = nodes
        for d in modules(k, "Data_ProceduralPart"):
            shape = p.get("shape") or {}
            option = shape.get("option")
            if option is None:
                option = d.get("meshOptionIndex", 1)
            values = slider_starts(p["part_id"], option)
            values.update({n: v for n, v in shape.get("sliders", [])})
            d["meshOptionIndex"] = option
            d["sliderData"] = {n: values[n] for n in d["sliderData"]}
        for d in modules(k, "Data_ControlSurface"):
            for key in ("EnablePitch", "EnableYaw", "EnableRoll"):
                d[key]["storedValue"] = True
            d["InvertControl"]["storedValue"] = False
            d["Deploy"]["storedValue"] = False
            if p["part_id"] in ranges:
                d["AuthorityLimiter"]["storedValue"] = ranges[p["part_id"]]
        for d in modules(k, "Data_WheelSteering"):
            d["SteeringEnabled"]["storedValue"] = True
            d["SteeringInvert"]["storedValue"] = False
        out.append(k)
    asm["parts"] = out
    root = next(i for i, p in enumerate(parts) if p.get("parent") is None)
    engines = [guids[i] for i, p in enumerate(parts) if p["part_id"].startswith(("engine_", "booster_"))]
    stage = {"availableStages": [{"partGuids": None, "PartIds": [ref(g) for g in engines], "StageID": 0, "IsActive": False}],
             "unassignedParts": None, "totalStageCount": 0}
    asm["stagingState"] = copy.deepcopy(stage)
    asm["AssemblyOABConfig"]["StagingState"] = copy.deepcopy(stage)
    asm["AssemblyOABConfig"]["RootGuid"] = ref(guids[root])
    # mirror pairs as the hangar made them
    sets = {}
    for i, p in enumerate(parts):
        if p.get("symmetry"):
            sets.setdefault(p["symmetry"], []).append(guids[i])
    sym = [{"Version": "0.1", "SymmetryType": "Mirror", "Anchor": ref("00000000-0000-0000-0000-000000000000"),
            "AllParts": [ref(g) for g in gs]} for gs in sets.values() if len(gs) == 2]
    asm["AssemblyOABConfig"]["SymmetrySets"] = sym
    asm["SymmetrySets"] = copy.deepcopy(sym)
    title = "OSE " + plane["name"]
    asm["AssemblyDefinition"]["assemblyName"] = title
    asm["AssemblyDefinition"]["description"] = title
    craft["Metadata"].update({"Name": title, "VehicleName": title, "Description": title, "Parts": len(out)})
    return craft


def main() -> None:
    planes = json.loads(PLANES.read_text(encoding="utf-8"))
    template = json.loads((HERE / "craft-jet.json").read_text(encoding="utf-8-sig"))
    ranges = {d["id"]: d["ctrl_range_deg"] for d in json.loads(CATALOG.read_text(encoding="utf-8"))
              if d.get("ctrl_range_deg")}
    others = [json.loads(f.read_text(encoding="utf-8-sig")) for f in sorted(HERE.glob("craft-*.json"))
              if f.name != "craft-jet.json" and not f.name.startswith("craft-plane-")]
    for plane in planes:
        if plane["name"] not in NAMES:
            print(f"{plane['name']}: no KSP2 scenario name, skipped")
            continue
        try:
            craft = build(plane, template, ranges, others)
        except KeyError as e:
            print(f"{plane['name']}: the stock jet has no {e}, skipped")
            continue
        path = HERE / f"craft-plane-{NAMES[plane['name']]}.json"
        path.write_text(json.dumps(craft, indent=1) + "\n", encoding="utf-8")
        print(f"{path.name}: {len(plane['parts'])} parts")


if __name__ == "__main__":
    main()
