"""Export the Vehicle Assembly Building interior (`OAB.prefab`, `oab_assets_all.bundle`) to web assets.

The original's VAB is the "Env-Terrestrial" environment of the Object Assembly Builder: the hall (walls,
supports, catwalks, windows), parked trucks and props, with the craft built around `BuildAreaCenter`. This
writes, under `assets/web/` (gitignored, pushed with tools/push_assets.py):

    vab/vab.glb       every active MeshRenderer under the environment, nodes sharing meshes, base colour
                      textures under textures/; light-point props, sun rays and the shadow blocker left out
    vab/scene.json    the build area centre (the camera's orbit target), the floor height under it, the
                      original's initial camera distance, the hall light

Read-only on the install. Frames as the other exports (glTF, Unity X negated, triangles reversed); nodes
with a mirroring scale keep it, and the client flips their winding.

    .venv/Scripts/python tools/export_vab.py
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_menu_scene import decompose, gltf_dir, load_scene, to_gltf_trs, transform_dir  # noqa: E402
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter, Glb, to_gltf_pos  # noqa: E402

BUNDLE = "oab_assets_all.bundle"
SKIP_NAMES = ("Exterior", "SunRays", "LightShafts", "ShadowCaster", "CUTSCENES", "OAB_PostProcessing")
SKIP_MESH_SUBSTRINGS = ("vab_light_point", "SunRay", "Cube01")
SKIP_MATERIAL_SUBSTRINGS = ("shadow", "light_blocker", "Glass")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default=str(ROOT / "assets" / "web"))
    ap.add_argument("--max-texture", type=int, default=1024)
    a = ap.parse_args()
    out = Path(a.out)
    (out / "vab").mkdir(parents=True, exist_ok=True)
    t0 = time.time()
    ex = Exporter(Path(a.install), out, a.max_texture, verbose=False)
    env, _main = ex.env_for_key("OAB.prefab")
    if env is None:
        sys.exit("OAB.prefab not found in the catalog")
    roots = []
    for f in env.files.values():
        for sf in getattr(f, "files", {}).values() or []:
            if hasattr(sf, "objects") and sf.objects and any(o.type.name == "GameObject" for o in sf.objects.values()):
                try:
                    roots += load_scene(sf)
                except Exception as e:  # noqa: BLE001
                    print("  skip file:", e)
    envroot = next((r for r in roots if r.name == "Env-Terrestrial"), None)
    if envroot is None:
        sys.exit("Env-Terrestrial not found")
    print(f"loaded in {time.time() - t0:.0f} s")

    glb = Glb()
    materials: dict[int, int | None] = {}
    meshes: dict[int, int | None] = {}
    mesh_material: dict[tuple[int, int], int] = {}

    def material_index(mp):
        key = mp.path_id
        if key in materials:
            return materials[key]
        try:
            m = mp.read()
        except Exception:  # noqa: BLE001
            materials[key] = None
            return None
        if any(s.lower() in m.m_Name.lower() for s in SKIP_MATERIAL_SUBSTRINGS):
            materials[key] = -1
            return -1
        textures = {}
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
        idx = glb.material(m.m_Name, textures)
        c = dict(m.m_SavedProperties.m_Colors).get("_Color")
        if c is not None and (c.r, c.g, c.b) != (1.0, 1.0, 1.0):
            glb.materials[idx]["pbrMetallicRoughness"]["baseColorFactor"] = [c.r, c.g, c.b, 1.0]
        materials[key] = idx
        return idx

    def mesh_index(mp):
        key = mp.path_id
        if key in meshes:
            return meshes[key]
        try:
            mesh = mp.read()
            if any(s in mesh.m_Name for s in SKIP_MESH_SUBSTRINGS):
                meshes[key] = None
                return None
            pos, nrm, uv, tris = ex.mesh_arrays(mesh)
        except Exception:  # noqa: BLE001
            meshes[key] = None
            return None
        if not pos or not tris:
            meshes[key] = None
            return None
        meshes[key] = glb.mesh(mesh.m_Name, pos, nrm, uv, tris, None)
        return meshes[key]

    def with_material(mi, mat):
        if mat is None:
            return mi
        k = (mi, mat)
        if k in mesh_material:
            return mesh_material[k]
        base = glb.meshes[mi]
        if "material" not in base["primitives"][0]:
            base["primitives"][0]["material"] = mat
            mesh_material[k] = mi
            return mi
        prim = dict(base["primitives"][0])
        prim["material"] = mat
        glb.meshes.append({"name": base["name"], "primitives": [prim]})
        mesh_material[k] = len(glb.meshes) - 1
        return mesh_material[k]

    kept = 0
    floor_y = None
    center = None
    stack = [(envroot, True)]
    while stack:
        n, parent_active = stack.pop()
        active = parent_active and n.active and n.name not in SKIP_NAMES
        if n.name == "BuildAreaCenter":
            center = n.world
        if not active:
            continue
        for c in n.children:
            stack.append((c, True))
        mr = mf = None
        for cc in n.components():
            if cc.type.name == "MeshRenderer":
                mr = cc
            elif cc.type.name == "MeshFilter":
                mf = cc
        if mr is None or mf is None:
            continue
        mfr = mf.read()
        if not mfr.m_Mesh or not mfr.m_Mesh.path_id:
            continue
        mi = mesh_index(mfr.m_Mesh)
        if mi is None:
            continue
        mrr = mr.read()
        mats = [material_index(mp) for mp in mrr.m_Materials]
        if mats and all(m == -1 for m in mats):
            continue
        mat = next((m for m in mats if m is not None and m >= 0), None)
        trs = to_gltf_trs(n.world)
        # Mirroring scales come out of decompose as positive magnitudes; keep the sign from the determinant.
        w = n.world
        det = (w[0][0] * (w[1][1] * w[2][2] - w[1][2] * w[2][1]) - w[0][1] * (w[1][0] * w[2][2] - w[1][2] * w[2][0]) + w[0][2] * (w[1][0] * w[2][1] - w[1][1] * w[2][0]))
        scale = list(trs["scale"])
        rot = trs["rot"]
        if det < 0:
            # Fold the mirror into the X scale; recompute rotation from the matrix with column 0 negated.
            from export_menu_scene import mat_to_quat
            from export_web_assets import normalize, to_gltf_quat
            s = [((w[0][j] ** 2 + w[1][j] ** 2 + w[2][j] ** 2) ** 0.5) or 1.0 for j in range(3)]
            r = [[w[i][j] / s[j] * (-1.0 if j == 0 else 1.0) for j in range(3)] for i in range(3)]
            rot = list(to_gltf_quat(normalize(mat_to_quat(r))))
            scale = [-s[0], s[1], s[2]]
        glb.roots.append(glb.node(n.name, tuple(trs["pos"]), tuple(rot), tuple(scale), with_material(mi, mat), []))
        kept += 1
    size = glb.write(out / "vab" / "vab.glb", {"source": BUNDLE, "instances": kept})
    scene: dict = {"generated": time.strftime("%Y-%m-%dT%H:%M:%S"), "source": "OAB.prefab", "file": "vab/vab.glb", "instances": kept}
    if center is not None:
        t, _, _ = decompose(center)
        c = list(to_gltf_pos(t))
        scene["build_center"] = c
    scene["camera"] = {"distance": 20.0, "fov_deg": 60.0}
    for r in roots:
        for n in r.walk():
            if n.name == "OAB_DirectionalLight":
                d = transform_dir(n.world, (0, 0, 1))
                scene["light"] = {"to_light": gltf_dir((-d[0], -d[1], -d[2]))}
    (out / "vab" / "scene.json").write_text(json.dumps(scene, indent=1), encoding="utf-8")
    print(f"vab.glb: {kept} instances, {len([m for m in meshes.values() if m is not None])} meshes, {len(glb.materials)} materials, {size / 1e6:.1f} MB; centre {scene.get('build_center')}; {time.time() - t0:.0f} s")
    _ = floor_y
    return 0


if __name__ == "__main__":
    sys.exit(main())
