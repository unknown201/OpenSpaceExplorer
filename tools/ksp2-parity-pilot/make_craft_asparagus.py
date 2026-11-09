"""Build craft-asparagus.json for the KSP2 parity pilot's "asparagus" scenario (T39).

Pod, 1x4 core tank C, Swivel under it; two radial decouplers on C (+X and -X), each holding a 1x4 booster tank
with its own Swivel; a fuel line from each booster to the core (first anchor on the booster, far anchor on C).
All three engines fire in the first stage; the decouplers are never staged in this run. The trace shows which
tanks each engine drains and in what order (asparagus staging).

Parts come from craft-stack.json (pod, tank, Swivel), the pilot's craft-k1.json (radial decoupler) and its
craft-rover.json (the compound part module layout), in the KSP2 install's BepInEx/ose-parity folder:

    python -I tools/ksp2-parity-pilot/make_craft_asparagus.py "<KSP2>/BepInEx/ose-parity"
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


# a turn of half round Y (the +X side faces in) and none (the -X side)
HALF_Y = {"x": 0.0, "y": 1.0, "z": 0.0, "w": 0.0}
NONE = {"x": 0.0, "y": 0.0, "z": 0.0, "w": 1.0}


def fuel_line_from(strut: dict) -> dict:
    p = copy.deepcopy(strut)
    p["partName"] = "fuelline_0v_external"
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
    return p


def main() -> None:
    src = Path(sys.argv[1])
    stack = load(HERE / "craft-stack.json")
    k1 = load(src / "craft-k1.json")["Assemblies"][0]["Assembly"]
    rover = load(src / "craft-rover.json")["Assemblies"][0]["Assembly"]

    craft = copy.deepcopy(stack)
    asm = craft["Assemblies"][0]["Assembly"]
    pod, tank, engine = asm["parts"][0], asm["parts"][1], asm["parts"][-1]
    radial = next(p for p in k1["parts"] if p["partName"] == "decoupler_1v_radial")
    line = fuel_line_from(next(p for p in rover["parts"] if p["partName"] == "strut_1v_extensible"))

    keys = ["pod", "c", "eng_c", "dec_p", "dec_n", "b_p", "b_n", "eng_p", "eng_n", "lf_p", "le_p", "lf_n", "le_n"]
    ids = {k: str(uuid.UUID(int=0x05E1_0000_0000_0000_0000_0000_0000_0000 + n)) for n, k in enumerate(keys, 1)}

    def part(t: dict, key: str, pos: tuple, rot: dict, nodes: list) -> dict:
        p = copy.deepcopy(t)
        p["PartGuid"] = guid(ids[key])
        p["partGuid"] = None
        st = p["partState"]
        st["localPosition"] = {"x": pos[0], "y": pos[1], "z": pos[2]}
        st["localRotation"] = dict(rot)
        st["attachNodeStates"] = nodes
        st["StageIndex"] = 0
        return p

    # geometry: core tank below the pod as in the stack craft; the radial decoupler sits 0.02 m off the
    # tank's side and its booster's centre 1.4952 m out (the K1's Hammer boosters, the same 0.625 m node)
    yc = -1.2775
    yeng = yc - 1.7394
    xb = 1.4952
    new = [
        part(pod, "pod", (0.0, 0.0, 0.0), NONE, [node("bottom", ids["c"], "top")]),
        part(tank, "c", (0.0, yc, 0.0), NONE, [node("top", ids["pod"], "bottom"), node("bottom", ids["eng_c"], "top")]),
        part(engine, "eng_c", (0.0, yeng, 0.0), NONE, [node("top", ids["c"], "bottom")]),
    ]
    for side, sign, rot in (("p", 1.0, HALF_Y), ("n", -1.0, NONE)):
        new.append(part(radial, f"dec_{side}", (sign * 0.645, yc, 0.0), rot, [node("srfAttach", ids["c"], "")]))
        new[-1]["partState"]["FuelCrossfeed"] = False
        new.append(part(tank, f"b_{side}", (sign * xb, yc, 0.0), rot,
                        [node("srfAttach", ids[f"dec_{side}"], ""), node("bottom", ids[f"eng_{side}"], "top")]))
        new.append(part(engine, f"eng_{side}", (sign * xb, yeng, 0.0), rot, [node("top", ids[f"b_{side}"], "bottom")]))
        # the fuel line: first anchor on the booster's inner face near its top, far anchor on the core facing it
        first = (sign * (xb - 0.625), yc + 0.7, 0.0)
        far = (sign * 0.625, yc + 0.5, 0.0)
        # each anchor's node faces into the part it hangs on: the booster's inner face points at the core
        first_rot, far_rot = (NONE, HALF_Y) if sign > 0 else (HALF_Y, NONE)

        def local(rot: dict, d: tuple) -> dict:
            # the other anchor in this one's frame: a half turn round Y flips x and z
            if rot is HALF_Y:
                return {"x": -d[0], "y": d[1], "z": -d[2]}
            return {"x": d[0], "y": d[1], "z": d[2]}

        for key, pos, rot, parent, other, is_first, d in (
            (f"lf_{side}", first, first_rot, f"b_{side}", f"le_{side}", True, tuple(far[i] - first[i] for i in range(3))),
            (f"le_{side}", far, far_rot, "c", f"lf_{side}", False, tuple(first[i] - far[i] for i in range(3))),
        ):
            p = part(line, key, pos, rot, [node("srfAttach", ids[parent], "")])
            p["partState"]["FuelCrossfeed"] = True
            for m in p["PartModulesState"]:
                for md in m["ModuleData"]:
                    if md["Name"] == "Data_CompoundPart":
                        o = md["DataObject"]
                        o["OtherEndAnchor"] = guid(ids[other])
                        o["IsFirstAnchor"] = is_first
                        o["LocalOffset"] = local(rot, d)
            new.append(p)
    asm["parts"] = new

    # stages: the three engines fire first (highest id); the decouplers never in this run
    stages = [
        {"partGuids": None, "PartIds": [guid(ids["dec_p"]), guid(ids["dec_n"])], "StageID": 0, "IsActive": False},
        {"partGuids": None, "PartIds": [guid(ids["eng_c"]), guid(ids["eng_p"]), guid(ids["eng_n"])], "StageID": 1,
         "IsActive": False},
    ]
    asm["stagingState"]["availableStages"] = stages
    asm["AssemblyOABConfig"]["StagingState"]["availableStages"] = copy.deepcopy(stages)
    asm["AssemblyOABConfig"]["RootGuid"] = guid(ids["pod"])
    meta = craft["Metadata"]
    meta["Name"] = meta["VehicleName"] = asm["AssemblyDefinition"]["assemblyName"] = "Asparagus"
    meta["Parts"] = len(new)
    meta["Stages"] = 2
    (HERE / "craft-asparagus.json").write_text(json.dumps(craft, indent=1), encoding="utf-8")
    print("wrote craft-asparagus.json:", [p["partName"] for p in new])


if __name__ == "__main__":
    main()
