"""Build craft-sepland.json for the parity pilot's "sepland" scenario: the Mk1 pod over a TD-06 decoupler over a 1x2 tank.

The pod and the tank come from craft-drop-podtank.json (the pilot's own pod-on-tank craft); the decoupler is the one
KSP2 wrote in its Stock Kerbal-K1 assembly (right under its pod, as here), so it keeps its module state (ejection
force). The tank moves down by the decoupler's thickness, and the decoupler is the craft's only stage.

    python tools/ksp2-parity-pilot/make_craft_sepland.py <Stock Kerbal-K1.json> <craft-drop-podtank.json> <out json>
"""

import copy
import json
import sys

ZERO = "00000000-0000-0000-0000-000000000000"


def guid(g):
    return {"Guid": g, "DebugName": None}


def link(part, node, other_guid, other_node):
    for n in part["partState"]["attachNodeStates"]:
        if n["nodeId"] == node:
            n["AttachedPartGuid"] = guid(other_guid)
            n["attachedPartNodeID"] = other_node
            return
    raise KeyError(f"{part['partName']} has no node {node}")


def main(k1_path, podtank_path, out):
    k1 = json.load(open(k1_path, encoding="utf-8-sig"))["Assemblies"][0]["Assembly"]
    decoupler = copy.deepcopy(next(p for p in k1["parts"] if p["partName"] == "decoupler_1v_inline"))
    doc = json.load(open(podtank_path, encoding="utf-8"))
    asm = doc["Assemblies"][0]["Assembly"]
    pod, tank = asm["parts"]
    assert pod["partName"] == "pod_1v_conical_crew" and tank["partName"].startswith("fueltank_1v"), asm["parts"]
    pod_g, dec_g, tank_g = pod["PartGuid"]["Guid"], decoupler["PartGuid"]["Guid"], tank["PartGuid"]["Guid"]
    s = decoupler["partState"]
    s["PartOwnerGuid"] = copy.deepcopy(pod["partState"]["PartOwnerGuid"])
    s.setdefault("Temperature", "-Infinity")
    # the decoupler sits where KSP2's K1 has it under the same pod; its top node meets the pod's bottom node,
    # so it is twice the gap between them thick
    pod_bottom = tank["partState"]["originalAttachLocalPosition"]["y"]
    dec_y = s["localPosition"]["y"]
    thickness = 2.0 * (pod_bottom - dec_y)
    s["StageIndex"] = 0
    tank["partState"]["localPosition"]["y"] -= thickness
    link(pod, "bottom", dec_g, "top")
    link(decoupler, "top", pod_g, "bottom")
    link(decoupler, "bottom", tank_g, "top")
    link(tank, "top", dec_g, "bottom")
    asm["parts"] = [pod, decoupler, tank]
    asm["stagingState"] = {
        "availableStages": [{"partGuids": None, "PartIds": [guid(dec_g)], "StageID": 0, "IsActive": False}],
        "unassignedParts": None,
        "totalStageCount": 1,
    }
    meta = doc["Metadata"]
    meta["Name"] = meta["VehicleName"] = "SepLand"
    meta["Parts"] = 3
    meta["Stages"] = 1
    json.dump(doc, open(out, "w", encoding="utf-8"))
    print(f"wrote {out}: decoupler {thickness:.4f} m thick, tank moved to y {tank['partState']['localPosition']['y']:.4f}")


if __name__ == "__main__":
    main(*sys.argv[1:4])
