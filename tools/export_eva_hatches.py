"""Read KSP2's authored airlock/climb transforms without modifying the installation."""
import json
from pathlib import Path
import numpy as np
from export_web_assets import Exporter, DEFAULT_INSTALL, ROOT
from export_menu_scene import load_scene


def main():
    ex = Exporter(Path(DEFAULT_INSTALL), ROOT / 'assets/web', 1024, False)
    catalog = json.loads((ROOT / 'assets/own/parts-catalog.json').read_text())
    result = {}
    for part in catalog:
        if not part.get('crew_capacity') or part['id'] == 'eva_kerbal':
            continue
        env, _ = ex.env_for_key(part['id'] + '.prefab')
        if env is None:
            continue
        roots = [r for sf in env.assets for r in load_scene(sf) if r.name == part['id']]
        if not roots:
            continue
        nodes = list(roots[0].walk())
        node = next((n for n in nodes if n.name.lower() == 'trigger_airlock_climb'), None)
        if node is None:
            node = next((n for n in nodes if 'airlock' in n.name.lower() and 'climb' in n.name.lower() and not n.children), None)
        if node is None:
            continue
        w = np.array(node.world)
        vec = lambda v: dict(zip('xyz', map(float, v)))
        result[part['id']] = {'position': vec(w[:3, 3]),
                              'up': vec(w[:3, 1] / np.linalg.norm(w[:3, 1])),
                              'forward': vec(w[:3, 2] / np.linalg.norm(w[:3, 2]))}
    (ROOT / 'assets/own/eva-hatches.json').write_text(json.dumps(result, indent=2) + '\n')
    print(f'Exported {len(result)} authored EVA hatch frames')


if __name__ == '__main__':
    main()
