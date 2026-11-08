"""Export KSP2's procedural wing rigs (wings, stabilizers, control surfaces, fins) to assets/own/procedural-wings.json.

KSP2 shapes a procedural wing by moving bones of a skinned mesh: `OffsetSliderSystem` sliders (WING_SPAN,
ROOT_LENGTH, ...) add position/scale offsets to corner bones, Unity Position/Scale/Aim constraints place the
helper bones between them, and `SkinMeshToStaticGenerator` bakes the skinned meshes. The part's mass and lift
then come from a drag cube rendered from the baked mesh (`Module_ProceduralPart.SetMeshValues`,
`Module_ControlSurface.GetMeshArea`). `ose_data::procedural` replays all of that; this tool writes what it needs:

    parts.<part id>: density (`BasePartDensity`), mesh options (with/without control surface), per option
        the bone hierarchy (rest local TRS), sliders, constraints and the visual skinned meshes.

Unity space is kept (left-handed, Y up, metres); the evaluator converts. Read-only on the install.

    .venv/Scripts/python tools/export_procedural_wings.py
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_web_assets import DEFAULT_INSTALL, Exporter  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
MODES = ["position", "rotation", "scale", "constraint_pos", "constraint_look", "constraint_scale"]
MEASURE = ["distance", "dist_x", "dist_y", "dist_z", "angle_x", "angle_y", "angle_z"]


def r(x: float, n: int = 5) -> float:
    v = round(float(x), n)
    return 0.0 if v == 0 else v


def v3(o) -> list[float]:
    return [r(o["x"]), r(o["y"]), r(o["z"])]


def curve(c) -> list[list[float]]:
    return [[r(k["time"]), r(k["value"]), r(k["inSlope"]), r(k["outSlope"])] for k in c["m_Curve"]]


def mono_by_script(env, roots: set[int], go_tf, tfs, script: str):
    for o in env.objects:
        if o.type.name != "MonoBehaviour":
            continue
        try:
            mb = o.read()
            name = mb.m_Script.read().m_ClassName if mb.m_Script else ""
        except Exception:
            continue
        if name == script and go_tf.get(mb.m_GameObject.path_id) in roots:
            yield o.read_typetree()


def export_option(ex: Exporter, option: str) -> dict:
    from UnityPy.helpers.MeshHelper import MeshHandler

    env, _ = ex.env_for_key(f"{option}.prefab")
    gos = {o.path_id: o.read().m_Name for o in env.objects if o.type.name == "GameObject"}
    tfs = {o.path_id: o.read() for o in env.objects if o.type.name == "Transform"}
    go_tf = {t.m_GameObject.path_id: pid for pid, t in tfs.items()}
    root = next(pid for pid, t in tfs.items()
                if not (t.m_Father and t.m_Father.path_id in tfs) and gos.get(t.m_GameObject.path_id) == option
                and t.m_Children)
    # The option's own subtree (the environment also holds the source model's copy of the rig).
    order: list[int] = []
    stack = [root]
    while stack:
        cur = stack.pop()
        order.append(cur)
        stack.extend(reversed([c.path_id for c in tfs[cur].m_Children if c.path_id in tfs]))
    index = {pid: i for i, pid in enumerate(order)}
    sub = set(order)
    bones = []
    for pid in order:
        t = tfs[pid]
        father = t.m_Father.path_id if t.m_Father else 0
        q = t.m_LocalRotation
        bones.append({
            "name": gos.get(t.m_GameObject.path_id, ""),
            "parent": index.get(father, -1),
            "p": [r(t.m_LocalPosition.x), r(t.m_LocalPosition.y), r(t.m_LocalPosition.z)],
            "r": [r(q.x, 6), r(q.y, 6), r(q.z, 6), r(q.w, 6)],
            "s": [r(t.m_LocalScale.x), r(t.m_LocalScale.y), r(t.m_LocalScale.z)],
        })

    def bone(pptr) -> int:
        return index.get(pptr["m_PathID"], -1)

    sliders_tt = next(mono_by_script(env, sub, go_tf, tfs, "OffsetSliderSystem"))
    sliders = []
    for s in sliders_tt["Sliders"]:
        targets = [{"mode": MODES[t["mode"]], "bone": bone(t["target"]), "min": v3(t["MinV3"]), "max": v3(t["MaxV3"])}
                   for t in s["targets"] if bone(t["target"]) >= 0]
        m = s["SliderMeasurement"]
        sliders.append({
            "name": s["Name"],
            "label": s.get("DisplayName", ""),
            "group": s.get("GroupDisplayName", ""),
            "start": r(s["startValue"], 6),
            "limits": [{"parent": d["ParentSliderName"], "curve": curve(d["LimitCurve"])} for d in s.get("dynamicLimits", [])],
            "targets": targets,
            "measure": {"mode": MEASURE[m["MeasuringMode"]], "from": bone(m["StartPoint"]), "to": bone(m["EndPoint"])},
        })
    toggles = [{"name": t["Name"], "group": t.get("GroupDisplayName", ""), "value": bool(t["ToggleValue"])}
               for t in sliders_tt.get("Toggles", [])]

    constraints = []
    for o in env.objects:
        kind = o.type.name
        if kind not in ("PositionConstraint", "ScaleConstraint", "AimConstraint", "LookAtConstraint"):
            continue
        tt = o.read_typetree()
        target = index.get(go_tf.get(tt["m_GameObject"]["m_PathID"], 0), -1)
        # Inactive in the prefab, but OffsetSliderSystem.Initialize switches every constraint on.
        if target < 0:
            continue
        c = {"kind": kind.replace("Constraint", "").lower(), "bone": target,
             "sources": [[bone(s["sourceTransform"]), r(s["weight"])] for s in tt["m_Sources"]]}
        if kind == "PositionConstraint":
            c["offset"] = v3(tt["m_TranslationOffset"])
        elif kind == "ScaleConstraint":
            c["offset"] = v3(tt["m_ScaleOffset"])
        else:
            c["rotation_offset"] = v3(tt["m_RotationOffset"])
            if kind == "AimConstraint":
                c["aim"] = v3(tt["m_AimVector"])
                c["up"] = v3(tt["m_UpVector"])
                c["world_up"] = v3(tt["m_WorldUpVector"])
                c["up_object"] = bone(tt["m_WorldUpObject"])
                c["up_type"] = tt["m_UpType"]
        constraints.append(c)

    # Which baked group each skinned mesh belongs to: the control surface hangs from its attach bone.
    groups: dict[int, str] = {}
    attach: dict[str, int] = {}
    for gen in mono_by_script(env, sub, go_tf, tfs, "SkinMeshToStaticGenerator"):
        for g in gen["NamedMeshBonePairs"]:
            attach[g["Name"]] = bone(g["TargetAttachTransform"])
            for m in g["VisualSkinnedMeshes"]:
                groups[m["m_PathID"]] = g["Name"]

    meshes = []
    for o in env.objects:
        if o.type.name != "SkinnedMeshRenderer" or o.path_id not in groups:
            continue
        sk = o.read()
        tf = go_tf.get(sk.m_GameObject.path_id)
        if tf not in sub or not sk.m_Mesh.path_id:
            continue
        mesh = sk.m_Mesh.read()
        h = MeshHandler(mesh)
        h.process()
        joints, weights = h.m_BoneIndices, h.m_BoneWeights
        if weights is None and joints:
            weights = [(1.0,) for _ in joints]
        palette = [index.get(b.path_id, -1) for b in sk.m_Bones]
        # (KSP2 recalculates the normals after baking, so none are kept.)
        tris = [i for s in h.get_triangles() for t in s for i in t]
        # Up to four influences per vertex, as (palette slot, weight), dropping zero weights.
        infl = []
        for j, w in zip(joints, weights):
            row = [[int(a), r(b, 4)] for a, b in zip(j, w) if b > 0]
            infl.append(row)
        mat = ""
        if sk.m_Materials and sk.m_Materials[0].path_id:
            try:
                mat = sk.m_Materials[0].read().m_Name
            except Exception:
                mat = ""
        meshes.append({
            "name": gos.get(sk.m_GameObject.path_id, mesh.m_Name),
            "group": groups[o.path_id],
            "renderer": index[tf],
            "material": mat,
            "palette": palette,
            "bindposes": [[r(x, 6) for x in (m.e00, m.e01, m.e02, m.e03, m.e10, m.e11, m.e12, m.e13,
                                               m.e20, m.e21, m.e22, m.e23)] for m in mesh.m_BindPose],
            "vertices": [r(c, 4) for v in h.m_Vertices for c in tuple(v)[:3]],
            "uvs": [r(c, 4) for v in (h.m_UV0 or []) for c in tuple(v)[:2]],
            "influences": infl,
            "triangles": tris,
        })
    return {"bones": bones, "sliders": sliders, "toggles": toggles, "constraints": constraints,
            "attach": attach, "meshes": meshes}


def part_options(ex: Exporter, part: str) -> tuple[list[dict], dict]:
    env, _ = ex.env_for_key(f"{part}.prefab")
    for o in env.objects:
        if o.type.name != "MonoBehaviour":
            continue
        try:
            mb = o.read()
            if not mb.m_Script or mb.m_Script.read().m_ClassName != "ProcedurallyEditablePart":
                continue
        except Exception:
            continue
        tt = o.read_typetree()
        return [{"name": m["Name"], "attributes": {a["Name"]: a["Value"] for a in m["Attributes"]}}
                for m in tt["_meshOptionPrefabs"]], tt
    return [], {}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default=str(ROOT / "assets/own/procedural-wings.json"))
    ap.add_argument("--parts", nargs="*")
    args = ap.parse_args()
    ex = Exporter(Path(args.install), ROOT / ".cache/procedural-export", 256, False)
    catalog = json.loads((ROOT / "assets/own/parts-catalog.json").read_text(encoding="utf-8"))
    ids = args.parts or [p["id"] for p in catalog if "PartComponentModule_ProceduralPart" in p["module_names"]]
    out = {"parts": {}}
    for part in ids:
        data = json.loads((ROOT / f"assets/ksp2/data/parts/{part}.json").read_text(encoding="utf-8-sig"))["data"]
        mods = {md["Name"]: md["DataObject"] for m in data["serializedPartModules"] for md in m["ModuleData"]}
        pp = mods["Data_ProceduralPart"]
        cs = mods.get("Data_ControlSurface", {})
        options, _ = part_options(ex, part)
        entry = {
            "mass_t": data["mass"],
            "density": pp["BasePartDensity"],
            "type": pp["ProceduralPartTypeName"],
            "mesh_area_lift_multiplier": cs.get("MeshAreaLiftMultiplier", 0.25),
            "ctrl_pivot": cs.get("CtrlSurfacePivotTransformName", ""),
            "ctrl_rot_axis": cs.get("CtrlTransformRotAxis", "X"),
            "ctrl_dir": cs.get("CtrlTransformDir", "Z"),
            "options": [],
        }
        for opt in options:
            rig = export_option(ex, opt["name"])
            rig["name"] = opt["name"]
            rig["attributes"] = opt["attributes"]
            entry["options"].append(rig)
            print(f"{part}: {opt['name']} {len(rig['bones'])} bones, {len(rig['sliders'])} sliders, "
                  f"{len(rig['constraints'])} constraints, {len(rig['meshes'])} meshes", flush=True)
        out["parts"][part] = entry
    Path(args.out).write_text(json.dumps(out, separators=(",", ":")), encoding="utf-8")
    print(f"wrote {args.out} ({Path(args.out).stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
