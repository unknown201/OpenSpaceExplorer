"""Build craft-fairing.json for the parity pilot's "fairings" scenario: the Mk1 pod with an AE-FF125 procedural
fairing base on its top node (gap T38, fairing drag cubes).

The pod comes from craft-drop-pod.json (the pilot's own pod-only craft). The fairing is its part definition's
serialized modules (`assets/ksp2/data/parts/fairing_1v_procedural.json`, the same layout a craft file stores)
with the given wall (default a 2 m cylinder), so KSP2 builds its panels when the craft is
loaded. The scenario reads the drag cubes KSP2 renders for it; the craft is named after the wall.

    python tools/ksp2-parity-pilot/make_craft_fairing.py <fairing_1v_procedural.json> <craft-drop-pod.json> <out json> [name capped(0/1) [part=<base part>] height:radius ...]
"""

import copy
import json
import os
import sys
import uuid

ZERO = "00000000-0000-0000-0000-000000000000"
POD_TOP_Y = 0.5580000281333923


def section(height, radius, last=False):
    return {
        "Height": height,
        "Radius": radius,
        "IsLast": last,
        "IsCap": False,
        "IsValid": True,
        "Color": {"r": 0.0, "g": 0.0, "b": 0.0, "a": 0.0},
        "DefaultHeight": height,
        "DefaultRadius": radius,
        "FairingPanelAttachedParts": [],
    }


def node(node_id, guid="", other=""):
    return {
        "nodeId": node_id,
        "IsDynamic": False,
        "attachedPartGuid": None,
        "AttachedPartGuid": {"Guid": guid or ZERO, "DebugName": None},
        "attachedPartNodeID": other,
        "SizeForDynamic": 0.0,
        "LocalPositionForDynamic": {"x": 0.0, "y": 0.0, "z": 0.0},
        "LocalRotationForDynamic": {"x": 0.0, "y": 0.0, "z": 0.0, "w": 0.0},
    }


def main(part_def, pod_craft, out, name="cylinder-2", capped="0", *sections):
    # "part=fairing_2v_procedural" picks another base (its definition beside `part_def`)
    options = dict(s.split("=", 1) for s in sections if "=" in s)
    walls = [tuple(float(x) for x in s.split(":")) for s in sections if "=" not in s] or [(0.0, 0.625), (2.0, 0.625)]
    if "part" in options:
        part_def = os.path.join(os.path.dirname(part_def), options["part"] + ".json")
    definition = json.load(open(part_def, encoding="utf-8-sig"))["data"]
    bottom_y = next(n["position"]["y"] for n in definition["attachNodes"] if n["nodeID"] == "bottom")
    doc = json.load(open(pod_craft, encoding="utf-8-sig"))
    asm = doc["Assemblies"][0]["Assembly"]
    pod = asm["parts"][0]
    assert pod["partName"] == "pod_1v_conical_crew", pod["partName"]
    pod_guid = pod["PartGuid"]["Guid"]
    guid = str(uuid.uuid5(uuid.NAMESPACE_URL, "ose-parity/craft-fairing"))
    modules = copy.deepcopy(definition["serializedPartModules"])
    for m in modules:
        for md in m["ModuleData"]:
            if md["Name"] == "Data_Fairing":
                o = md["DataObject"]
                o["CrossSections"] = [section(h, r, last=k == len(walls) - 1) for k, (h, r) in enumerate(walls)]
                o["IsCapped"] = capped == "1"
                o["DeployType"] = {"ContextKey": "DeployType", "storedValue": "Clamshellx2"}
    state = copy.deepcopy(pod["partState"])
    state["attachNodeStates"] = [node("top"), node("bottom", pod_guid, "top")]
    state["resources"] = {}
    state["localPosition"] = {"x": 0.0, "y": POD_TOP_Y - bottom_y, "z": 0.0}
    state["originalAttachLocalPosition"] = copy.deepcopy(state["localPosition"])
    state["StageIndex"] = -1
    fairing = {
        "partGuid": None,
        "PartGuid": {"Guid": guid, "DebugName": None},
        "partName": definition["partName"],
        "partState": state,
        "PartModulesState": modules,
        "ModulesState": None,
        "LegacyModulesState": None,
    }
    for n in pod["partState"]["attachNodeStates"]:
        if n["nodeId"] == "top":
            n["AttachedPartGuid"] = {"Guid": guid, "DebugName": None}
            n["attachedPartNodeID"] = "bottom"
    asm["parts"] = [pod, fairing]
    meta = doc["Metadata"]
    meta["Name"] = meta["VehicleName"] = name
    meta["Parts"] = 2
    meta["Mass"] = round(float(meta.get("Mass", 0.0)) + float(definition["mass"]), 4)
    json.dump(doc, open(out, "w", encoding="utf-8"), indent=None)
    print(f"wrote {out}: pod {pod_guid[:8]} under fairing {guid[:8]}")


if __name__ == "__main__":
    main(*sys.argv[1:])
