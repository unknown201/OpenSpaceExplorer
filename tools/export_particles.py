"""Export authored KSP2 engine effects and nozzles into assets/web/fx (gitignored).

Run .venv/Scripts/python tools/export_particles.py; deploy with tools/push_assets.py fx.
Preserves meshes, morphs, shader arithmetic, source textures/mips and particle recipes.
"""
from __future__ import annotations
import argparse
import json
import math
from pathlib import Path
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter, to_gltf_pos
from export_menu_scene import load_scene, transform_dir


def kerbal_bone(ex: Exporter, name: str) -> tuple[float, float, float]:
    """Rest position of a bone in the exported Kerbal model (glTF frame, feet at the origin)."""
    import struct
    import numpy as np
    data = (ex.out / 'kerbal' / 'kerbal_m.glb').read_bytes()
    size = struct.unpack('<I', data[12:16])[0]
    nodes = json.loads(data[20:20 + size])['nodes']
    parent = {c: i for i, n in enumerate(nodes) for c in n.get('children', [])}
    def local(n):
        x, y, z, w = n.get('rotation', [0, 0, 0, 1])
        r = np.array([[1-2*(y*y+z*z), 2*(x*y-z*w), 2*(x*z+y*w)], [2*(x*y+z*w), 1-2*(x*x+z*z), 2*(y*z-x*w)],
                      [2*(x*z-y*w), 2*(y*z+x*w), 1-2*(x*x+y*y)]])
        m = np.eye(4); m[:3, :3] = r * np.array(n.get('scale', [1, 1, 1])); m[:3, 3] = n.get('translation', [0, 0, 0])
        return m
    i = next(i for i, n in enumerate(nodes) if n.get('name') == name)
    m = local(nodes[i])
    while i in parent:
        i = parent[i]; m = local(nodes[i]) @ m
    return tuple(float(v) for v in m[:3, 3])


def export_engines(ex: Exporter) -> dict:
    engines = {}
    for key in sorted(ex.cat.string_keys()):
        if not (key.startswith(("engine_", "booster_", "rcs_")) or key=='eva_kerbal.prefab') or not key.endswith(".prefab"):
            continue
        env, _ = ex.env_for_key(key)
        if env is None:
            continue
        part = key.removesuffix(".prefab")
        nozzles = []
        for sf in env.assets:
            for root in load_scene(sf):
                if root.name != part:
                    continue
                if part == 'eva_kerbal':
                    transforms = {n.tf.object_reader.path_id:n for n in root.walk()}
                    # The jetpack effects hang from bone_kerbal_eva_root. The prefab's root is the Kerbal's
                    # centre; the drawn Kerbal model stands on its feet. Re-anchor the nozzles from the
                    # effect container to that bone's rest position in the model.
                    container = next((n for n in root.walk() if n.name.startswith('fx_eva_spacesuit')), None)
                    shift = (0.0, 0.0, 0.0)
                    if container is not None:
                        here = to_gltf_pos(tuple(container.world[i][3] for i in range(3)))
                        shift = tuple(a - b for a, b in zip(kerbal_bone(ex, 'bone_kerbal_eva_root'), here))
                    for node in root.walk():
                        jetpack = node.behaviour('JetpackVFXManager')
                        if not jetpack: continue
                        moves = jetpack['MoveEffectObjects']
                        for group, effect in enumerate(moves+jetpack['RotateEffectObjects']):
                            direction = transforms[effect['Direction']['m_PathID']]
                            visual = transforms[effect['Visual']['m_PathID']]
                            d = transform_dir(direction.world,(0,1,0))
                            rotate = group >= len(moves)
                            at = to_gltf_pos(tuple(visual.world[i][3] for i in range(3)))
                            nozzles.append({'position':[a + b for a, b in zip(at, shift)],
                                'direction':list((d[0],-d[1],-d[2]) if rotate else to_gltf_pos(d)),
                                'mode':group,'rotate':rotate})
                    continue
                for node in root.walk():
                    rcs = part.startswith('rcs_')
                    if not (node.name in ('RCSthruster','rcsPort') if rcs else node.name.lower().startswith("thrusttransform")):
                        continue
                    # KSP thrust is -transform.forward; exhaust is +Z.
                    pos = to_gltf_pos(tuple(node.world[i][3] for i in range(3)))
                    direction = to_gltf_pos(transform_dir(node.world, (0,1,0) if rcs else (0,0,1)))
                    norm = math.sqrt(sum(v*v for v in direction))
                    if norm < 1e-9:
                        continue
                    nozzle = {"position": list(pos), "direction": [v/norm for v in direction]}
                    if rcs: nozzle['mode']=len(nozzles)
                    if nozzle not in nozzles:
                        nozzles.append(nozzle)
        if nozzles:
            engines[part] = nozzles
        else:
            print(f"warning: {part}: no thrust transform; client will use mesh bounds")
    return engines


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", type=Path, default=ROOT / "assets/web")
    ap.add_argument("--authored-only", action="store_true")
    ap.add_argument("--skip-reentry", action="store_true", help="Fast engine/context-only export while iterating")
    args = ap.parse_args()
    out = args.out / "fx"
    out.mkdir(parents=True, exist_ok=True)
    ex = Exporter(Path(args.install), args.out, 8192, False)
    from export_engine_fx import EngineFxExporter
    EngineFxExporter(ex,out).export(reentry=not args.skip_reentry)
    if args.authored_only: return
    provenance = {"generator":"tools/export_particles.py", "effects":"effects.json"}
    engines = export_engines(ex)
    (out / "engines.json").write_text(json.dumps(engines, indent=2) + "\n", encoding="utf-8")
    provenance["engine_count"] = len(engines)
    provenance["generator"] = "tools/export_particles.py"
    (out / "manifest.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    print(f"Exported authored effects and {len(engines)} engines ({sum(map(len, engines.values()))} nozzles) to {out}")


if __name__ == "__main__":
    main()
