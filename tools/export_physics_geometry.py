"""Extract engine and RCS thrust points/directions from the installed part prefabs.

Coordinates remain Unity part-local, as in the imported part data. The simulator
performs the same mounted rotation and handedness conversion as for other data.
No meshes or decompiled source are written. Missing transforms are fatal.
"""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

from export_menu_scene import inverse, load_scene, mul, normalize, transform_dir, trs
from export_web_assets import DEFAULT_INSTALL, Exporter, ROOT


def thrust_points(root, mode):
    """Match Module_Engine.SetThrustTransforms/InitThrustTransformMultipliers."""
    authored = mode.get("ThrustTransformNamesMultipliers") or []
    nodes = list(root.walk())
    if authored:
        selected = []
        for item in authored:
            matches = [n for n in nodes if n.name == item["ThrustTransformName"]]
            if not matches:
                raise ValueError(f"Missing {item['ThrustTransformName']}")
            # The flight path uses FindModelTransform, singular, for named weights.
            selected.append((matches[0], float(item["ThrustTransformMultiplier"])))
    else:
        matches = [n for n in nodes if n.name == mode.get("thrustVectorTransformName", "thrustTransform")]
        if not matches:
            raise ValueError("No engine thrust transforms")
        selected = [(n, 1.0 / len(matches)) for n in matches]
    return force_points(root, selected, (0, 0, -1))


def rcs_points(root):
    modules = [module for node in root.walk() if (module := node.behaviour("Module_RCS"))]
    if len(modules) != 1:
        raise ValueError("Missing or ambiguous RCS module")
    nodes = {node.tf.object_reader.path_id: node for node in root.walk()}
    selected = []
    for ref in modules[0]["ThrusterTransforms"]:
        if ref["m_FileID"] != 0 or ref["m_PathID"] not in nodes:
            raise ValueError("Missing RCS transform reference")
        selected.append((nodes[ref["m_PathID"]], 1.0))
    if not selected:
        raise ValueError("No RCS nozzle transforms")
    return force_points(root, selected, (0, -1, 0))


def force_points(root, selected, force_axis):
    scale = tuple(getattr(root.tf.m_LocalScale, c) for c in "xyz")
    # Instantiate replaces root position/rotation; prefab scale is retained.
    relative = mul(trs((0, 0, 0), (0, 0, 0, 1), scale), inverse(root.world))
    rotations = {}

    def rotation_tree(node, rotation):
        rotations[id(node)] = rotation
        for child in node.children:
            q = normalize(tuple(getattr(child.tf.m_LocalRotation, c) for c in "xyzw"))
            rotation_tree(child, mul(rotation, trs((0, 0, 0), q, (1, 1, 1))))

    rotation_tree(root, trs((0, 0, 0), (0, 0, 0, 1), (1, 1, 1)))
    out = []
    for node, weight in selected:
        matrix = mul(relative, node.world)
        # Unity Transform.forward uses rotation, independent of hierarchy scales.
        direction = transform_dir(rotations[id(node)], force_axis)
        length = math.sqrt(sum(x * x for x in direction))
        if length <= 0 or not math.isfinite(weight) or weight < 0:
            raise ValueError("Invalid thrust transform")
        clean = lambda x: 0.0 if abs(x) < 1e-12 else float(x)
        vector = lambda v: dict(zip("xyz", (clean(x) for x in v)))
        out.append({"position_m": vector(matrix[i][3] for i in range(3)),
                    "direction": vector(x / length for x in direction), "weight": weight})
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--install", type=Path, default=Path(DEFAULT_INSTALL))
    parser.add_argument("--data", type=Path, default=ROOT / "assets/ksp2/data/parts")
    parser.add_argument("--out", type=Path, default=ROOT / "assets/own/physics-geometry.json")
    parser.add_argument("--rcs-out", type=Path, default=ROOT / "assets/own/rcs-geometry.json")
    args = parser.parse_args()
    exporter = Exporter(args.install, ROOT / "target/physics-geometry-export", 64, False)
    result = {}
    rcs = {}
    for path in sorted(args.data.glob("*.json")):
        data = json.loads(path.read_text(encoding="utf-8"))["data"]
        modes = [mode for module in data.get("serializedPartModules", [])
                 for entry in module.get("ModuleData", []) if entry["Name"] == "Data_Engine"
                 for mode in entry["DataObject"]["engineModes"]]
        has_rcs = any(entry["Name"] == "Data_RCS" for module in data.get("serializedPartModules", [])
                      for entry in module.get("ModuleData", []))
        if not modes and not has_rcs:
            continue
        env, _ = exporter.env_for_key(path.stem + ".prefab")
        roots = [root for sf in env.assets for root in load_scene(sf) if root.name == path.stem]
        candidates = []
        for root in roots:
            try:
                candidates.append([thrust_points(root, mode) for mode in modes])
            except ValueError:
                pass  # Serialized files also contain model-only roots with the same name.
        if modes and (not candidates or any(c != candidates[0] for c in candidates)):
            raise ValueError(f"Missing or ambiguous engine geometry: {path.stem}")
        if modes:
            result[path.stem] = candidates[0]
            print(path.stem, [len(points) for points in candidates[0]], flush=True)
        if has_rcs:
            candidates = []
            for root in roots:
                try:
                    candidates.append(rcs_points(root))
                except ValueError:
                    pass
            if not candidates or any(c != candidates[0] for c in candidates):
                raise ValueError(f"Missing or ambiguous RCS geometry: {path.stem}")
            rcs[path.stem] = candidates[0]
            print(path.stem, "RCS", len(candidates[0]), flush=True)
        exporter._envs.clear()
    if not result:
        raise ValueError(f"No engine definitions under {args.data}")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote {len(result)} engine parts to {args.out}")
    args.rcs_out.parent.mkdir(parents=True, exist_ok=True)
    args.rcs_out.write_text(json.dumps(rcs, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote {len(rcs)} RCS parts to {args.rcs_out}")


if __name__ == "__main__":
    main()
