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
                    for node in root.walk():
                        jetpack = node.behaviour('JetpackVFXManager')
                        if not jetpack: continue
                        moves = jetpack['MoveEffectObjects']
                        for group, effect in enumerate(moves+jetpack['RotateEffectObjects']):
                            direction = transforms[effect['Direction']['m_PathID']]
                            visual = transforms[effect['Visual']['m_PathID']]
                            d = transform_dir(direction.world,(0,1,0))
                            rotate = group >= len(moves)
                            nozzles.append({'position':list(to_gltf_pos(tuple(visual.world[i][3] for i in range(3)))),
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
