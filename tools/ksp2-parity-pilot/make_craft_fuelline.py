"""Build craft-fuelline.json for the KSP2 parity pilot's "fuelline" scenario (T39).

Pod, FL-T800-sized 1x4 tank A, inline decoupler (crossfeed off), 1x4 tank B, Swivel; a fuel line runs from
A (first anchor) to B (far anchor), so the engine below the decoupler can only reach A through the line.

The pod, tanks and engine come from craft-stack.json (stock assemblies); the decoupler and the compound part
module layout come from the pilot's craft-k1.json and craft-rover.json (a strut) in the KSP2 install's
BepInEx/ose-parity folder, passed as the first argument.

    python -I tools/ksp2-parity-pilot/make_craft_fuelline.py "<KSP2>/BepInEx/ose-parity"
"""

import copy
import json
import sys
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load(p: Path) -> dict:
    return json.loads(p.read_text(encoding="utf-8-sig"))


def guid(g: str) -> dict:
    return {"Guid": g, "DebugName": None}


def node(node_id: str, to: str, other: str) -> dict:
    return {
        "nodeId": node_id,
        "IsDynamic": False,
        "attachedPartGuid": None,
        "AttachedPartGuid": guid(to),
        "attachedPartNodeID": other,
        "SizeForDynamic": 0.0,
        "LocalPositionForDynamic": {"x": 0.0, "y": 0.0, "z": 0.0},
        "LocalRotationForDynamic": {"x": 0.0, "y": 0.0, "z": 0.0, "w": 0.0},
    }


def main() -> None:
    src = Path(sys.argv[1])
    stack = load(HERE / "craft-stack.json")
    k1 = load(src / "craft-k1.json")["Assemblies"][0]["Assembly"]
    rover = load(src / "craft-rover.json")["Assemblies"][0]["Assembly"]

    craft = copy.deepcopy(stack)
    asm = craft["Assemblies"][0]["Assembly"]
    parts = asm["parts"]
    pod, tank, engine = parts[0], parts[1], parts[-1]
    dec = copy.deepcopy(next(p for p in k1["parts"] if p["partName"] == "decoupler_1v_inline"))
    strut = next(p for p in rover["parts"] if p["partName"] == "strut_1v_extensible")

    ids = {k: str(uuid.UUID(int=0x05E0_0000_0000_0000_0000_0000_0000_0000 + n)) for n, k in
           enumerate(["pod", "a", "dec", "b", "eng", "line1", "line2"], 1)}

    def stack_part(p: dict, key: str, y: float, nodes: list) -> dict:
        p = copy.deepcopy(p)
        p["PartGuid"] = guid(ids[key])
        p["partGuid"] = None
        st = p["partState"]
        st["localPosition"] = {"x": 0.0, "y": y, "z": 0.0}
        st["attachNodeStates"] = nodes
        st["StageIndex"] = 0
        return p

    # geometry: 1x4 tank nodes at +-0.9375, inline decoupler nodes at +-0.0483, Swivel 1.7394 below its tank
    ya = -1.2775
    ydec = ya - 0.9375 - 0.0483
    yb = ydec - 0.0483 - 0.9375
    yeng = yb - 1.7394
    new = [
        stack_part(pod, "pod", 0.0, [node("bottom", ids["a"], "top")]),
        stack_part(tank, "a", ya, [node("top", ids["pod"], "bottom"), node("bottom", ids["dec"], "top")]),
        stack_part(dec, "dec", ydec, [node("top", ids["a"], "bottom"), node("bottom", ids["b"], "top")]),
        stack_part(tank, "b", yb, [node("top", ids["dec"], "bottom"), node("bottom", ids["eng"], "top")]),
        stack_part(engine, "eng", yeng, [node("top", ids["b"], "bottom")]),
    ]
    new[2]["partState"]["FuelCrossfeed"] = False

    # the fuel line: two anchors on the +X side, A's low down and B's high up, turned to face the tank
    def anchor(key: str, parent: str, y: float, other: str, first: bool, dy: float) -> dict:
        p = copy.deepcopy(strut)
        p["partName"] = "fuelline_0v_external"
        p["PartGuid"] = guid(ids[key])
        p["partGuid"] = None
        st = p["partState"]
        st["attachNodeStates"] = [node("srfAttach", ids[parent], "")]
        st["localPosition"] = {"x": 0.625, "y": y, "z": 0.0}
        st["localRotation"] = {"x": 0.0, "y": 1.0, "z": 0.0, "w": 0.0}
        st["StageIndex"] = 0
        st["FuelCrossfeed"] = True
        for m in p["PartModulesState"]:
            if m["Name"] == "PartComponentModule_Strut":
                m["Name"] = "PartComponentModule_FuelLine"
                for f in ("ComponentType", "BehaviourType"):
                    m[f] = m[f].replace("PartComponentModule_Strut", "PartComponentModule_FuelLine").replace(
                        "Module_Strut", "Module_FuelLine")
                for md in m["ModuleData"]:
                    if md["Name"] == "Data_Strut":
                        md["Name"] = "Data_FuelLine"
                        for f in ("ModuleType", "DataType"):
                            md[f] = md[f].replace("Module_Strut", "Module_FuelLine").replace("Data_Strut", "Data_FuelLine")
                        if isinstance(md.get("DataObject"), dict):
                            for f in ("$type", "ModuleType", "DataType"):
                                if f in md["DataObject"]:
                                    md["DataObject"][f] = md["DataObject"][f].replace("Module_Strut", "Module_FuelLine").replace(
                                        "Data_Strut", "Data_FuelLine")
                    if md["Name"] == "Data_CompoundPart":
                        o = md["DataObject"]
                        o["OtherEndAnchor"] = guid(ids[other])
                        o["IsFirstAnchor"] = first
                        # the other anchor in this one's frame (turned half round Y: x and z flip)
                        o["LocalOffset"] = {"x": 0.0, "y": dy, "z": 0.0}
        return p

    y1, y2 = ya - 0.6, yb + 0.6
    new.append(anchor("line1", "a", y1, "line2", True, y2 - y1))
    new.append(anchor("line2", "b", y2, "line1", False, y1 - y2))
    asm["parts"] = new

    # stages: the engine fires first (highest id), the decoupler never in this run
    stages = [
        {"partGuids": None, "PartIds": [guid(ids["dec"])], "StageID": 0, "IsActive": False},
        {"partGuids": None, "PartIds": [guid(ids["eng"])], "StageID": 1, "IsActive": False},
    ]
    asm["stagingState"]["availableStages"] = stages
    asm["AssemblyOABConfig"]["StagingState"]["availableStages"] = copy.deepcopy(stages)
    asm["AssemblyOABConfig"]["RootGuid"] = guid(ids["pod"])
    meta = craft["Metadata"]
    meta["Name"] = meta["VehicleName"] = asm["AssemblyDefinition"]["assemblyName"] = "FuelLine"
    meta["Parts"] = len(new)
    meta["Stages"] = 2
    (HERE / "craft-fuelline.json").write_text(json.dumps(craft, indent=1), encoding="utf-8")
    print("wrote craft-fuelline.json:", [p["partName"] for p in new])

    # the "crossfeed" scenario: no fuel line, the decoupler's crossfeed switched on instead
    cf = copy.deepcopy(craft)
    cparts = cf["Assemblies"][0]["Assembly"]["parts"]
    cparts[:] = [p for p in cparts if p["partName"] != "fuelline_0v_external"]
    next(p for p in cparts if p["partName"] == "decoupler_1v_inline")["partState"]["FuelCrossfeed"] = True
    cf["Metadata"]["Name"] = cf["Metadata"]["VehicleName"] = "Crossfeed"
    cf["Metadata"]["Parts"] = len(cparts)
    (HERE / "craft-crossfeed.json").write_text(json.dumps(cf, indent=1), encoding="utf-8")
    print("wrote craft-crossfeed.json:", [p["partName"] for p in cparts])


if __name__ == "__main__":
    main()
