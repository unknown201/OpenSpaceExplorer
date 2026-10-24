"""Export the Kerbal Space Center (`KSC.prefab`, `kerbin-ksc_assets_all.bundle`) to web assets, read-only.

The prefab is the whole campus: buildings, launch pads, runways, roads, lights, props, about 7000 LOD0
renderers over 229 meshes. This writes, under `assets/web/` (gitignored, pushed with tools/push_assets.py):

    ksc/ksc.glb        one glTF with every kept renderer as a node that shares its mesh (instances, not
                       copies), PBR materials with base colour textures under textures/, LOD0 only, tiny
                       props (runway and street lights, signs, tyre marks) left out by name
    ksc/scene.json     the camera origin and look-at the original uses, the sun, the ground level, and the
                       clickable buildings (`KSCTargetableBuilding`): facility name and world-space box

Frames as the other exports: glTF right-handed Y-up with Unity's X negated and triangles reversed.

    .venv/Scripts/python tools/export_ksc.py
    .venv/Scripts/python tools/export_ksc.py --max-texture 1024 --keep-lights
"""
from __future__ import annotations

import argparse
import re
import json
import math
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_menu_scene import decompose, gltf_dir, load_scene, mul, to_gltf_trs, transform_dir  # noqa: E402
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter, Glb, to_gltf_pos  # noqa: E402

BUNDLE = "kerbin-ksc_assets_all.bundle"
NAV_ENUM = ["Invalid", "AirTraffic", "AstronautComplex", "TrackingCenter", "ResearchCenter", "TrainingCenter", "VAB", "Launchpad_1", "Launchpad_2", "Launchpad_3", "Launchpad_4", "Runway_1", "Runway_2", "Boat_Launch", "Launch_Site_Generic"]
# Small props that add thousands of draws and nothing to a replica seen from the KSC camera.
SKIP_MESH_SUBSTRINGS = ("runway_light", "light_housing", "ksc_light_aviation", "tireMarks", "_Sign", "streetlight", "street_light", "ksc_prop_lights")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default=str(ROOT / "assets" / "web"))
    ap.add_argument("--max-texture", type=int, default=1024)
    ap.add_argument("--keep-lights", action="store_true", help="keep the runway and street light props")
    ap.add_argument("--max-instances", type=int, default=0, help="debug: stop after this many renderers")
    a = ap.parse_args()
    out = Path(a.out)
    (out / "ksc").mkdir(parents=True, exist_ok=True)
    (out / "textures").mkdir(parents=True, exist_ok=True)
    t0 = time.time()
    ex = Exporter(Path(a.install), out, a.max_texture, verbose=False)
    env = ex.env(BUNDLE)
    bundle = next(iter(env.files.values()))
    sf = next(f for f in bundle.files.values() if hasattr(f, "objects") and f.objects)
    objs = list(sf.objects.values())
    print(f"bundle loaded in {time.time() - t0:.0f} s: {len(objs)} objects")

    # LOD0 renderer set (renderers in a LODGroup's lower levels are skipped).
    lod0, lod_other = set(), set()
    for o in objs:
        if o.type.name == "LODGroup":
            for i, lod in enumerate(o.read_typetree().get("m_LODs", [])):
                for r in lod.get("renderers", []):
                    (lod0 if i == 0 else lod_other).add(r["renderer"]["m_PathID"])
    roots = load_scene(sf)
    ksc = next(r for r in roots if r.name.startswith("KSC"))
    print(f"scene walked in {time.time() - t0:.0f} s; root {ksc.name}")

    glb = Glb()
    materials: dict[int, int | None] = {}
    meshes: dict[int, int | None] = {}
    mesh_verts: dict[int, int] = {}
    mesh_bounds: dict[int, tuple] = {}
    texture_count = 0

    def material_index(mp) -> int | None:
        nonlocal texture_count
        key = mp.path_id
        if key in materials:
            return materials[key]
        try:
            m = mp.read()
        except Exception:  # noqa: BLE001
            materials[key] = None
            return None
        textures: dict[str, int] = {}
        for name, te in m.m_SavedProperties.m_TexEnvs:
            slot = Exporter.MATERIAL_SLOTS.get(name)
            if not slot or not te.m_Texture or not te.m_Texture.path_id:
                continue
            try:
                tex = te.m_Texture.read()
            except Exception:  # noqa: BLE001
                continue
            if tex.__class__.__name__ != "Texture2D":
                continue
            uri = ex.export_texture_object(tex)
            if uri:
                textures[slot] = glb.image_uri("../" + uri, Path(uri).stem)
                texture_count += 1
        idx = glb.material(m.m_Name, textures)
        colors = dict(m.m_SavedProperties.m_Colors)
        c = colors.get("_Color")
        if c is not None and (c.r, c.g, c.b) != (1.0, 1.0, 1.0):
            glb.materials[idx]["pbrMetallicRoughness"]["baseColorFactor"] = [c.r, c.g, c.b, 1.0]
        if "baseColor" not in textures:
            # Terrain-style materials (grass, lake, campus ground) have no plain base map; give them the
            # colours the original's ground reads as, so they are not white.
            low = m.m_Name.lower()
            flat = None
            if "grass" in low:
                flat = [0.30, 0.42, 0.20]
            elif "lake" in low or "water" in low:
                flat = [0.10, 0.28, 0.45]
            elif "campus_mat" in low or "ground" in low:
                flat = [0.42, 0.40, 0.33]
            if flat:
                glb.materials[idx]["pbrMetallicRoughness"]["baseColorFactor"] = flat + [1.0]
        materials[key] = idx
        return idx

    def mesh_index(mesh_pptr) -> int | None:
        key = mesh_pptr.path_id
        if key in meshes:
            return meshes[key]
        try:
            mesh = mesh_pptr.read()
            positions, normals, uvs, tris = ex.mesh_arrays(mesh)
        except Exception:  # noqa: BLE001
            meshes[key] = None
            return None
        if not positions or not tris:
            meshes[key] = None
            return None
        mi = glb.mesh(mesh.m_Name, positions, normals, uvs, tris, None)
        mesh_verts[key] = len(positions)
        mesh_bounds[key] = (
            tuple(min(p[i] for p in positions) for i in range(3)),
            tuple(max(p[i] for p in positions) for i in range(3)),
        )
        meshes[key] = mi
        return mi

    def active_chain(n) -> bool:
        return n.active

    # Walk every node; keep active ones with a LOD0 (or non-LOD) MeshRenderer whose mesh is not a skipped prop.
    nodes_out: list[int] = []
    kept = skipped_lod = skipped_prop = skipped_missing = 0
    instance_verts = 0
    camera_origin = camera_lookat = None
    spawns: dict = {}
    buildings: dict[str, dict] = {}
    building_nodes: list[tuple[str, object]] = []
    mesh_material: dict[tuple[int, int], int] = {}  # (mesh index, material index) -> glTF mesh with material

    def mesh_with_material(mi: int, mat: int | None) -> int:
        if mat is None:
            return mi
        key = (mi, mat)
        if key in mesh_material:
            return mesh_material[key]
        base = glb.meshes[mi]
        if "material" not in base["primitives"][0]:
            base["primitives"][0]["material"] = mat
            mesh_material[key] = mi
            return mi
        prim = dict(base["primitives"][0])
        prim["material"] = mat
        glb.meshes.append({"name": base["name"], "primitives": [prim]})
        mesh_material[key] = len(glb.meshes) - 1
        return mesh_material[key]

    stack = [(ksc, True)]
    while stack:
        n, parent_active = stack.pop()
        active = parent_active and n.active
        for c in n.children:
            stack.append((c, active))
        if n.name == "KSCCameraOrigin":
            camera_origin = n.world
        elif n.name == "KSCCameraLookAt":
            camera_lookat = n.world
        elif re.fullmatch(r"(LaunchPad|Runway)_spawn_\d\d", n.name):
            spawns[n.name] = n.world
        tb = n.behaviour("KSCTargetableBuilding") if active else None
        if tb:
            enum = int(tb.get("_buildingEnum", 0))
            building_nodes.append((NAV_ENUM[enum] if 0 <= enum < len(NAV_ENUM) else str(enum), n))
        if not active:
            continue
        mr = None
        mf = None
        for cc in n.components():
            if cc.type.name == "MeshRenderer":
                mr = cc
            elif cc.type.name == "MeshFilter":
                mf = cc
        if mr is None or mf is None:
            continue
        if mr.path_id in lod_other and mr.path_id not in lod0:
            skipped_lod += 1
            continue
        mfr = mf.read()
        if not mfr.m_Mesh or not mfr.m_Mesh.path_id:
            continue
        try:
            mesh_name = mfr.m_Mesh.read().m_Name
        except Exception:  # noqa: BLE001
            skipped_missing += 1
            continue
        if not a.keep_lights and any(s in mesh_name for s in SKIP_MESH_SUBSTRINGS):
            skipped_prop += 1
            continue
        mi = mesh_index(mfr.m_Mesh)
        if mi is None:
            skipped_missing += 1
            continue
        mrr = mr.read()
        mat = material_index(mrr.m_Materials[0]) if mrr.m_Materials else None
        gm = mesh_with_material(mi, mat)
        trs = to_gltf_trs(n.world)
        nodes_out.append(glb.node(n.name, tuple(trs["pos"]), tuple(trs["rot"]), tuple(trs["scale"]), gm, []))
        kept += 1
        instance_verts += mesh_verts[mfr.m_Mesh.path_id]
        n.kept_mesh = mfr.m_Mesh.path_id  # for building boxes
        if a.max_instances and kept >= a.max_instances:
            break
    # Ground: a flat square under the campus at the runway level (the real terrain is procedural PQS).
    ground_y = -126.5
    half = 30000.0
    gm = glb.material("ksc_ground", {})
    glb.materials[gm]["pbrMetallicRoughness"]["baseColorFactor"] = [0.30, 0.40, 0.20, 1.0]
    gmesh = glb.mesh("ground", [(-half, ground_y, -half), (half, ground_y, -half), (half, ground_y, half), (-half, ground_y, half)], [(0.0, 1.0, 0.0)] * 4, [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)], [0, 2, 1, 0, 3, 2], gm)
    nodes_out.append(glb.node("ground", (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), gmesh, []))
    glb.roots = nodes_out
    size = glb.write(out / "ksc" / "ksc.glb", {"source": BUNDLE, "instances": kept, "unique_meshes": len([m for m in meshes.values() if m is not None])})
    print(f"ksc.glb: {kept} instances of {len([m for m in meshes.values() if m is not None])} meshes ({instance_verts / 1e6:.1f} M instanced vertices, {sum(mesh_verts.values()) / 1e6:.2f} M unique), {len(glb.materials)} materials, {texture_count} textures, {size / 1e6:.1f} MB; skipped {skipped_lod} lower LODs, {skipped_prop} props, {skipped_missing} missing")

    # Buildings: world-space boxes over every kept mesh under the targetable node (or its target object).
    for name, n in building_nodes:
        lo = [math.inf] * 3
        hi = [-math.inf] * 3
        count = 0
        for d in n.walk():
            key = getattr(d, "kept_mesh", None)
            if key is None or key not in mesh_bounds:
                continue
            bmin, bmax = mesh_bounds[key]
            trs = to_gltf_trs(d.world)
            # Transform the 8 corners of the local box (already in the glTF frame) by the node's world TRS.
            px, py, pz = trs["pos"]
            qx, qy, qz, qw = trs["rot"]
            sx, sy, sz = trs["scale"]
            for cx in (bmin[0], bmax[0]):
                for cy in (bmin[1], bmax[1]):
                    for cz in (bmin[2], bmax[2]):
                        x, y, z = cx * sx, cy * sy, cz * sz
                        # quaternion rotate
                        tx, ty, tz = 2 * (qy * z - qz * y), 2 * (qz * x - qx * z), 2 * (qx * y - qy * x)
                        rx = x + qw * tx + (qy * tz - qz * ty)
                        ry = y + qw * ty + (qz * tx - qx * tz)
                        rz = z + qw * tz + (qx * ty - qy * tx)
                        wx, wy, wz = rx + px, ry + py, rz + pz
                        lo = [min(lo[0], wx), min(lo[1], wy), min(lo[2], wz)]
                        hi = [max(hi[0], wx), max(hi[1], wy), max(hi[2], wz)]
            count += 1
        if count:
            entry = buildings.setdefault(name, {"facility": name, "nodes": [], "min": lo, "max": hi})
            entry["nodes"].append(n.name)
            entry["min"] = [min(x, y) for x, y in zip(entry["min"], lo)]
            entry["max"] = [max(x, y) for x, y in zip(entry["max"], hi)]
    scene: dict = {"generated": time.strftime("%Y-%m-%dT%H:%M:%S"), "source": "KSC.prefab", "file": "ksc/ksc.glb", "instances": kept}
    if camera_origin is not None and camera_lookat is not None:
        t, _, _ = decompose(camera_origin)
        l, _, _ = decompose(camera_lookat)
        scene["camera"] = {"origin": list(to_gltf_pos(t)), "look_at": list(to_gltf_pos(l)), "fov_deg": 60.0, "default_distance": 80.0, "min_distance": 0.625, "max_distance": 40000.0}
    scene["buildings"] = list(buildings.values())
    # Vessel spawn points (where the original puts a craft on each pad and runway), world positions.
    scene["spawns"] = {k: list(to_gltf_pos(decompose(w)[0])) for k, w in sorted(spawns.items())}
    scene["ground_y"] = ground_y
    (out / "ksc" / "scene.json").write_text(json.dumps(scene, indent=1), encoding="utf-8")
    print("camera", scene.get("camera"))
    for b in scene["buildings"]:
        print(f"building {b['facility']}: {b['nodes']} box {[round(v) for v in b['min']]}..{[round(v) for v in b['max']]}")
    idx = out / "index.json"
    if idx.exists():
        index = json.load(open(idx, encoding="utf-8"))
        index["ksc"] = {"file": "ksc/scene.json", "generated": scene["generated"], "instances": kept}
        idx.write_text(json.dumps(index, indent=1), encoding="utf-8")
    print(f"done in {time.time() - t0:.0f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
