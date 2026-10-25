"""Export each body's surface scatter (rocks, boulders, plants, trees): the original's Vegetation Studio Pro packages.

The original scatters props over the PQS surface with Vegetation Studio Pro: per biome (one `VegetationPackagePro`
per biome mask channel, matched through `VegetationSystemPro.BiomeTextureMask<C>ChannelType`), each item a prefab
placed on a grid of `SampleDistance` metres with its density, scale range, height and steepness rules and a
render distance from the system's `VegetationSettings` and the item's `RenderDistanceFactor`. This writes

    assets/web/scatter/<prefab>.glb       the prefab's LOD0 meshes and materials (shared textures under textures/)
    assets/web/planets/<Body>/scatter.json   the packages and items the client places (planet_scatter.rs)

Grass and items closer than 2.5 m apart are left out (thousands per square kilometre; the ground texture covers
them).

Usage: .venv/Scripts/python -I tools/export_scatter.py [--bodies Kerbin,Mun] [--max-texture 512]
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_menu_scene import inverse, load_scene, mul, to_gltf_trs  # noqa: E402
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter, Glb  # noqa: E402

BODIES = ["Kerbin", "Mun", "Minmus", "Duna", "Ike", "Eve", "Gilly", "Moho", "Dres", "Laythe", "Vall", "Tylo",
          "Bop", "Pol", "Eeloo"]
TYPE_NAMES = {0: "grass", 1: "plant", 2: "tree", 3: "object", 4: "large_object", 5: "medium_object"}
MIN_SPACING_M = 2.5


def submesh_arrays(mesh_obj):
    """Like Exporter.mesh_arrays, but one (positions, normals, uvs, triangles) per submesh."""
    from UnityPy.helpers.MeshHelper import MeshHandler

    from export_web_assets import to_gltf_pos

    h = MeshHandler(mesh_obj)
    h.process()
    positions = [to_gltf_pos(v) for v in h.m_Vertices]
    normals = [to_gltf_pos(nv) for nv in h.m_Normals] if h.m_Normals else []
    uvs = [(float(u[0]), 1.0 - float(u[1])) for u in h.m_UV0] if h.m_UV0 else []
    out = []
    for sub in h.get_triangles():
        tris: list[int] = []
        for a, b, c in sub:
            tris.extend((a, c, b))  # X mirrored: reverse to keep counter-clockwise fronts
        out.append((positions, normals, uvs, tris))
    return out


def mean_rgb(path: Path):
    import numpy as np
    from PIL import Image

    try:
        a = np.asarray(Image.open(path).convert("RGBA")).astype(np.float64) / 255.0
    except Exception:  # noqa: BLE001
        return None
    m = a[..., 3] > 0.5
    if m.sum() < 16:
        m = np.ones(a.shape[:2], bool)
    return a[m][:, :3].mean(axis=0)


def colour_match(out: Path, base_uris: list[str], atlas: Path) -> list[float]:
    import numpy as np
    from PIL import Image

    def transparent(u):
        try:
            a = np.asarray(Image.open(out / u).convert("RGBA"))[..., 3]
        except Exception:  # noqa: BLE001
            return 0.0
        return float((a < 128).mean())

    # foliage cards (largely transparent maps) set a tree's colour seen from afar, not its bark
    leaves = [u for u in base_uris if transparent(u) > 0.1]
    means = [m for m in (mean_rgb(out / u) for u in (leaves or base_uris)) if m is not None]
    am = mean_rgb(atlas)
    if not means or am is None:
        return [1.0, 1.0, 1.0]
    target = np.mean(means, axis=0)
    # brightness only: a per-channel ratio shifts the hue of trunks and shadows the atlas also holds
    lum = np.array([0.2126, 0.7152, 0.0722])
    k = float(np.clip(target.dot(lum) / max(am.dot(lum), 0.02), 0.5, 2.0))
    return [k, k, k]


def impostor_assets(env) -> dict:
    """AmplifyImpostorAsset by name ("tree_poplar_large_01_Impostor" ...) with their material."""
    objs = {o.path_id: o for o in env.objects}
    out = {}
    for o in env.objects:
        if o.type.name != "MonoBehaviour":
            continue
        try:
            tt = o.read_typetree()
        except Exception:  # noqa: BLE001
            continue
        if "ImpostorType" in tt and "Material" in tt:
            mo = objs.get((tt["Material"] or {}).get("m_PathID"))
            if mo is not None:
                out[tt["m_Name"]] = (tt, mo.read_typetree(), objs)
    # impostor materials whose asset is elsewhere: the material alone carries the atlas and its numbers
    for o in env.objects:
        if o.type.name != "Material":
            continue
        try:
            mt = o.read_typetree()
        except Exception:  # noqa: BLE001
            continue
        name = mt.get("m_Name", "")
        if name in out or not name.endswith("_Impostor"):
            continue
        if "_Frames" in dict(mt["m_SavedProperties"]["m_Floats"]):
            out[name] = ({"m_Name": name, "ImpostorType": 1}, mt, objs)
    return out


def export_impostor(imps: dict, prefab_name: str, png_path: Path) -> dict | None:
    """The prefab's Amplify impostor (the original draws far props as these), matched by name: its octahedral
    atlas of `frames` x `frames` views as a PNG, and its size and centre offset (object space, Unity)."""
    base = prefab_name
    found = None
    for _ in range(4):
        found = imps.get(base + "_Impostor")
        if found or "_" not in base:
            break
        base = base.rsplit("_", 1)[0]
    if not found:
        return None
    asset, mat, objs = found
    sp = mat["m_SavedProperties"]
    texs = {k: v["m_Texture"] for k, v in sp["m_TexEnvs"]}
    floats = dict(sp["m_Floats"])
    colors = {k: v for k, v in sp["m_Colors"]}
    alb = objs.get((texs.get("_Albedo") or {}).get("m_PathID"))
    if alb is None:
        return None
    try:
        img = alb.read().image.convert("RGBA")
    except Exception:  # noqa: BLE001
        return None
    img.resize((1024, 1024)).save(png_path, optimize=True)
    off = colors.get("_Offset", {"r": 0.0, "g": 0.0, "b": 0.0})
    return {
        "frames": int(floats.get("_Frames", asset.get("HorizontalFrames", 16))),
        "hemi": bool(floats.get("_Hemi", 0.0)),
        "size_m": floats.get("_ImpostorSize", 10.0),
        "offset": [off["r"], off["g"], off["b"]],
        "type": asset.get("ImpostorType", 1),
        "source": asset["m_Name"],
    }


def export_prefab(ex: Exporter, node, glb_path: Path) -> dict | None:
    """The prefab under `node` as a GLB: LOD0 renderers (all renderers when there is no LODGroup), each mesh
    placed relative to the prefab root."""
    lod_low: set[int] = set()
    for n in node.walk():
        for cc in n.components():
            if cc.type.name == "LODGroup":
                for i, lod in enumerate(cc.read_typetree().get("m_LODs", [])):
                    if i > 0:
                        for r in lod.get("renderers", []):
                            lod_low.add(r["renderer"]["m_PathID"])
    glb = Glb()
    materials: dict[int, int | None] = {}
    base_uris: list[str] = []

    def material_index(mp) -> int | None:
        if mp.path_id in materials:
            return materials[mp.path_id]
        try:
            m = mp.read()
        except Exception:  # noqa: BLE001
            materials[mp.path_id] = None
            return None
        textures = {}
        for name, te in m.m_SavedProperties.m_TexEnvs:
            slot = Exporter.MATERIAL_SLOTS.get(name) or {"_BaseColorMap": "baseColor", "_Albedo": "baseColor",
                                                          "_NormalMap": "normal"}.get(name)
            if not slot or not te.m_Texture or not te.m_Texture.path_id or slot in textures:
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
                if slot == "baseColor":
                    base_uris.append(uri)
        idx = glb.material(m.m_Name, textures)
        # Foliage (leaf and branch cards) is alpha-tested in the original; the client blends it instead.
        if any(k in m.m_Name.lower() for k in ("leaves", "leaf", "branch", "fern", "bush", "plant", "flower",
                                                "daffodil", "grass", "cactus")):
            glb.materials[idx]["alphaMode"] = "BLEND"
            glb.materials[idx]["doubleSided"] = True
        materials[mp.path_id] = idx
        return idx

    root_inv = inverse(node.world)
    nodes = []
    for n in node.walk():
        if not n.active:
            continue
        mr = mf = None
        mr_id = None
        for cc in n.components():
            if cc.type.name == "MeshRenderer":
                mr, mr_id = cc.read(), cc.path_id
            elif cc.type.name == "MeshFilter":
                mf = cc.read()
        if mr is None or mf is None or mr_id in lod_low or not mf.m_Mesh or not mf.m_Mesh.path_id:
            continue
        if n.name.lower().startswith(("col", "collider")):
            continue
        try:
            mesh = mf.m_Mesh.read()
            subs = submesh_arrays(mesh)
        except Exception:  # noqa: BLE001
            continue
        t = to_gltf_trs(mul(root_inv, n.world))
        mats = list(mr.m_Materials)
        # one glTF mesh per submesh, each with its own material (bark and leaves of a tree)
        for k, (positions, normals, uvs, tris) in enumerate(subs):
            if not positions or not tris:
                continue
            mp = mats[min(k, len(mats) - 1)] if mats else None
            mat = material_index(mp) if mp is not None and mp.path_id else None
            mi = glb.mesh(f"{mesh.m_Name}_{k}", positions, normals, uvs, tris, mat)
            nodes.append(glb.node(f"{n.name}_{k}", t["pos"], t["rot"], t["scale"], mi, []))
    if not nodes:
        return None
    root = glb.node(node.name, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), None, nodes)
    glb.roots = [root]
    glb.write(glb_path)
    return {"meshes": len(nodes), "base_colours": base_uris}


def rules(v: dict) -> dict:
    return {
        "height_rule": [v["MinHeight"], v["MaxHeight"]] if v.get("UseHeightRule") else None,
        "steepness_rule": [v["MinSteepness"], v["MaxSteepness"]] if v.get("UseSteepnessRule") else None,
        "noise_density": [v["NoiseDensityScale"], bool(v.get("NoiseDensityInverse"))]
        if v.get("UseNoiseDensity") else None,
        "noise_cutoff": [v["NoiseCutoffScale"], v["NoiseCutoffValue"], bool(v.get("NoiseCutoffInverse"))]
        if v.get("UseNoiseCutoff") else None,
    }


def grass_item(ex: Exporter, env, v: dict, distance: float) -> dict | None:
    """A grass item: Vegetation Studio draws a textured patch (its `VegetationTexture`) tinted between the
    shader controller's dry and healthy colours over `TintAreaScale` metres."""
    pid = (v.get("VegetationTexture") or {}).get("m_PathID")
    tex = next((o.read() for o in env.objects if o.path_id == pid), None) if pid else None
    if tex is None or tex.__class__.__name__ != "Texture2D":
        return None
    # every grass texture at one size, so the client can stack them in one texture array
    uri = f"scatter/grass_{tex.m_Name}.png"
    try:
        tex.image.convert("RGBA").resize((256, 256)).save(ex.out / uri)
    except Exception:  # noqa: BLE001
        return None
    props = {p["PropertyName"]: p for p in (v.get("ShaderControllerSettings") or {}).get("ControlerPropertyList", [])}

    def col(k, d):
        c = props.get(k, {}).get("ColorValue")
        return [c["r"], c["g"], c["b"]] if c else d

    sm = v.get("ScaleMultiplier", {"x": 1, "y": 1, "z": 1})
    return {
        "name": v.get("Name") or tex.m_Name,
        "model": "",
        "texture": uri,
        "kind": "grass",
        "size_m": 1.0 * sm["y"] * v.get("MaxScale", 1.0),
        "spacing_m": v["SampleDistance"],
        "density": v.get("Density", 1.0),
        "scale": [v.get("MinScale", 1.0) * sm["y"], v.get("MaxScale", 1.0) * sm["y"]],
        "y_offset": [0.0, 0.0],
        "seed": v.get("Seed", 0),
        "render_distance_m": distance * v.get("RenderDistanceFactor", 1.0),
        "tint": [col("TintColor1", [1.0, 1.0, 1.0]), col("TintColor2", [1.0, 1.0, 1.0])],
        "tint_scale_m": props.get("TintAreaScale", {}).get("FloatValue", 100.0),
        **rules(v),
    }


MASK_W, MASK_H = 1024, 512


def export_scatter_mask(install: Path, bid: str, pdir: Path) -> dict | None:
    """The vegetation system's own biome mask (`VegetationSystemPro.BiomeTextureMask`: kerbin_scatter_mask, or
    the scaled mask on most bodies), the weights its packages are placed by, as raw RGBA (`scatter_mask.bin`,
    MASK_W x MASK_H, row 0 at the north pole, column 0 at -180 deg) plus a gzip copy. Decoded when the texture
    is imported as sRGB, as the GPU samples it in the original."""
    import gzip
    import sys

    import numpy as np
    from PIL import Image

    sys.path.insert(0, str(Path(__file__).parent))
    import export_planets as ep

    bdir = install / "KSP2_x64_Data/StreamingAssets/aa/StandaloneWindows64"
    a = ep.Assets([bdir / f"celestialbody-{k}-{bid.lower()}_assets_all.bundle" for k in ("shared", "simulation")])
    for tt in a.find_mono("BiomeTextureMaskRChannelType"):
        o = a.get(tt.get("BiomeTextureMask"))
        if o is None:
            return None
        img = o.read().image.convert("RGBA").resize((MASK_W, MASK_H), Image.BILINEAR)
        if o.read_typetree().get("m_ColorSpace", 0) == 1:
            img = ep.srgb_to_linear_image(img, alpha=True)
        raw = np.asarray(img, dtype=np.uint8).tobytes()
        (pdir / "scatter_mask.bin").write_bytes(raw)
        (pdir / "scatter_mask.bin.gz").write_bytes(gzip.compress(raw, compresslevel=6))
        return {"file": "scatter_mask.bin", "width": MASK_W, "height": MASK_H, "source": o.read().m_Name}
    return None


def export_body(ex: Exporter, bid: str, out: Path) -> dict | None:
    bundle = f"celestialbody-simulation-{bid.lower()}_assets_all.bundle"
    if not (ex.bdir / bundle).exists():
        return None
    env = ex.env(bundle)
    roots_by_go: dict[int, object] = {}
    for bf in env.files.values():
        for sf in getattr(bf, "files", {}).values():
            if not getattr(sf, "objects", None):
                continue
            for r in load_scene(sf):
                for n in r.walk():
                    roots_by_go[n.go.object_reader.path_id] = n
    system = None
    packages = []
    for o in env.objects:
        if o.type.name != "MonoBehaviour":
            continue
        try:
            tt = o.read_typetree()
        except Exception:  # noqa: BLE001
            continue
        if "BiomeTextureMaskRChannelType" in tt:
            system = tt
        elif "VegetationInfoList" in tt:
            packages.append(tt)
    if system is None or not packages:
        return None
    imps = impostor_assets(env)
    channel_of = {system[f"BiomeTextureMask{c}ChannelType"]: c for c in "RGBA"}
    settings = system.get("VegetationSettings", {})
    base_distance = {
        "grass": settings.get("PlantDistance", 150.0) * settings.get("GrassDistanceFactor", 1.0),
        "plant": settings.get("PlantDistance", 150.0) * settings.get("PlantDistanceFactor", 1.0),
        "tree": (settings.get("PlantDistance", 150.0) + settings.get("AdditionalTreeMeshDistance", 200.0))
        * settings.get("TreeDistanceFactor", 1.0),
        "object": settings.get("PlantDistance", 150.0) * 2 * settings.get("ObjectDistanceFactor", 1.0),
        "medium_object": settings.get("AdditionalBillboardDistance", 2500.0)
        * settings.get("MediumObjectDistanceFactor", 0.5),
        "large_object": settings.get("AdditionalBillboardDistance", 2500.0)
        * settings.get("LargeObjectDistanceFactor", 0.5),
    }
    kind_density = {
        "grass": settings.get("GrassDensity", 1.0),
        "plant": settings.get("PlantDensity", 1.0),
        "tree": settings.get("TreeDensity", 1.0),
        "object": settings.get("ObjectDensity", 1.0),
        "medium_object": settings.get("MediumObjectDensity", 1.0),
        "large_object": settings.get("LargeObjectDensity", 1.0),
    }
    (out / "scatter").mkdir(parents=True, exist_ok=True)
    written: dict[str, dict | None] = {}
    result = {"version": 1, "id": bid, "sea_level": system.get("SeaLevel", 0.0), "packages": []}
    for pk in packages:
        ch = channel_of.get(pk["BiomeType"])
        if ch is None:
            continue  # a package outside the biome mask (KSC trees: baked placements)
        items = []
        for v in pk["VegetationInfoList"]:
            kind = TYPE_NAMES.get(v.get("VegetationType"), "object")
            pid = (v.get("VegetationPrefab") or {}).get("m_PathID")
            if kind == "grass":
                g = grass_item(ex, env, v, base_distance["grass"])
                if g:
                    items.append(g)
                continue
            if not pid or not v.get("RenderItem", 1):
                continue
            if v.get("SampleDistance", 0.0) < MIN_SPACING_M:
                continue
            node = roots_by_go.get(pid)
            if node is None:
                continue
            name = node.name.replace(" ", "_")
            if name not in written:
                written[name] = export_prefab(ex, node, out / "scatter" / f"{name}.glb")
            if not written[name]:
                continue
            imp_key = "imp:" + name
            if imp_key not in written:
                imp = export_impostor(imps, node.name, out / "scatter" / f"imp_{name}.png")
                if imp:
                    imp["texture"] = f"scatter/imp_{name}.png"
                    # The original grades its impostors in the shader (tint, hue, saturation, brightness); here
                    # the atlas is scaled so its mean colour matches the mesh's colour maps, and the switch from
                    # mesh to impostor does not show.
                    imp["colour_scale"] = colour_match(out, written[name].get("base_colours", []),
                                                       out / "scatter" / f"imp_{name}.png")
                written[imp_key] = imp
            sm = v.get("ScaleMultiplier", {"x": 1, "y": 1, "z": 1})
            ext = (v.get("Bounds") or {}).get("m_Extent", {"x": 1.0, "y": 1.0, "z": 1.0})
            size_m = 2.0 * max(ext["x"], ext["y"], ext["z"]) * sm["y"] * v.get("MaxScale", 1.0)
            items.append({
                "name": v.get("Name") or name,
                "model": f"scatter/{name}.glb",
                "impostor": written.get(imp_key),
                "kind": kind,
                "size_m": size_m,
                "spacing_m": v["SampleDistance"],
                # the item's density times the system's global one for its kind (Duna: objects 0.5, medium 0.4)
                "density": v.get("Density", 1.0) * kind_density.get(kind, 1.0),
                "scale": [v.get("MinScale", 1.0) * sm["y"], v.get("MaxScale", 1.0) * sm["y"]],
                "y_offset": [v.get("MinYOffset", 0.0), v.get("MaxYOffset", 0.0)],
                "seed": v.get("Seed", 0),
                "render_distance_m": base_distance[kind] * v.get("RenderDistanceFactor", 1.0),
                "height_rule": [v["MinHeight"], v["MaxHeight"]] if v.get("UseHeightRule") else None,
                "steepness_rule": [v["MinSteepness"], v["MaxSteepness"]] if v.get("UseSteepnessRule") else None,
                "noise_density": [v["NoiseDensityScale"], bool(v.get("NoiseDensityInverse"))]
                if v.get("UseNoiseDensity") else None,
                "noise_cutoff": [v["NoiseCutoffScale"], v["NoiseCutoffValue"], bool(v.get("NoiseCutoffInverse"))]
                if v.get("UseNoiseCutoff") else None,
            })
        if items:
            result["packages"].append({"name": pk["m_Name"], "channel": ch,
                                       "cutoff": pk.get("BiomeCutoffWeight", 0.0), "items": items})
    pdir = out / "planets" / bid
    pdir.mkdir(parents=True, exist_ok=True)
    mask = export_scatter_mask(ex.aa.parent.parent.parent, bid, pdir)
    if mask:
        result["mask"] = mask
    (pdir / "scatter.json").write_text(json.dumps(result, indent=1), encoding="utf-8")
    return {"packages": len(result["packages"]), "items": sum(len(p["items"]) for p in result["packages"]),
            "prefabs": sum(1 for k, v in written.items() if v and not k.startswith("imp:")),
            "impostors": sum(1 for k, v in written.items() if v and k.startswith("imp:"))}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default=str(ROOT / "assets" / "web"))
    ap.add_argument("--bodies", default=",".join(BODIES))
    ap.add_argument("--max-texture", type=int, default=512)
    a = ap.parse_args()
    out = Path(a.out)
    ex = Exporter(Path(a.install), out, a.max_texture, verbose=False)
    for bid in [b for b in a.bodies.split(",") if b]:
        info = export_body(ex, bid, out)
        print(f"{bid:8s} {info}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
