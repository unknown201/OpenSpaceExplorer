"""Export KSP2 part and planet visuals from the install's Addressables bundles to web formats.

For every part in assets/own/parts-catalog.json: the part prefab's mesh hierarchy (MeshFilter + MeshRenderer
under each GameObject, with local transforms) becomes one binary glTF, `assets/web/parts/<part id>.glb`, with
a PBR material per renderer pointing at the part's diffuse (`_d`), normal (`_n`), emissive (`_e`) and
metallic-gloss (`_m`) textures, written as PNG under `assets/web/textures/` (downscaled to --max-texture).
For every body in assets/own/kerbol-system.json: the scaled-space sphere mesh `Celestial.<Body>.Scaled_mesh`
and the `<body>_scaled_d` / `_n` textures become `assets/web/bodies/<body id>.glb`.

`assets/web/index.json` lists what was exported. The output is gitignored: it is derived from the user's own
install and is copied to the server with tools/push_assets.py. Read-only on the install.

    .venv/Scripts/python tools/export_web_assets.py                 # everything
    .venv/Scripts/python tools/export_web_assets.py --parts engine_1v_methalox_cornet --bodies Kerbin Mun
    .venv/Scripts/python tools/export_web_assets.py --max-texture 1024 --out assets/web

Unity is left-handed Y-up with X mirrored relative to glTF (right-handed Y-up); positions, normals, node
translations and rotations are converted by negating X (and the quaternion's y and z), and each triangle is
reversed so that front faces are counter-clockwise from outside (the signed volume of every exported mesh is
positive).
"""
from __future__ import annotations

import argparse
import json
import math
import struct
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from ksp2_catalog import Catalog  # noqa: E402

DEFAULT_INSTALL = r"C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2"
ROOT = Path(__file__).resolve().parent.parent

TEXTURE_SUFFIXES = {"_d": "baseColor", "_n": "normal", "_e": "emissive", "_m": "metallicRoughness", "_ao": "occlusion"}


# ---------------------------------------------------------------------------------------------------------------
# glTF writer


class Glb:
    """Accumulates meshes, nodes, materials and images into one GLB (glTF 2.0 binary)."""

    def __init__(self) -> None:
        self.bin = bytearray()
        self.buffer_views: list[dict] = []
        self.accessors: list[dict] = []
        self.meshes: list[dict] = []
        self.nodes: list[dict] = []
        self.materials: list[dict] = []
        self.images: list[dict] = []
        self.textures: list[dict] = []
        self.samplers = [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}]
        self.roots: list[int] = []
        self.skins: list[dict] = []
        self.animations: list[dict] = []

    def _view(self, data: bytes, target: int | None) -> int:
        while len(self.bin) % 4:
            self.bin.append(0)
        view = {"buffer": 0, "byteOffset": len(self.bin), "byteLength": len(data)}
        if target is not None:
            view["target"] = target
        self.bin += data
        self.buffer_views.append(view)
        return len(self.buffer_views) - 1

    def accessor_vec(self, values: list[tuple], ncomp: int, with_minmax: bool = False) -> int:
        fmt = "<" + "f" * ncomp
        data = b"".join(struct.pack(fmt, *v[:ncomp]) for v in values)
        view = self._view(data, 34962)
        acc = {"bufferView": view, "componentType": 5126, "count": len(values), "type": {2: "VEC2", 3: "VEC3", 4: "VEC4"}[ncomp]}
        if with_minmax and values:
            acc["min"] = [min(v[i] for v in values) for i in range(ncomp)]
            acc["max"] = [max(v[i] for v in values) for i in range(ncomp)]
        self.accessors.append(acc)
        return len(self.accessors) - 1

    def accessor_indices(self, indices: list[int]) -> int:
        data = struct.pack(f"<{len(indices)}I", *indices)
        view = self._view(data, 34963)
        self.accessors.append({"bufferView": view, "componentType": 5125, "count": len(indices), "type": "SCALAR"})
        return len(self.accessors) - 1

    def image_png(self, png: bytes, name: str) -> int:
        view = self._view(png, None)
        self.images.append({"bufferView": view, "mimeType": "image/png", "name": name})
        self.textures.append({"sampler": 0, "source": len(self.images) - 1})
        return len(self.textures) - 1

    def image_uri(self, uri: str, name: str) -> int:
        self.images.append({"uri": uri, "name": name})
        self.textures.append({"sampler": 0, "source": len(self.images) - 1})
        return len(self.textures) - 1

    def material(self, name: str, textures: dict[str, int]) -> int:
        pbr: dict = {"baseColorFactor": [1, 1, 1, 1], "metallicFactor": 1.0 if "metallicRoughness" in textures else 0.0, "roughnessFactor": 0.8}
        mat: dict = {"name": name, "pbrMetallicRoughness": pbr, "doubleSided": False}
        if "baseColor" in textures:
            pbr["baseColorTexture"] = {"index": textures["baseColor"]}
        if "metallicRoughness" in textures:
            pbr["metallicRoughnessTexture"] = {"index": textures["metallicRoughness"]}
        if "normal" in textures:
            mat["normalTexture"] = {"index": textures["normal"]}
        if "occlusion" in textures:
            mat["occlusionTexture"] = {"index": textures["occlusion"]}
        if "emissive" in textures:
            mat["emissiveTexture"] = {"index": textures["emissive"]}
            mat["emissiveFactor"] = [1, 1, 1]
        self.materials.append(mat)
        return len(self.materials) - 1

    def mesh(self, name: str, positions, normals, uvs, triangles: list[int], material: int | None) -> int:
        prim: dict = {"attributes": {"POSITION": self.accessor_vec(positions, 3, True)}, "indices": self.accessor_indices(triangles), "mode": 4}
        if normals:
            prim["attributes"]["NORMAL"] = self.accessor_vec(normals, 3)
        if uvs:
            prim["attributes"]["TEXCOORD_0"] = self.accessor_vec(uvs, 2)
        if material is not None:
            prim["material"] = material
        self.meshes.append({"name": name, "primitives": [prim]})
        return len(self.meshes) - 1

    def node(self, name: str, translation, rotation, scale, mesh: int | None, children: list[int]) -> int:
        n: dict = {"name": name}
        if translation != (0.0, 0.0, 0.0):
            n["translation"] = list(translation)
        if rotation != (0.0, 0.0, 0.0, 1.0):
            n["rotation"] = list(rotation)
        if scale != (1.0, 1.0, 1.0):
            n["scale"] = list(scale)
        if mesh is not None:
            n["mesh"] = mesh
        if children:
            n["children"] = children
        self.nodes.append(n)
        return len(self.nodes) - 1

    def write(self, path: Path, extras: dict | None = None) -> int:
        gltf: dict = {
            "asset": {"version": "2.0", "generator": "OpenSpaceExplorer export_web_assets"},
            "scene": 0,
            "scenes": [{"nodes": self.roots}],
            "nodes": self.nodes,
            "meshes": self.meshes,
            "accessors": self.accessors,
            "bufferViews": self.buffer_views,
            "buffers": [{"byteLength": len(self.bin)}],
        }
        if self.materials:
            gltf["materials"] = self.materials
        if self.images:
            gltf["images"] = self.images
            gltf["textures"] = self.textures
            gltf["samplers"] = self.samplers
        if extras:
            gltf["extras"] = extras
        if self.skins:
            gltf["skins"] = self.skins
        if self.animations:
            gltf["animations"] = self.animations
        js = json.dumps(gltf, separators=(",", ":")).encode("utf-8")
        while len(js) % 4:
            js += b" "
        binc = bytes(self.bin)
        while len(binc) % 4:
            binc += b"\0"
        total = 12 + 8 + len(js) + 8 + len(binc)
        with open(path, "wb") as f:
            f.write(struct.pack("<III", 0x46546C67, 2, total))
            f.write(struct.pack("<II", len(js), 0x4E4F534A))
            f.write(js)
            f.write(struct.pack("<II", len(binc), 0x004E4942))
            f.write(binc)
        return total


# ---------------------------------------------------------------------------------------------------------------
# Unity readers


def to_gltf_pos(v) -> tuple[float, float, float]:
    return (-float(v[0]), float(v[1]), float(v[2]))


def to_gltf_quat(q) -> tuple[float, float, float, float]:
    # Mirroring X flips the handedness: negate the y and z components of the rotation.
    return (float(q[0]), -float(q[1]), -float(q[2]), float(q[3]))


def normalize(q):
    n = math.sqrt(sum(c * c for c in q)) or 1.0
    return tuple(c / n for c in q)


class Exporter:
    def __init__(self, install: Path, out: Path, max_texture: int, verbose: bool) -> None:
        import UnityPy  # noqa: F401  (imported lazily so --help works without it)

        self.aa = install / "KSP2_x64_Data/StreamingAssets/aa"
        self.bdir = self.aa / "StandaloneWindows64"
        self.cat = Catalog(str(self.aa / "catalog.json"))
        # Bone name -> world matrix (Unity space) of a reference character skeleton, see tools/export_kerbal.py.
        self.skeleton: dict | None = None
        # World matrix of the bone a worn prop (a helmet prefab) is attached to, with `skeleton`.
        self.attach = None
        self.out = out
        self.max_texture = max_texture
        self.verbose = verbose
        self._envs: dict[str, object] = {}
        self.texture_files: dict[str, str] = {}
        (out / "parts").mkdir(parents=True, exist_ok=True)
        (out / "bodies").mkdir(parents=True, exist_ok=True)
        (out / "textures").mkdir(parents=True, exist_ok=True)

    def env(self, bundle: str):
        import UnityPy

        if bundle not in self._envs:
            self._envs[bundle] = UnityPy.load(str(self.bdir / bundle))
        return self._envs[bundle]

    def bundle(self, key: str) -> str | None:
        return self.cat.bundle_for(key)

    # -- textures

    def export_texture(self, key: str) -> str | None:
        """Export the Texture2D behind an Addressables key as PNG; returns the relative uri or None."""
        if key in self.texture_files:
            return self.texture_files[key]
        bundle = self.bundle(key)
        if not bundle:
            return None
        env = self.env(bundle)
        stem = Path(key).stem
        for o in env.objects:
            if o.type.name != "Texture2D":
                continue
            t = o.read()
            if t.m_Name != stem and len([x for x in env.objects if x.type.name == "Texture2D"]) > 1:
                continue
            try:
                img = t.image
            except Exception as e:  # noqa: BLE001
                print(f"  texture {key}: cannot decode ({e})")
                return None
            if img.width > self.max_texture or img.height > self.max_texture:
                s = self.max_texture / max(img.width, img.height)
                img = img.resize((max(1, int(img.width * s)), max(1, int(img.height * s))))
            if img.mode not in ("RGB", "RGBA"):
                img = img.convert("RGBA")
            rel = f"textures/{stem}.png"
            img.save(self.out / rel, optimize=False)
            self.texture_files[key] = rel
            return rel
        return None

    # -- meshes

    def mesh_arrays(self, mesh_obj, bake=None):
        """Positions, normals, uvs and triangles in glTF space; `bake` (a 4x4 in Unity space) first moves the
        vertices, for skinned meshes whose bind pose differs from their renderer's transform."""
        from UnityPy.helpers.MeshHelper import MeshHandler

        h = MeshHandler(mesh_obj)
        h.process()
        verts = [tuple(v)[:3] for v in h.m_Vertices]
        norms = [tuple(n)[:3] for n in h.m_Normals] if h.m_Normals else []
        if bake is not None:
            import numpy as np

            m = np.asarray(bake, float)
            v = np.c_[np.asarray(verts, float), np.ones(len(verts))] @ m.T
            verts = [tuple(r[:3]) for r in v]
            if norms:
                nm = np.linalg.inv(m[:3, :3]).T
                n = np.asarray(norms, float) @ nm.T
                n /= np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)
                norms = [tuple(r) for r in n]
        positions = [to_gltf_pos(v) for v in verts]
        normals = [to_gltf_pos(n) for n in norms] if norms else []
        uvs = [(float(u[0]), 1.0 - float(u[1])) for u in h.m_UV0] if h.m_UV0 else []
        tris: list[int] = []
        for sub in h.get_triangles():
            for a, b, c in sub:
                # Unity's triangle order comes out clockwise from outside once X is mirrored (checked by the
                # signed volume of the exported spheres and tanks), so each triangle is reversed to give glTF
                # its counter-clockwise front faces.
                tris.extend((a, c, b))
        return positions, normals, uvs, tris

    # -- parts

    def env_for_key(self, key: str):
        """One UnityPy environment holding the key's bundle and every dependency bundle, so cross-bundle
        references (prefab -> mesh in the fbx bundle, material -> textures) resolve."""
        import UnityPy
        from ksp2_catalog import PREFIX

        res = self.cat.resolve(key)
        deps = list(dict.fromkeys(d.replace(PREFIX, "") for r in res for d in r[2] if d.endswith(".bundle")))
        main = self.cat.bundle_for(key)
        if not deps and main:
            deps = [main]
        if not deps:
            return None, None
        tag = "|".join(deps)
        if tag not in self._envs:
            env = UnityPy.Environment()
            for d in deps:
                path = self.bdir / d
                if path.exists():
                    env.load_file(str(path))
            self._envs[tag] = env
            if len(self._envs) > 64:
                # Environments hold decompressed bundles; keep the cache bounded.
                for k in list(self._envs)[:32]:
                    del self._envs[k]
        return self._envs[tag], main

    def export_texture_object(self, tex) -> str | None:
        """Export a Texture2D object as PNG under textures/<name>.png (once); returns the relative uri."""
        name = tex.m_Name
        if name in self.texture_files:
            return self.texture_files[name]
        try:
            img = tex.image
        except Exception as e:  # noqa: BLE001
            if self.verbose:
                print(f"  texture {name}: cannot decode ({e})")
            return None
        if img.width > self.max_texture or img.height > self.max_texture:
            sc = self.max_texture / max(img.width, img.height)
            img = img.resize((max(1, int(img.width * sc)), max(1, int(img.height * sc))))
        if img.mode not in ("RGB", "RGBA"):
            img = img.convert("RGBA")
        rel = f"textures/{name}.png"
        if not (self.out / rel).exists():
            img.save(self.out / rel, optimize=False)
        self.texture_files[name] = rel
        return rel

    MATERIAL_SLOTS = {"_MainTex": "baseColor", "_BumpMap": "normal", "_EmissionMap": "emissive", "_MetallicGlossMap": "metallicRoughness", "_OcclusionMap": "occlusion"}
    SKIP_MATERIALS = ("ReentryShockwave", "Default-Material")

    def export_part(self, part_id: str) -> dict | None:
        env, main = self.env_for_key(f"{part_id}.prefab")
        if env is None:
            return None
        glb = Glb()
        materials: dict[int, int | None] = {}  # material path_id -> glTF material index
        self._excluded_lod_renderers = set()
        for obj in env.objects:
            if obj.type.name == "LODGroup":
                lods = obj.read().m_LODs
                if lods:
                    first = {p.renderer.path_id for p in lods[0].renderers}
                    self._excluded_lod_renderers.update(p.renderer.path_id for lod in lods[1:] for p in lod.renderers if p.renderer.path_id not in first)
        texture_slots_used: set[str] = set()

        def material_index(mat_pptr) -> int | None:
            key = mat_pptr.path_id
            if key in materials:
                return materials[key]
            idx: int | None = None
            try:
                m = mat_pptr.read()
            except Exception:  # noqa: BLE001
                materials[key] = None
                return None
            if m.m_Name in self.SKIP_MATERIALS:
                materials[key] = -1
                return -1
            textures: dict[str, int] = {}
            for name, te in m.m_SavedProperties.m_TexEnvs:
                slot = self.MATERIAL_SLOTS.get(name)
                pp = te.m_Texture
                if not slot or not pp.path_id:
                    continue
                try:
                    tex = pp.read()
                except Exception:  # noqa: BLE001
                    continue
                if tex.__class__.__name__ != "Texture2D":
                    continue
                uri = self.export_texture_object(tex)
                if uri:
                    textures[slot] = glb.image_uri("../" + uri, Path(uri).stem)
                    texture_slots_used.add(slot)
            idx = glb.material(m.m_Name, textures)
            materials[key] = idx
            return idx

        # Transforms of the whole environment, keyed by path id; the prefab's roots are those whose father
        # is missing. Collision ("col_*") and reentry LOD objects are skipped, and engine fairings and exhaust plumes.
        transforms = {o.path_id: o.read() for o in env.objects if o.type.name == "Transform"}
        go_name = {}
        for o in env.objects:
            if o.type.name == "GameObject":
                try:
                    go_name[o.path_id] = o.read().m_Name
                except Exception:  # noqa: BLE001
                    pass
        import numpy as np

        def mat44(m) -> np.ndarray:
            return np.array([[getattr(m, f"e{r}{c}") for c in range(4)] for r in range(4)], float)

        def trs(tf) -> np.ndarray:
            x, y, z, w = (tf.m_LocalRotation.x, tf.m_LocalRotation.y, tf.m_LocalRotation.z, tf.m_LocalRotation.w)
            r = np.array([[1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
                          [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
                          [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]])
            m = np.eye(4)
            m[:3, :3] = r * np.array([tf.m_LocalScale.x, tf.m_LocalScale.y, tf.m_LocalScale.z])
            m[:3, 3] = (tf.m_LocalPosition.x, tf.m_LocalPosition.y, tf.m_LocalPosition.z)
            return m

        def world_of(tf_id: int) -> np.ndarray:
            m = np.eye(4)
            cur, hops = tf_id, 0
            while cur in transforms and hops < 64:
                m = trs(transforms[cur]) @ m
                cur = transforms[cur].m_Father.path_id if transforms[cur].m_Father else 0
                hops += 1
            return m

        meshes_by_tf: dict[int, int] = {}
        renderer_material: dict[int, int | None] = {}  # GameObject path id -> material index
        for o in env.objects:
            if o.type.name not in ("MeshRenderer", "SkinnedMeshRenderer"):
                continue
            if o.path_id in self._excluded_lod_renderers:
                renderer_material[o.read().m_GameObject.path_id] = -1
                continue
            try:
                mr = o.read()
                mats = [material_index(mp) for mp in mr.m_Materials]
            except Exception:  # noqa: BLE001
                continue
            mat = next((m for m in mats if m is not None and m >= 0), None)
            if mats and all(m == -1 for m in mats if m is not None):
                renderer_material[mr.m_GameObject.path_id] = -1
            else:
                renderer_material[mr.m_GameObject.path_id] = mat
        mesh_count = 0
        # Meshes come from MeshFilters and from SkinnedMeshRenderers (wheels, dishes, chutes, legs), the latter
        # drawn in their bind pose at the renderer's transform.
        for o in env.objects:
            if o.type.name not in ("MeshFilter", "SkinnedMeshRenderer"):
                continue
            # Part skins retain raw vertices and real weights below. Character pieces exported with
            # a reference skeleton keep the existing baked-pose path so their head/suit/helmet align.
            if o.type.name == "SkinnedMeshRenderer" and not self.skeleton:
                continue
            try:
                mf = o.read()
                if not mf.m_Mesh or not mf.m_Mesh.path_id:
                    continue
                go_id = mf.m_GameObject.path_id
                name = go_name.get(go_id, "")
                if name.startswith("col_") or renderer_material.get(go_id) == -1:
                    continue
                # An engine's fairing (the shroud round its bell) shows in the original only when a part hangs
                # below the engine; drawn always, it turns an LV-T45 into a fat cylinder.
                if part_id.startswith("engine_") and "fairing" in name.lower():
                    continue
                # Exhaust plume meshes (exh_*, vfx_*) are effect geometry the original only shows while the
                # engine burns; static, they stand as 20 m white cones under every engine.
                if name.lower().startswith(("exh_", "vfx_")):
                    continue
                mesh = mf.m_Mesh.read()
            except Exception:  # noqa: BLE001
                continue
            tf_id = next((k for k, v in transforms.items() if v.m_GameObject.path_id == go_id), None)
            if tf_id is None:
                continue
            bake = None
            if o.type.name == "SkinnedMeshRenderer" and mf.m_Bones and mesh.m_BindPose:
                # Rest pose: the first bone's world matrix times its bind pose, relative to the renderer.
                b0 = mf.m_Bones[0].path_id
                if b0 in transforms:
                    # With a reference skeleton (pieces of one character, each prefab holding its own bones at
                    # different poses), the bone's world matrix comes from the reference by bone name.
                    bone_name = go_name.get(transforms[b0].m_GameObject.path_id, "")
                    w_bone = (self.skeleton or {}).get(bone_name)
                    if w_bone is None:
                        w_bone = world_of(b0)
                    bake = np.linalg.inv(world_of(tf_id)) @ w_bone @ mat44(mesh.m_BindPose[0])
            elif self.skeleton and o.type.name == "MeshFilter":
                # A rigid mesh hanging under one of the character's bones follows the reference pose of the
                # nearest such bone.
                cur, hops = tf_id, 0
                while cur in transforms and hops < 64:
                    name = go_name.get(transforms[cur].m_GameObject.path_id, "")
                    if cur != tf_id and name in self.skeleton:
                        w_tf = world_of(tf_id)
                        bake = np.linalg.inv(w_tf) @ self.skeleton[name] @ np.linalg.inv(world_of(cur)) @ w_tf
                        break
                    cur = transforms[cur].m_Father.path_id if transforms[cur].m_Father else 0
                    hops += 1
                else:
                    # Not under a bone: a prop the character wears (the helmet) hangs from `attach`.
                    if self.attach is not None:
                        w_tf = world_of(tf_id)
                        bake = np.linalg.inv(w_tf) @ self.attach @ w_tf
            positions, normals, uvs, tris = self.mesh_arrays(mesh, bake)
            if not positions or not tris:
                continue
            mat = renderer_material.get(go_id)
            meshes_by_tf[tf_id] = glb.mesh(mesh.m_Name, positions, normals, uvs, tris, mat if mat is not None and mat >= 0 else None)
            mesh_count += 1
        from export_animation import skinned_meshes, export_rig
        skins = [] if self.skeleton else skinned_meshes(self, env, glb, transforms, go_name, material_index, meshes_by_tf, part_id)
        mesh_count = len(meshes_by_tf)
        if mesh_count == 0:
            return None

        # The environment also holds the dependency bundles (the source models, shared effects), whose own
        # MeshFilters are not part of this prefab: keep only meshes under the prefab's root, the root-level
        # transform named after the part. Without it, effect plumes and source-model copies at their authoring
        # scale (cm) came along and drew 100 m cargo bays and antenna dishes.
        roots = [
            k for k, v in transforms.items()
            if not (v.m_Father and v.m_Father.path_id in transforms) and go_name.get(v.m_GameObject.path_id) == part_id
        ]
        # The EVA Kerbal's prefab holds only its sample container; the body comes from its model bundles.
        if roots and part_id != "eva_kerbal":
            under: set[int] = set()
            stack = list(roots)
            while stack:
                cur = stack.pop()
                if cur in under or cur not in transforms:
                    continue
                under.add(cur)
                stack.extend(c.path_id for c in transforms[cur].m_Children)
            mine = {k: v for k, v in meshes_by_tf.items() if k in under}
            if mine:
                meshes_by_tf = mine

        skins = [entry for entry in skins if entry[0] in meshes_by_tf]
        mesh_count = len(meshes_by_tf)

        # Only transforms that lead to a mesh are written, so the GLB stays small.
        keep: set[int] = set(transforms)  # Preserve bones, animation targets and procedural pivots too.
        for tf_id in meshes_by_tf:
            cur = tf_id
            hops = 0
            while cur in transforms and hops < 64:
                keep.add(cur)
                father = transforms[cur].m_Father.path_id if transforms[cur].m_Father else 0
                cur = father
                hops += 1

        node_by_tf: dict[int, int] = {}
        def build(tf_id: int) -> int:
            tf = transforms[tf_id]
            children = [build(c.path_id) for c in tf.m_Children if c.path_id in keep]
            name = go_name.get(tf.m_GameObject.path_id, "node")
            t = to_gltf_pos((tf.m_LocalPosition.x, tf.m_LocalPosition.y, tf.m_LocalPosition.z))
            r = to_gltf_quat(normalize((tf.m_LocalRotation.x, tf.m_LocalRotation.y, tf.m_LocalRotation.z, tf.m_LocalRotation.w)))
            sc = (float(tf.m_LocalScale.x), float(tf.m_LocalScale.y), float(tf.m_LocalScale.z))
            node = glb.node(name, t, r, sc, meshes_by_tf.get(tf_id), children)
            node_by_tf[tf_id] = node
            return node

        for tf_id in sorted(keep):
            tf = transforms[tf_id]
            father = tf.m_Father.path_id if tf.m_Father else 0
            if not father or father not in transforms:
                glb.roots.append(build(tf_id))
        if not glb.roots:
            return None
        path = self.out / "parts" / f"{part_id}.glb"
        metadata = export_rig(env, glb, transforms, go_name, node_by_tf, skins)
        # ProcedurallyEditablePart instantiates its mesh-option prefab with
        # DEFAULT_PART_VISUAL_ROTATION = Euler(0, 0, 90). The dependency meshes
        # above are still in their authoring frame: span +Z, chord +X. Apply
        # that runtime turn so chord becomes +Y and thickness matches the
        # part's -X lift axis. Mirroring X reverses the glTF turn about Z.
        if "_procedural_" in part_id and part_id.startswith(("wing_", "stabilizer_", "controlsurface_")):
            h = math.sqrt(0.5)
            glb.roots = [glb.node("procedural_visual", (0.0, 0.0, 0.0),
                                  (0.0, 0.0, -h, h), (1.0, 1.0, 1.0), None, glb.roots)]
        # Preserve semantic controller ownership. A state called OPEN on an engine can be a reverser
        # or a mode animation; it must not be treated as landing gear or a solar deployment.
        part_data = ROOT / "assets/ksp2/data/parts" / f"{part_id}.json"
        if "ose_deployment" in metadata and part_data.exists():
            modules = json.loads(part_data.read_text(encoding="utf-8"))["data"].get("serializedPartModules", [])
            names = {m["Name"] for m in modules}
            control = "panels" if "PartComponentModule_SolarPanel" in names else "gear" if any(n in names for n in ("PartComponentModule_LandingLeg", "PartComponentModule_WheelBase")) else ""
            metadata["ose_deployment"]["control"] = control
        size = glb.write(path, {"part_id": part_id, "source": main, **metadata})
        return {"id": part_id, "file": f"parts/{part_id}.glb", "bytes": size, "meshes": mesh_count, "textures": sorted(texture_slots_used)}

    # -- bodies

    def export_body(self, body_id: str) -> dict | None:
        key = f"Celestial.{body_id}.Scaled.prefab"
        env, bundle = self.env_for_key(key)
        if env is None:
            bundle = next((p.name for p in self.bdir.glob(f"celestialbody-scaled-{body_id.lower()}_assets_all*.bundle")), None)
            if not bundle:
                return None
            env = self.env(bundle)
        glb = Glb()
        textures: dict[str, int] = {}
        low = body_id.lower()
        for suffix, slot in (("_scaled_d", "baseColor"), ("_scaled_n", "normal"), ("_scaled_e", "emissive")):
            uri = None
            for k in self.cat.string_keys():
                kl = k.lower()
                if kl.endswith(f"/{low}{suffix}.png") or kl.endswith(f"/{low}{suffix}.tga") or kl == f"{low}{suffix}.png":
                    uri = self.export_texture(k)
                    if uri:
                        break
            if uri:
                textures[slot] = glb.image_uri("../" + uri, Path(uri).stem)
        if "baseColor" not in textures:
            # Stars and bodies with differently named maps: take the first big texture in the scaled bundle.
            for o in env.objects:
                if o.type.name == "Texture2D":
                    t = o.read()
                    if t.m_Width >= 1024:
                        uri = self.export_texture_object(t)
                        if uri:
                            textures["baseColor"] = glb.image_uri("../" + uri, Path(uri).stem)
                            break
        material = glb.material(body_id, textures) if textures else None
        done = None
        # The scaled sphere is the mesh behind the MeshFilter of the "Celestial.<Body>.Scaled" object; it may
        # live in a dependency bundle (shared spheres), so go through the filter rather than the bundle's meshes.
        candidates = []
        for o in env.objects:
            if o.type.name != "MeshFilter":
                continue
            try:
                mf = o.read()
                name = mf.m_GameObject.read().m_Name
                if mf.m_Mesh and mf.m_Mesh.path_id and name.startswith("Celestial."):
                    candidates.append(mf.m_Mesh.read())
            except Exception:  # noqa: BLE001
                continue
        if not candidates:
            candidates = [o.read() for o in env.objects if o.type.name == "Mesh"]
        for mesh in candidates:
            positions, normals, uvs, tris = self.mesh_arrays(mesh)
            if not positions:
                continue
            # The scaled-space sphere is unit-ish; the client scales it to the body radius.
            radius = max(math.sqrt(x * x + y * y + z * z) for x, y, z in positions) or 1.0
            m = glb.mesh(mesh.m_Name, positions, normals, uvs, tris, material)
            glb.roots.append(glb.node(body_id, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), m, []))
            done = {"id": body_id, "file": f"bodies/{body_id}.glb", "mesh_radius": radius, "vertices": len(positions), "textures": sorted(textures)}
            break
        if done is None:
            return None
        done["bytes"] = glb.write(self.out / "bodies" / f"{body_id}.glb", {"body_id": body_id, "mesh_radius": done["mesh_radius"], "source": bundle})
        return done


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default=str(ROOT / "assets" / "web"))
    ap.add_argument("--max-texture", type=int, default=1024)
    ap.add_argument("--parts", nargs="*", help="part ids (default: every part in assets/own/parts-catalog.json)")
    ap.add_argument("--bodies", nargs="*", help="body ids (default: every body in assets/own/kerbol-system.json)")
    ap.add_argument("--no-parts", action="store_true")
    ap.add_argument("--no-bodies", action="store_true")
    ap.add_argument("-v", "--verbose", action="store_true")
    a = ap.parse_args()

    out = Path(a.out)
    ex = Exporter(Path(a.install), out, a.max_texture, a.verbose)
    index: dict = {"generated": time.strftime("%Y-%m-%dT%H:%M:%S"), "max_texture": a.max_texture, "parts": [], "bodies": [], "missing": []}
    t0 = time.time()
    if not a.no_bodies:
        bodies = a.bodies if a.bodies is not None else [b["id"] for b in json.load(open(ROOT / "assets/own/kerbol-system.json", encoding="utf-8"))]
        for b in bodies:
            r = ex.export_body(b)
            if r:
                index["bodies"].append(r)
                print(f"body {b}: {r['vertices']} vertices, {r['textures']}, {r['bytes'] / 1e6:.1f} MB")
            else:
                index["missing"].append(f"body {b}")
                print(f"body {b}: not found")
    if not a.no_parts:
        parts = a.parts if a.parts is not None else [p["id"] for p in json.load(open(ROOT / "assets/own/parts-catalog.json", encoding="utf-8"))]
        for i, p in enumerate(parts):
            try:
                r = ex.export_part(p)
            except Exception as e:  # noqa: BLE001
                r = None
                print(f"part {p}: error {e}")
            if r:
                index["parts"].append(r)
                if a.verbose or len(parts) <= 10:
                    print(f"part {p}: {r['meshes']} meshes, {r['textures']}, {r['bytes'] / 1e6:.2f} MB")
            else:
                index["missing"].append(f"part {p}")
            if (i + 1) % 50 == 0:
                print(f"  ... {i + 1}/{len(parts)} parts, {time.time() - t0:.0f} s")
    index["textures"] = sorted(ex.texture_files.values())
    (out / "index.json").write_text(json.dumps(index, indent=1), encoding="utf-8")
    total = sum(f.stat().st_size for f in out.rglob("*") if f.is_file())
    print(f"done: {len(index['parts'])} parts, {len(index['bodies'])} bodies, {len(index['textures'])} textures, {total / 1e6:.0f} MB in {out} ({time.time() - t0:.0f} s); missing {len(index['missing'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
