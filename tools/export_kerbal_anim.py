#!/usr/bin/env python
"""Export an animated Kerbal: the original's spacesuit body skinned to its skeleton, with the head, eyes and
helmet merged onto the same bones, and the original's locomotion clips (walk, run, idle, jetpack idle)
converted to glTF animations on that skeleton.

    .venv/Scripts/python tools/export_kerbal_anim.py
    .venv/Scripts/python tools/push_assets.py kerbal

Writes assets/web/kerbal/kerbal_m.glb (male head) and kerbal_f.glb (female head). The clips live in
kerbal_shared_assets_all.bundle as generic transform curves, keyed by CRC32 of each bone's path under the
Kerbal's Animator; the paths are rebuilt from the exported skeleton's node names.
"""
from __future__ import annotations

import json
import math
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_animation import sampled_components  # noqa: E402
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter, normalize, to_gltf_pos, to_gltf_quat  # noqa: E402

CLIP_BUNDLE = "kerbal_shared_assets_all.bundle"
# Our names for the clips the client plays.
CLIPS = {
    # The "jp" clips are the original's EVA set, with the backpack on.
    "idle": ["kerbal_jp_idle"],
    "walk": ["kerbal_jp_walk_f", "kerbal_walk_f"],
    "walk_back": ["kerbal_jp_walk_b", "kerbal_walk_b"],
    "run": ["kerbal_jp_run", "kerbal_run"],
    "air": ["kerbal_jp_suspended_idle", "kerbal_suspended_idle"],
    # On a ladder (climbing up, down, hanging still); boarding plays the climb up.
    "ladder_up": ["kerbal_ladder_up"],
    "ladder_down": ["kerbal_ladder_down"],
    "ladder_idle": ["kerbal_ladder_idle"],
}
VARIANTS = {"m": ("head_m_01", "eyes_m_01"), "f": ("head_f_01", "eyes_f_01")}


class Doc:
    """A loaded GLB: its JSON and binary chunk, with helpers to append data and copy from another Doc."""

    def __init__(self, data: bytes) -> None:
        n = struct.unpack_from("<I", data, 12)[0]
        self.j = json.loads(data[20 : 20 + n])
        rest = data[20 + n :]
        self.bin = bytearray(rest[8 : 8 + struct.unpack_from("<I", rest, 0)[0]]) if rest else bytearray()
        for key in ("accessors", "bufferViews", "meshes", "materials", "textures", "images", "skins", "animations", "nodes", "samplers"):
            self.j.setdefault(key, [])

    def view(self, data: bytes) -> int:
        while len(self.bin) % 4:
            self.bin.append(0)
        self.j["bufferViews"].append({"buffer": 0, "byteOffset": len(self.bin), "byteLength": len(data)})
        self.bin += data
        return len(self.j["bufferViews"]) - 1

    def accessor(self, rows, width: int) -> int:
        data = b"".join(struct.pack("<" + "f" * width, *r) for r in rows)
        acc = {"bufferView": self.view(data), "componentType": 5126, "count": len(rows),
               "type": {1: "SCALAR", 3: "VEC3", 4: "VEC4", 16: "MAT4"}[width]}
        if width == 1 and rows:
            acc["min"] = [min(r[0] for r in rows)]
            acc["max"] = [max(r[0] for r in rows)]
        self.j["accessors"].append(acc)
        return len(self.j["accessors"]) - 1

    def copy_accessor(self, src: "Doc", i: int) -> int:
        acc = dict(src.j["accessors"][i])
        bv = src.j["bufferViews"][acc["bufferView"]]
        start = bv.get("byteOffset", 0)
        acc["bufferView"] = self.view(bytes(src.bin[start : start + bv["byteLength"]]))
        if "byteStride" in bv:
            self.j["bufferViews"][acc["bufferView"]]["byteStride"] = bv["byteStride"]
        self.j["accessors"].append(acc)
        return len(self.j["accessors"]) - 1

    def copy_material(self, src: "Doc", i: int | None, memo: dict) -> int | None:
        if i is None:
            return None
        if i in memo:
            return memo[i]
        m = json.loads(json.dumps(src.j["materials"][i]))

        def tex(ref: dict) -> None:
            t = src.j["textures"][ref["index"]]
            img = dict(src.j["images"][t["source"]])
            self.j["images"].append(img)
            self.j["textures"].append({"sampler": t.get("sampler", 0), "source": len(self.j["images"]) - 1})
            ref["index"] = len(self.j["textures"]) - 1

        for k in ("normalTexture", "occlusionTexture", "emissiveTexture"):
            if k in m:
                tex(m[k])
        for k in ("baseColorTexture", "metallicRoughnessTexture"):
            if k in m.get("pbrMetallicRoughness", {}):
                tex(m["pbrMetallicRoughness"][k])
        self.j["materials"].append(m)
        memo[i] = len(self.j["materials"]) - 1
        if not self.j["samplers"] and src.j.get("samplers"):
            self.j["samplers"] = src.j["samplers"]
        return memo[i]

    def copy_mesh(self, src: "Doc", i: int, memo: dict) -> int:
        m = json.loads(json.dumps(src.j["meshes"][i]))
        for p in m["primitives"]:
            p["attributes"] = {k: self.copy_accessor(src, v) for k, v in p["attributes"].items()}
            if "indices" in p:
                p["indices"] = self.copy_accessor(src, p["indices"])
            if "material" in p:
                p["material"] = self.copy_material(src, p["material"], memo)
        self.j["meshes"].append(m)
        return len(self.j["meshes"]) - 1

    def write(self, path: Path) -> int:
        while len(self.bin) % 4:
            self.bin.append(0)
        self.j["buffers"] = [{"byteLength": len(self.bin)}]
        js = json.dumps(self.j, separators=(",", ":")).encode()
        js += b" " * (-len(js) % 4)
        total = 12 + 8 + len(js) + 8 + len(self.bin)
        out = struct.pack("<III", 0x46546C67, 2, total) + struct.pack("<II", len(js), 0x4E4F534A) + js
        out += struct.pack("<II", len(self.bin), 0x004E4942) + bytes(self.bin)
        path.write_bytes(out)
        return len(out)


def parents(doc: Doc) -> dict[int, int]:
    return {c: i for i, n in enumerate(doc.j["nodes"]) for c in n.get("children", [])}


def add_child(doc: Doc, parent: int, node: dict) -> int:
    doc.j["nodes"].append(node)
    k = len(doc.j["nodes"]) - 1
    doc.j["nodes"][parent].setdefault("children", []).append(k)
    return k


def local_chain(src: Doc, par: dict, start: int, stop: int) -> list[dict]:
    """The nodes from just below `stop` down to `start`, as TRS dicts (top first)."""
    chain = []
    k = start
    while k != stop and k in par:
        chain.append(k)
        k = par[k]
    return [src.j["nodes"][c] for c in reversed(chain)]


def merge_piece(dst: Doc, src: Doc, by_name: dict[str, int], attach_to: str | None) -> None:
    """Merge a piece's meshes onto dst's skeleton: rigid meshes under the same-named bone (keeping their
    local offsets), skinned meshes with joints remapped by name, and meshes under no known bone onto
    `attach_to` (the helmet)."""
    par = parents(src)
    memo: dict = {}
    root = dst.j["scenes"][0]["nodes"][0]
    for i, n in enumerate(src.j["nodes"]):
        if "mesh" not in n:
            continue
        mesh = dst.copy_mesh(src, n["mesh"], memo)
        if "skin" in n:
            skin = src.j["skins"][n["skin"]]
            joints = [by_name[src.j["nodes"][jn]["name"]] for jn in skin["joints"]]
            dst.j["skins"].append({"joints": joints, "inverseBindMatrices": dst.copy_accessor(src, skin["inverseBindMatrices"])})
            add_child(dst, root, {"name": n["name"], "mesh": mesh, "skin": len(dst.j["skins"]) - 1})
            continue
        # The nearest ancestor that is one of dst's bones.
        k, bone = i, None
        while k in par:
            k = par[k]
            if src.j["nodes"][k]["name"] in by_name:
                bone = k
                break
        if bone is None:
            # A worn prop (the helmet) hangs from the attach bone with its whole authored chain, root
            # included, as export_kerbal's `attach` placed it: bone world times the mesh's prefab world.
            target = by_name[attach_to] if attach_to else root
            chain, k = [n], i
            while k in par:
                k = par[k]
                chain.insert(0, src.j["nodes"][k])
        else:
            target = by_name[src.j["nodes"][bone]["name"]]
            chain = local_chain(src, par, i, bone)
        for c in chain[:-1]:
            target = add_child(dst, target, {k2: c[k2] for k2 in ("name", "translation", "rotation", "scale") if k2 in c})
        leaf = {k2: n[k2] for k2 in ("name", "translation", "rotation", "scale") if k2 in n}
        leaf["mesh"] = mesh
        add_child(dst, target, leaf)


def add_clips(dst: Doc, ex: Exporter) -> list[str]:
    nodes = dst.j["nodes"]
    par = parents(dst)

    def path_of(i: int, stop: int | None) -> str | None:
        parts = []
        k = i
        while True:
            parts.append(nodes[k]["name"])
            if k not in par:
                return "/".join(reversed(parts)) if stop is None else None
            k = par[k]
            if k == stop:
                return "/".join(reversed(parts))

    env = ex.env(CLIP_BUNDLE)
    clips = {o.peek_name(): o for o in env.objects if o.type.name == "AnimationClip"}
    wanted = {}
    for key, names in CLIPS.items():
        for name in names:
            if name in clips:
                wanted[key] = clips[name]
                break
    # The Animator's root is not exported; try each node as the root (and none) and keep the one whose
    # paths match most bindings.
    sample = wanted["walk"].read_typetree()  # the bones every clip moves
    hashes = {b["path"] for b in sample["m_ClipBindingConstant"]["genericBindings"] if b["typeID"] == 4}
    # Only the skeleton the skins use (under the first root): the export also carries the source rig's
    # unbound copy (all_kerbal_parts), whose identical paths must not take the curves.
    live = set()
    stack = [dst.j["scenes"][0]["nodes"][0]]
    while stack:
        k = stack.pop()
        live.add(k)
        stack.extend(nodes[k].get("children", []))
    best, best_map = -1, {}
    for stop in [None, *sorted(live)]:
        m = {}
        for i in sorted(live):
            p = path_of(i, stop)
            if p is not None:
                m.setdefault(zlib.crc32(p.encode()), i)
        hits = len(hashes & m.keys())
        if hits > best:
            best, best_map = hits, m
    print(f"clip bindings matched: {best}/{len(hashes)}")
    done = []
    for key, obj in wanted.items():
        clip = obj.read_typetree()
        times, rows = sampled_components(clip)
        channels, samplers = [], []
        offset = 0
        for b in clip["m_ClipBindingConstant"]["genericBindings"]:
            attr, is_tf = b["attribute"], b["typeID"] == 4
            width = (4 if attr == 2 else 3) if is_tf and attr in (1, 2, 3, 4) else 1
            values = [r[offset : offset + width] for r in rows]
            offset += width
            node = best_map.get(b["path"]) if is_tf and attr in (1, 2, 3, 4) else None
            if node is None:
                continue
            if attr == 1:
                values, path = [to_gltf_pos(v) for v in values], "translation"
            elif attr in (2, 4):
                if attr == 4:
                    def qmul(a, c):
                        x, y, z, w = a
                        X, Y, Z, W = c
                        return (w * X + x * W + y * Z - z * Y, w * Y - x * Z + y * W + z * X, w * Z + x * Y - y * X + z * W, w * W - x * X - y * Y - z * Z)

                    def euler(v):
                        x, y, z = [math.radians(a) * 0.5 for a in v]
                        # The Kerbal rig's bones keep the FBX rotation order XYZ (X first): q = qZ * qY * qX.
                        # Checked against the prefab's rest pose (the neck and attach bones match exactly).
                        return qmul(qmul((0, 0, math.sin(z), math.cos(z)), (0, math.sin(y), 0, math.cos(y))), (math.sin(x), 0, 0, math.cos(x)))

                    values = [euler(v) for v in values]
                values = [to_gltf_quat(normalize(v)) for v in values]
                for i in range(1, len(values)):
                    if sum(p * q for p, q in zip(values[i - 1], values[i])) < 0:
                        values[i] = tuple(-v for v in values[i])
                path, width = "rotation", 4
            else:
                path = "scale"
            ts = times
            # A bone that holds still through the clip needs one key, not one per sample.
            if all(max(abs(a - b) for a, b in zip(v, values[0])) < 1e-5 for v in values):
                ts, values = [times[0]], [values[0]]
            samplers.append({"input": dst.accessor([(t,) for t in ts], 1), "output": dst.accessor(values, width), "interpolation": "LINEAR"})
            channels.append({"sampler": len(samplers) - 1, "target": {"node": node, "path": path}})
        if channels:
            dst.j["animations"].append({"name": key, "samplers": samplers, "channels": channels})
            done.append(f"{key}={obj.peek_name()} ({len(channels)} channels, {times[-1]:.2f}s)")
    return done


def glass_visor(doc: Doc) -> None:
    for m in doc.j["materials"]:
        if m.get("name") == "visor":
            m["alphaMode"] = "BLEND"
            m["doubleSided"] = True
            m["pbrMetallicRoughness"] = {"baseColorFactor": [0.55, 0.66, 0.8, 0.22], "metallicFactor": 0.0, "roughnessFactor": 0.05}


def main() -> int:
    web = ROOT / "assets" / "web"
    ex = Exporter(Path(DEFAULT_INSTALL), web, 1024, False)
    ex.skeleton = None
    ex.attach = None
    pieces: dict[str, Doc] = {}
    for piece in ["body_spacesuit_01", "helm_spacesuit_01", *[p for v in VARIANTS.values() for p in v]]:
        r = ex.export_part(piece)
        if not r:
            print(f"{piece}: not found")
            return 1
        src = web / "parts" / f"{piece}.glb"
        pieces[piece] = Doc(src.read_bytes())
        src.unlink()
    for variant, (head, eyes) in VARIANTS.items():
        body = pieces["body_spacesuit_01"]
        doc = Doc.__new__(Doc)
        doc.j = json.loads(json.dumps(body.j))
        doc.bin = bytearray(body.bin)
        doc.j["scenes"][0]["nodes"] = doc.j["scenes"][0]["nodes"][:1]
        by_name = {}
        for i, n in enumerate(doc.j["nodes"]):
            by_name.setdefault(n["name"], i)
        merge_piece(doc, pieces[head], by_name, None)
        merge_piece(doc, pieces[eyes], by_name, None)
        merge_piece(doc, pieces["helm_spacesuit_01"], by_name, "bone_kerbal_helmet")
        glass_visor(doc)
        # The original tints its grey head texture with the Kerbal's skin colour; bake the commonest (BADA55)
        # into the head mesh's own material (the eyes share the texture name but stay white).
        for mesh in doc.j["meshes"]:
            if "head" in mesh["name"]:
                for prim in mesh["primitives"]:
                    if "material" in prim:
                        mat = doc.j["materials"][prim["material"]]
                        mat.setdefault("pbrMetallicRoughness", {})["baseColorFactor"] = [0xBA / 255, 0xDA / 255, 0x55 / 255, 1.0]
        print(variant, *add_clips(doc, ex), sep="\n  ")
        doc.j["extras"] = {"source": "tools/export_kerbal_anim.py", "head": head}
        out = web / "kerbal" / f"kerbal_{variant}.glb"
        print(f"  -> {out.relative_to(ROOT)}: {doc.write(out)} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
