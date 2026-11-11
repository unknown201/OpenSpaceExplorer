"""Build stock-part ISS replica payloads and launch vehicles for rendezvous validation.

Uses authored attachment nodes and sizes. The silhouette/module topology follows NASA's
ISS assembly guide; these are Kerbin-compatible replicas, not engineering-scale ISS hardware.
Run before `cargo run -p ose-sim --example iss_assembly -- <output-directory>`.
"""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFS = {p["id"]: p for p in json.loads((ROOT / "assets/own/parts-catalog.json").read_text(encoding="utf-8"))}
RESOURCES = {p["id"]: p for p in json.loads((ROOT / "assets/own/resources.json").read_text(encoding="utf-8"))}


def filled_resources(spec):
    result = []
    for container in spec.get("resource_containers", []):
        recipe = RESOURCES[container["resource"]]["kind"].get("Recipe")
        ingredients = recipe["ingredients"] if recipe else [dict(resource=container["resource"], units_per_recipe_unit=1)]
        for ingredient in ingredients:
            units = container["capacity_units"] * ingredient["units_per_recipe_unit"]
            result.append(dict(resource=ingredient["resource"], stored_units=units, capacity_units=units))
    return result


def vec(x=0.0, y=0.0, z=0.0):
    return dict(x=x, y=y, z=z)


def rotate(q, p):
    x, y, z, w = (q[k] for k in ("x", "y", "z", "w"))
    a, b, c = (p[k] for k in ("x", "y", "z"))
    tx, ty, tz = 2 * (y*c-z*b), 2 * (z*a-x*c), 2 * (x*b-y*a)
    return vec(a+w*tx+y*tz-z*ty, b+w*ty+z*tx-x*tz, c+w*tz+x*ty-y*tx)


def turn(axis=(0, 0, 1), degrees=0):
    s = math.sin(math.radians(degrees)/2)
    return dict(x=axis[0]*s, y=axis[1]*s, z=axis[2]*s, w=math.cos(math.radians(degrees)/2))


class Builder:
    def __init__(self, name):
        self.name = name
        self.parts = []

    def add(self, part_id, position=None, rotation=None, parent=None, own=None, node=None, tag=None):
        spec = DEFS[part_id]
        p = dict(guid=f"iss-{self.name.lower().replace(' ', '-')}-{tag or len(self.parts)}", part_id=part_id,
                 parent=parent, own_node=own, parent_node=node, local_position=position or vec(),
                 local_rotation=rotation or turn(), stage_index=-1, fuel_crossfeed=True,
                 resources=filled_resources(spec))
        self.parts.append(p)
        return len(self.parts)-1

    def attach(self, part_id, parent, parent_node="top", own_node="bottom", rotation=None, tag=None):
        other = self.parts[parent]
        pn = next(n for n in DEFS[other["part_id"]]["attach_nodes"] if n["id"] == parent_node)
        on = next(n for n in DEFS[part_id]["attach_nodes"] if n["id"] == own_node)
        q = rotation or turn()
        a, b = rotate(other["local_rotation"], pn["position"]), rotate(q, on["position"])
        position = {k: other["local_position"][k]+a[k]-b[k] for k in vec()}
        return self.add(part_id, position, q, parent, own_node, parent_node, tag)

    def surface(self, part_id, xyz, parent=0, rotation=None, tag=None):
        return self.add(part_id, vec(*xyz), rotation, parent, "srfAttach", None, tag)

    def craft(self, stages):
        return dict(name=f"ISS {self.name}", description="ISS replica assembly validation; ordinary stock parts and flight controls.",
                    colors=[[220, 224, 230], [48, 70, 102]], loose=[], parts=self.parts, stages=stages)


def module(name, lab=False, hub=False, truss=False):
    b = Builder(name)
    root = b.add("truss_2v_square_1x1" if truss else "lab_2v_science_orbital" if lab else "cabin_2v_crew_hitchhiker", tag="body")
    probe = b.attach("probe_2v_disk_electricity", root, tag="control")
    end = probe
    if truss:
        for section in range(4):
            end = b.attach("truss_1v_square_1x4", end, tag=f"beam-{section}")
            if section in [1, 3]:
                y = b.parts[end]["local_position"]["y"]
                for bank in [-1, 1]:
                    b.surface("solararray_3v_large", (bank*.8, y, 0), parent=end,
                              rotation=turn((0,1,0),90*bank),tag=f"array-{section}-{bank}")
    front = b.attach("dockingport_2v_inline", end, tag="forward")
    tank = b.attach("fueltank_2v_inline_monoprop", root, "bottom", "top", tag="service-tank")
    support = tank
    if name == "Truss Centre":
        # Raise the integrated truss clear of the axial modules and their service engines.
        for i in range(1):
            support = b.attach("truss_1v_square_1x4", support, "bottom", "top", tag=f"support-{i}")
    rear = b.attach("dockingport_2v_inline", support, "bottom", "bottom", turn(degrees=180), tag="aft")
    b.surface("stabilizer_2v_reactionwheel", (0, .0, 0), tag="wheel")
    b.surface("storage_2v_battery_electricity", (0, -.4, 0), tag="battery")
    for i, xyz in enumerate([(1.45, .8, 0), (-1.45, .8, 0), (0, .8, 1.45), (0, .8, -1.45)]):
        b.surface("rcs_0v_block_monoprop", xyz, rotation=turn((0,1,0), [0,180,270,90][i]), tag=f"rcs-{i}")
    for i, x in enumerate([-2.1, 2.1]):
        b.surface("solararray_1v_1x6", (x, .0, 0), rotation=turn((0,1,0), i*180), tag=f"solar-{i}")
    # Radial monopropellant engines leave the aft docking face accessible after the booster separates.
    puffs = [b.surface("engine_0v_monoprop_puff", (x, -2.0, 0), tag=f"tug-engine-{i}") for i,x in enumerate([-2.2,2.2])]
    if hub:
        for tag, xyz, q in [("starboard",(1.5,0,0),turn(degrees=-90)),("port",(-1.5,0,0),turn(degrees=90)),
                            ("zenith",(0,0,1.5),turn((1,0,0),90)),("nadir",(0,0,-1.5),turn((1,0,0),-90))]:
            b.surface("dockingport_2v_inline", xyz, rotation=q, tag=tag)
    payload = b.craft([puffs])
    # Independent copy: module craft stays available for orbital/proximity fixture tests.
    payload = json.loads(json.dumps(payload))
    decoupler = b.attach("decoupler_3v_inline", rear, "top", "top", tag="booster-separator")
    upper = b.attach("fueltank_3v_inline_methalox_1x4", decoupler, "bottom", "top", tag="booster-tank-a")
    lower = b.attach("fueltank_3v_inline_methalox_1x4", upper, "bottom", "top", tag="booster-tank-b")
    lower = b.attach("fueltank_3v_inline_methalox_1x1", lower, "bottom", "top", tag="booster-tank-c")
    engine = b.attach("engine_3v_methalox_mammoth", lower, "bottom", "top", tag="booster-engine")
    launcher = b.craft([[decoupler, *puffs], [engine]])
    return dict(name=name, payload=payload, launcher=launcher, incoming=f"iss-{name.lower().replace(' ', '-')}-aft")


def build():
    modules = [module("Zarya"), module("Unity", hub=True), module("Zvezda"), module("Destiny", lab=True),
               module("Harmony", hub=True), module("Columbus", lab=True), module("Kibo", lab=True),
               module("Truss Centre", hub=True), module("Port Truss", truss=True), module("Starboard Truss", truss=True)]
    # Target GUIDs survive every merge and do not depend on changing part indices.
    links = [("Unity","Zarya","forward"),("Zvezda","Zarya","aft"),("Destiny","Unity","forward"),
             ("Harmony","Destiny","forward"),("Columbus","Harmony","starboard"),
             ("Kibo","Harmony","port"),("Truss Centre","Unity","zenith"),
             ("Port Truss","Truss Centre","port"),("Starboard Truss","Truss Centre","starboard")]
    return dict(source="https://www.nasa.gov/international-space-station/international-space-station-assembly-elements/",
                scope="Stock-part ISS replica: Russian segment, US axial modules, side laboratories, broad integrated truss with eight large solar wings.",
                modules=modules, deliveries=[dict(module=m,target=f"iss-{t.lower().replace(' ', '-')}-{port}") for m,t,port in links])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT/".cache/iss-guidance")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    result = build()
    (args.output/"crafts.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    for m in result["modules"]:
        for kind in ["payload","launcher"]:
            (args.output/f"{m['name'].lower().replace(' ', '-')}-{kind}.json").write_text(json.dumps(m[kind],indent=2),encoding="utf-8")
    print(f"Built {len(result['modules'])} ISS module blueprints and launchers in {args.output}")


if __name__ == "__main__":
    main()
