"""Build craft-reentry.json for the parity pilot's "reentry" scenario: the Mk1 pod over a 1.25 m heat shield.

The pod comes from craft-drop-pod.json (the pilot's own pod-only craft); the heat shield is the part KSP2 wrote in
its Lesson 3.5 tutorial save (`tutorials/Tutorial3.5_Save.json`, the same pod + heat shield + parachute), so it
keeps the state KSP2 gives it (its 0.2 units of Ablator). The parachute is left off: the scenario wants the
shield's own heating and ablation, then the fall.

    python tools/ksp2-parity-pilot/make_craft_reentry.py <Tutorial3.5_Save.json> <craft-drop-pod.json> <out json>
"""

import copy
import json
import re
import sys

ZERO = "00000000-0000-0000-0000-000000000000"


def lenient(path):
    """KSP2's tutorial saves carry trailing commas."""
    text = open(path, encoding="utf-8-sig").read()
    return json.loads(re.sub(r",(\s*[\]}])", r"\1", text))


def main(save, pod_craft, out):
    tutorial = lenient(save)
    shield = next(p for p in tutorial["Vessels"][0]["parts"] if p["partName"] == "heatshield_1v")
    doc = json.load(open(pod_craft, encoding="utf-8"))
    asm = doc["Assemblies"][0]["Assembly"]
    pod = asm["parts"][0]
    assert pod["partName"] == "pod_1v_conical_crew", pod["partName"]
    pod_guid = pod["PartGuid"]["Guid"]
    shield = copy.deepcopy(shield)
    shield_guid = shield["PartGuid"]["Guid"]
    s = shield["partState"]
    s["PartOwnerGuid"] = copy.deepcopy(pod["partState"]["PartOwnerGuid"])
    s.setdefault("Temperature", "-Infinity")
    s["StageIndex"] = -1
    for node in s["attachNodeStates"]:
        if node["nodeId"] == "top":
            node["AttachedPartGuid"] = {"Guid": pod_guid, "DebugName": None}
            node["attachedPartNodeID"] = "bottom"
        else:
            node["AttachedPartGuid"] = {"Guid": ZERO, "DebugName": None}
            node["attachedPartNodeID"] = ""
    for node in pod["partState"]["attachNodeStates"]:
        if node["nodeId"] == "bottom":
            node["AttachedPartGuid"] = {"Guid": shield_guid, "DebugName": None}
            node["attachedPartNodeID"] = "top"
    asm["parts"] = [pod, shield]
    meta = doc["Metadata"]
    meta["Name"] = meta["VehicleName"] = "ReentryPod"
    meta["Parts"] = 2
    meta["Mass"] = round(float(meta.get("Mass", 0.0)) + 0.1 + 0.2 * 0.01, 4)
    json.dump(doc, open(out, "w", encoding="utf-8"), indent=None)
    print(f"wrote {out}: pod {pod_guid[:8]} over heat shield {shield_guid[:8]}")


if __name__ == "__main__":
    main(*sys.argv[1:4])
