"""Convert Unity generic transform clips and skinned renderers to standard glTF.

Unity's streamed polynomial, dense and constant curves share a component index space.
We evaluate that space at the source sample rate plus authored key times. Humanoid
retargeting, material curves and blend shapes are reported, never fabricated as TRS.
"""
from __future__ import annotations
import bisect
import math
import struct
import zlib


def accessor(glb, rows, width, kind=5126):
    fmt = {5126: "f", 5123: "H"}[kind]
    data = b"".join(struct.pack("<" + fmt * width, *row) for row in rows)
    view = glb._view(data, None)
    glb.accessors.append({"bufferView": view, "componentType": kind, "count": len(rows),
                          "type": {1:"SCALAR", 3:"VEC3", 4:"VEC4", 16:"MAT4"}[width]})
    if width == 1 and rows:
        glb.accessors[-1].update(min=[min(r[0] for r in rows)], max=[max(r[0] for r in rows)])
    return len(glb.accessors) - 1


def streamed_curves(clip):
    words = clip.get("data", [])
    data = struct.pack(f"<{len(words)}I", *words)
    curves = [[] for _ in range(clip.get("curveCount", 0))]
    at = 0
    while at < len(data):
        time, count = struct.unpack_from("<fI", data, at)
        at += 8
        for _ in range(count):
            index, *coeff = struct.unpack_from("<I4f", data, at)
            at += 20
            if index >= len(curves):
                raise ValueError("streamed curve index out of range")
            curves[index].append((time, coeff))
    return curves


def evaluate_stream(keys, time):
    at = max(0, bisect.bisect_right([k[0] for k in keys], time) - 1)
    start, (a, b, c, d) = keys[at]
    dt = time - start if math.isfinite(start) and start > -1e30 else 0.0
    return ((a * dt + b) * dt + c) * dt + d


def sampled_components(clip):
    muscle = clip["m_MuscleClip"]
    data = muscle["m_Clip"]["data"]
    streams = streamed_curves(data["m_StreamedClip"])
    dense = data["m_DenseClip"]
    constants = data.get("m_ConstantClip", {}).get("data", [])
    start, stop = muscle["m_StartTime"], muscle["m_StopTime"]
    duration = stop - start
    rate = max(1.0, min(120.0, clip.get("m_SampleRate", 30.0)))
    times = {start, stop}
    times.update(start + min(i / rate, duration) for i in range(math.ceil(duration * rate) + 1))
    times.update(t for keys in streams for t, _ in keys if start <= t <= stop)
    # glTF stores f32 times. Merge keys that would round to the same encoded time.
    times = sorted({struct.unpack("<f", struct.pack("<f", t-start))[0] for t in times})
    rows = []
    width = dense["m_CurveCount"]
    frames = dense["m_FrameCount"]
    for local_time in times:
        time = local_time + start
        row = [evaluate_stream(keys, time) if keys else 0.0 for keys in streams]
        if width:
            f = max(0.0, min(frames - 1, (time - dense["m_BeginTime"]) * dense["m_SampleRate"]))
            a, b = int(f), min(int(f) + 1, frames - 1)
            t = f - a
            row.extend(dense["m_SampleArray"][a*width+i]*(1-t) + dense["m_SampleArray"][b*width+i]*t for i in range(width))
        row.extend(constants)
        rows.append(row)
    return times, rows


def skinned_meshes(ex, env, glb, transforms, names, material_index, meshes_by_tf, part_id=""):
    """Add skinned meshes before the node hierarchy is built; return renderer/bind-pose pairs."""
    from UnityPy.helpers.MeshHelper import MeshHandler
    result = []
    tf_by_go = {t.m_GameObject.path_id: i for i,t in transforms.items()}
    for obj in env.objects:
        if obj.type.name != "SkinnedMeshRenderer":
            continue
        if obj.path_id in ex._excluded_lod_renderers:
            continue
        skin = obj.read()
        name = names.get(skin.m_GameObject.path_id, "").lower()
        if name.startswith(("col_", "exh_", "vfx_")) or (part_id.startswith("engine_") and "fairing" in name):
            continue
        tf = tf_by_go.get(skin.m_GameObject.path_id)
        if tf is None or not skin.m_Mesh.path_id:
            continue
        mesh = skin.m_Mesh.read()
        mats = [material_index(p) for p in skin.m_Materials]
        if mats and all(m == -1 for m in mats if m is not None):
            continue
        pos, normal, uv, triangles = ex.mesh_arrays(mesh)
        if not pos or not triangles:
            continue
        mat = next((m for m in mats if m is not None and m >= 0), None)
        mi = glb.mesh(mesh.m_Name, pos, normal, uv, triangles, mat)
        meshes_by_tf[tf] = mi
        if not skin.m_Bones:
            continue
        handler = MeshHandler(mesh)
        handler.process()
        joints, weights = handler.m_BoneIndices, handler.m_BoneWeights
        # Unity omits the weight stream for a rigid one-bone-per-vertex skin.
        if weights is None and joints and all(len(j) == 1 for j in joints):
            weights = [(1.0,) for _ in joints]
        if joints is None or weights is None:
            raise ValueError(f"{mesh.m_Name}: missing skin influence stream")
        if len(joints) != len(pos) or len(weights) != len(pos):
            raise ValueError(f"{names.get(skin.m_GameObject.path_id)}: incomplete skin weights")
        # Unity's four influences are preserved, including padding on meshes with fewer influences.
        jr = [tuple(list(j)[:4] + [0] * (4-len(j))) for j in joints]
        wr = [tuple(list(w)[:4] + [0.0] * (4-len(w))) for w in weights]
        if any(j >= len(skin.m_Bones) for row in jr for j in row):
            raise ValueError("skin joint index exceeds palette")
        attributes = glb.meshes[mi]["primitives"][0]["attributes"]
        attributes["JOINTS_0"] = accessor(glb, jr, 4, 5123)
        attributes["WEIGHTS_0"] = accessor(glb, wr, 4)
        result.append((tf, skin, mesh.m_BindPose))
    return result


def export_rig(env, glb, transforms, names, node_by_tf, skins):
    from export_web_assets import normalize, to_gltf_pos, to_gltf_quat
    audit = {"unresolved_bindings": [], "unresolved_binding_count": 0,
             "unsupported_bindings": 0, "sampled_clips": 0}
    # Animator paths are CRC32 hashes of slash-separated paths relative to its GameObject.
    by_go = {t.m_GameObject.path_id: i for i,t in transforms.items()}
    animators = [o.read() for o in env.objects if o.type.name == "Animator"]
    paths = {}
    for animator in animators:
        # Imported FBX dependencies may contain a second, unbound Animator and identical bone paths.
        # Only the prefab's controller owns these clips; the dependency rig must not overwrite targets.
        if not animator.m_Controller.path_id:
            continue
        root = by_go.get(animator.m_GameObject.path_id)
        for tf in transforms:
            at, pieces = tf, []
            while at in transforms and at != root:
                t = transforms[at]
                pieces.append(names.get(t.m_GameObject.path_id, ""))
                at = t.m_Father.path_id
            if at == root:
                path = "/".join(reversed(pieces))
                paths[zlib.crc32(path.encode())] = node_by_tf[tf]
    for tf, skin, bind in skins:
        if len(bind) != len(skin.m_Bones):
            raise ValueError("inverse bind count differs from joints")
        # S * B * S, S=diag(-1,1,1,1), column-major glTF.
        sign = [-1, 1, 1, 1]
        matrices = [tuple(getattr(m, f"e{r}{c}") * sign[r]*sign[c] for c in range(4) for r in range(4)) for m in bind]
        glb.skins.append({"joints": [node_by_tf[p.path_id] for p in skin.m_Bones],
                          "inverseBindMatrices": accessor(glb, matrices, 16)})
        glb.nodes[node_by_tf[tf]]["skin"] = len(glb.skins) - 1
    clip_by_id = {}
    for obj in env.objects:
        if obj.type.name != "AnimationClip":
            continue
        clip = obj.read_typetree()
        if "m_MuscleClip" not in clip:
            audit["unsupported_bindings"] += 1
            continue
        times, rows = sampled_components(clip)
        channels, samplers = [], []
        offset = 0
        for binding in clip["m_ClipBindingConstant"]["genericBindings"]:
            attr = binding["attribute"]
            is_transform = binding["typeID"] == 4
            width = (4 if attr == 2 else 3) if is_transform and attr in (1,2,3,4) else 1
            values = [r[offset:offset+width] for r in rows]
            offset += width
            if not is_transform or attr not in (1,2,3,4):
                audit["unsupported_bindings"] += 1
                continue
            node = paths.get(binding["path"])
            if node is None:
                audit["unresolved_binding_count"] += 1
                if len(audit["unresolved_bindings"]) < 32:
                    audit["unresolved_bindings"].append({"clip":clip["m_Name"], "path":binding["path"]})
                continue
            if any(len(v) != width for v in values):
                raise ValueError(f"{clip['m_Name']}: component binding exceeds clip")
            if attr == 1:
                values = [to_gltf_pos(v) for v in values]
                path = "translation"
            elif attr in (2,4):
                if attr == 4:
                    # Unity Euler is Z then X then Y (qY*qX*qZ).
                    def quat_mul(a,b):
                        x,y,z,w = a
                        X,Y,Z,W = b
                        return (w*X+x*W+y*Z-z*Y, w*Y-x*Z+y*W+z*X,
                                w*Z+x*Y-y*X+z*W, w*W-x*X-y*Y-z*Z)
                    def euler(v):
                        x,y,z = [math.radians(a)*0.5 for a in v]
                        return quat_mul(quat_mul((0,math.sin(y),0,math.cos(y)),(math.sin(x),0,0,math.cos(x))),(0,0,math.sin(z),math.cos(z)))
                    values = [euler(v) for v in values]
                values = [to_gltf_quat(normalize(v)) for v in values]
                # Quaternion signs must remain continuous between resampled keys.
                for i in range(1,len(values)):
                    if sum(a*b for a,b in zip(values[i-1],values[i])) < 0:
                        values[i] = tuple(-v for v in values[i])
                path, width = "rotation", 4
            else:
                path = "scale"
            samplers.append({"input": accessor(glb, [(t,) for t in times], 1),
                             "output": accessor(glb, values, width), "interpolation":"LINEAR"})
            channels.append({"sampler":len(samplers)-1,"target":{"node":node,"path":path}})
        if channels:
            clip_by_id[obj.path_id] = len(glb.animations)
            glb.animations.append({"name":clip["m_Name"],"samplers":samplers,"channels":channels})
            audit["sampled_clips"] += 1
    # Use the controller's semantic deployment state to select the clip, rather than part-id heuristics.
    candidates = set()
    for obj in env.objects:
        if obj.type.name != "AnimatorController":
            continue
        ctrl = obj.read_typetree()
        strings = dict(ctrl["m_TOS"])
        for machine in ctrl["m_Controller"]["m_StateMachineArray"]:
            for state in machine["data"]["m_StateConstantArray"]:
                state = state["data"]
                name = strings.get(state["m_NameID"], "").lower()
                if name not in ("open", "extend", "deploy", "opening", "extending", "deploying"):
                    continue
                for tree in state["m_BlendTreeConstantArray"]:
                    for node in tree["data"]["m_NodeArray"]:
                        ci = node["data"].get("m_ClipID", -1)
                        refs = ctrl["m_AnimationClips"]
                        if 0 <= ci < len(refs) and refs[ci]["m_PathID"] in clip_by_id:
                            candidates.add(clip_by_id[refs[ci]["m_PathID"]])
    metadata = {"ose_animation_audit":audit}
    if len(candidates) == 1:
        metadata["ose_deployment"] = {"clip":candidates.pop(), "reverse":False}
    return metadata
