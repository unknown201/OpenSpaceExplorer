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
# The campus trees: they looked nothing like KSP2's spaceport.
SKIP_TREE_SUBSTRINGS = ("SM_FLTree",)


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
        # The original's campus lawns (KSP2_ksc_grass) blend grassland maps at three world-space scales and have no
        # plain base map; its lakes (KSP2/Environment/LandBasedWater) are a water shader. The client draws both
        # from these extras (render.rs `surface`).
        try:
            shader_name = m.m_Shader.read().m_ParsedForm.m_Name if m.m_Shader else ""
        except Exception:  # noqa: BLE001
            shader_name = ""
        tex_envs = dict(m.m_SavedProperties.m_TexEnvs)
        surface_extras = None
        # The campus ground around the facilities (KSP2/KSC/ksc_campus_vertexPaint) blends the same grassland maps
        # (as _MediumTexture): drawn like the lawns, or it stands as a flat field with a hard line round them.
        ground_tex = {"KSP2_ksc_grass": "_MediumDiffuse", "KSP2/KSC/ksc_campus_vertexPaint": "_MediumTexture"}
        if shader_name in ground_tex and "baseColor" not in textures:
            te = tex_envs.get(ground_tex[shader_name])
            try:
                tex = te.m_Texture.read() if te and te.m_Texture and te.m_Texture.path_id else None
            except Exception:  # noqa: BLE001
                tex = None
            uri = ex.export_texture_object(tex) if tex is not None else None
            if uri:
                textures["baseColor"] = glb.image_uri("../" + uri, Path(uri).stem)
                texture_count += 1
                # Tile sizes in metres from the campus lawn's mesh: its uv unit spans about 6.4 km, and the
                # material tiles the medium map 75 times, the large 6 times, the small one at _TileRate 1800.
                # Then a linear tint: the grassland map alone is far darker than the lawn KSP2 shows (its
                # shader lifts and grades it); matched to Josh's KSC screenshot.
                surface_extras = {"ose_ground": [85.0, 1070.0, 4.0, 0.9, 1.35, 1.7]}
                # The campus ground blends KSP2's large grassland map (olive with yellow patches, 7 tiles over the
                # 18.9 km mesh) under the medium one, which fades out past a kilometre: carried in the emissive
                # slot, with the tiles (medium 189 m, large 2.7 km, fine 4 m) and KSP2's _GlobalColorize times
                # _GlobalIntensity as the tint (ose_ground_large).
                # The lawns (KSP2_ksc_grass) take the same large map and grade, so they sit in the ground round
                # them instead of standing out as brighter patches, with a touch more green (mown lawn).
                if shader_name in ("KSP2/KSC/ksc_campus_vertexPaint", "KSP2_ksc_grass"):
                    te_l = tex_envs.get("_LargeTexture")
                    try:
                        tex_l = te_l.m_Texture.read() if te_l and te_l.m_Texture and te_l.m_Texture.path_id else None
                    except Exception:  # noqa: BLE001
                        tex_l = None
                    if tex_l is None:
                        tex_l = next(
                            (o.read() for o in objs if o.type.name == "Texture2D" and o.peek_name() == "Kerbin_Grassland_Large_D"),
                            None,
                        )
                    uri_l = ex.export_texture_object(tex_l) if tex_l is not None else None
                    if uri_l:
                        textures["emissive"] = glb.image_uri("../" + uri_l, Path(uri_l).stem)
                        texture_count += 1
                        # The campus ground's _GlobalColorize x _GlobalIntensity (0.745, 0.811, 0.689) x 1.6.
                        tint = [0.745 * 1.6, 0.811 * 1.6, 0.689 * 1.6]
                        # Through our outdoor tone curve KSP2's grade reads too yellow and bright: scaled to the
                        # ground in Josh's KSC screenshot (109,131,89 sRGB).
                        tint = [tint[0] * 0.40, tint[1] * 0.43, tint[2] * 0.81]
                        if shader_name == "KSP2_ksc_grass":
                            tint = [tint[0] * 0.97, tint[1] * 1.06, tint[2] * 0.97]
                        surface_extras = {"ose_ground_large": [189.0, 2700.0, 4.0, *tint]}
        elif shader_name == "KSP2/Environment/LandBasedWater":
            cols = dict(m.m_SavedProperties.m_Colors)
            floats = dict(m.m_SavedProperties.m_Floats)
            sky = cols.get("_SkyColor")
            surface_extras = {
                "ose_water": [
                    sky.r if sky else 0.52,
                    sky.g if sky else 0.72,
                    sky.b if sky else 1.0,
                    float(floats.get("_FresnelPower", 5.0)),
                ]
            }
        idx = glb.material(m.m_Name, textures)
        # The original scales its metallic map by _Metallic (1 on the launch tower's steel, 0 on the concrete
        # platform); the client multiplies the map by metallicFactor.
        metallic = dict(m.m_SavedProperties.m_Floats).get("_Metallic")
        if metallic is not None:
            glb.materials[idx]["pbrMetallicRoughness"]["metallicFactor"] = float(metallic)
        if surface_extras:
            glb.materials[idx].setdefault("extras", {}).update(surface_extras)
            if "ose_water" in surface_extras:
                dc = dict(m.m_SavedProperties.m_Colors).get("_DiffuseColor")
                if dc is not None:
                    glb.materials[idx]["pbrMetallicRoughness"]["baseColorFactor"] = [dc.r, dc.g, dc.b, 1.0]
                materials[key] = idx
                return idx
            materials[key] = idx
            return idx
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
                # The campus terrain is vertex-painted grass with dirt in the original (paths are separate
                # decals); without the vertex colours it reads as the grass.
                flat = [0.34, 0.43, 0.22]
            if flat:
                glb.materials[idx]["pbrMetallicRoughness"]["baseColorFactor"] = flat + [1.0]
        materials[key] = idx
        return idx

    lake_meshes: list[tuple] = []

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
        if mesh.m_Name.startswith("ksc_lake") and uvs:
            lake_meshes.append((positions, uvs, tris))
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
        if any(s in mesh_name or s in n.name for s in SKIP_TREE_SUBSTRINGS):
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
    # The lakes' shores: KSP2 blends its LandBasedWater into the bank by depth; our lake meshes carry no depth, so a
    # mask baked in their uv space gives each texel its distance to the shore (0 at the edge, 1 at SHORE_M in),
    # carried in the paint-mask slot (the water shader reads its red channel).
    lake_mat = next((i for i, m in enumerate(glb.materials) if m.get("name") == "ksc_lake_mat"), None)
    if lake_meshes and lake_mat is not None:
        import numpy as np
        from PIL import Image, ImageDraw
        from scipy.ndimage import distance_transform_edt

        size_px, shore_m = 2048, 140.0
        inside = Image.new("L", (size_px, size_px), 0)
        draw = ImageDraw.Draw(inside)
        m_per_uv = []
        for positions, uvs, tris in lake_meshes:
            pts = np.asarray(positions, float)
            uv = np.asarray(uvs, float)
            # Metres per uv unit along the mesh's long axis (the lakes' nodes scale them 100x).
            ext_m = (pts.max(0) - pts.min(0)).max() * 100.0
            ext_uv = (uv.max(0) - uv.min(0)).max()
            m_per_uv.append(ext_m / max(ext_uv, 1e-6))
            for k in range(0, len(tris), 3):
                tri = [(uv[t][0] * size_px, uv[t][1] * size_px) for t in tris[k : k + 3]]
                draw.polygon(tri, fill=255)
        dist_px = distance_transform_edt(np.asarray(inside) > 0)
        m_per_px = float(np.mean(m_per_uv)) / size_px
        d = np.clip(dist_px * m_per_px / shore_m, 0.0, 1.0)
        img = (d * 255).astype(np.uint8)
        rel = "textures/ksc_lake_shore_mask.png"
        Image.fromarray(np.stack([img, img, img], -1), "RGB").save(out / rel)
        ti = glb.image_uri("../" + rel, "ksc_lake_shore_mask")
        glb.materials[lake_mat].setdefault("extras", {})["ose_paint_mask"] = {"index": ti}
        texture_count += 1
        print(f"lake shore mask: {len(lake_meshes)} lakes, {m_per_px:.1f} m a texel")
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
