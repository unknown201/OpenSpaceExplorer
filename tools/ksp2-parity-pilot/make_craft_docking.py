"""Build craft-docking.json for the parity pilot's "docking" scenario (T18).

Two halves joined port to port in the VAB, cut from KSP2's own "Stock KSS Hub" workspace so every part keeps
the module state KSP2 wrote for it:

- the active half: the Hub's top (cupola, reaction wheels, batteries, monopropellant, eight RCS blocks, the
  2.5-1.25 m adapter) ending in a Clamp-O-Tron (dockingport_1v_inline) facing down;
- the passive half: a Clamp-O-Tron facing up, a 2.5 m battery and a 2.5 m monopropellant tank, no command part.

In flight the pilot undocks them (KSP2 decouples VAB-attached ports with the port's ejection force), backs the
active half off and flies it back in.

    python tools/ksp2-parity-pilot/make_craft_docking.py <KSS Hub workspace json> <out json>
"""

import copy
import json
import sys
import uuid

ZERO = "00000000-0000-0000-0000-000000000000"
UP = {"x": 0.0, "y": 0.0, "z": 0.0, "w": 1.0}
# half a turn about X, as KSP2 writes a flipped stack part
FLIP_LOCAL = {"x": -1.0, "y": 0.0, "z": 0.0, "w": 0.0}
FLIP_USER = {"x": 1.0, "y": 0.0, "z": 0.0, "w": 0.0}

# kept from the Hub, by GUID prefix
KEEP = ["5255fc", "e79da5", "cbd508", "2b4b64", "0f7885", "d1b948", "216d25", "008f23", "a5ea66"]
RCS_ON = "216d25"


def guid(g):
    return {"Guid": g, "DebugName": None}


def main(src, out):
    doc = json.load(open(src, encoding="utf-8"))
    asm = doc["Assemblies"][0]["Assembly"]
    parts = asm["parts"]
    by_prefix = {p["PartGuid"]["Guid"][:6]: p for p in parts}
    keep = [by_prefix[k] for k in KEEP]
    keep += [
        p
        for p in parts
        if p["partName"] == "rcs_0v_block_monoprop"
        and any(
            n["AttachedPartGuid"]["Guid"].startswith(RCS_ON) for n in p["partState"]["attachNodeStates"]
        )
    ]
    kept = {p["PartGuid"]["Guid"] for p in keep}
    template = {p["partName"]: p for p in parts}

    def new_part(name, y, flipped, src_part=None):
        p = copy.deepcopy(src_part or template[name])
        g = str(uuid.uuid4())
        p["PartGuid"] = guid(g)
        s = p["partState"]
        s["localPosition"] = {"x": 0.0, "y": y, "z": 0.0}
        s["localRotation"] = dict(FLIP_LOCAL if flipped else UP)
        s["UserRotation"] = dict(FLIP_USER if flipped else UP)
        for n in s["attachNodeStates"]:
            n["AttachedPartGuid"] = guid(ZERO)
            n["attachedPartNodeID"] = ""
        return p

    adapter = by_prefix["008f23"]
    # node heights from the part data: adapter top 0.764 (flipped: down), port bottom 0.019 / top 0.287,
    # battery +-0.129, tank +-0.5
    port_a = new_part("dockingport_1v_inline", -3.91 - 0.764 + 0.019, True)
    port_b = new_part("dockingport_1v_inline", -3.91 - 0.764 + 0.019 - 0.287 - 0.287, False)
    bat_b = new_part("storage_2v_battery_electricity", port_b["partState"]["localPosition"]["y"] + 0.019 - 0.129, False, by_prefix["cbd508"])
    tank_b = new_part("fueltank_2v_inline_monoprop", bat_b["partState"]["localPosition"]["y"] - 0.129 - 0.5, False, by_prefix["d1b948"])

    def link(a, na, b, nb):
        for p, n, q, m in ((a, na, b, nb), (b, nb, a, na)):
            for s in p["partState"]["attachNodeStates"]:
                if s["nodeId"] == n:
                    s["AttachedPartGuid"] = guid(q["PartGuid"]["Guid"])
                    s["attachedPartNodeID"] = m
                    break
            else:
                raise SystemExit(f"{p['partName']} has no node {n}")

    # the adapter's connector went: free its top node, then hang the ports under it
    for s in adapter["partState"]["attachNodeStates"]:
        if s["AttachedPartGuid"]["Guid"] not in kept:
            s["AttachedPartGuid"] = guid(ZERO)
            s["attachedPartNodeID"] = ""
    link(adapter, "top", port_a, "bottom")
    link(port_a, "top", port_b, "top")
    link(port_b, "bottom", bat_b, "top")
    link(bat_b, "bottom", tank_b, "top")

    asm["parts"] = keep + [port_a, port_b, bat_b, tank_b]
    asm["SymmetrySets"] = []
    asm["AssemblyOABConfig"]["SymmetrySets"] = []
    asm["AssemblyDefinition"]["assemblyName"] = "DockingPair"
    doc["Metadata"]["Name"] = "DockingPair"
    doc["Metadata"]["VehicleName"] = "DockingPair"
    doc["Metadata"]["Parts"] = len(asm["parts"])
    json.dump(doc, open(out, "w", encoding="utf-8"), indent=1)
    print(out, len(asm["parts"]), "parts")


if __name__ == "__main__":
    main(*sys.argv[1:3])
