"""Preserve original gimbal controller ownership in GLBs and engine effect layers."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def definition(part, nodes=None):
    source = ROOT / 'assets/ksp2/data/parts' / f'{part}.json'
    if not source.exists():
        return None
    modules = json.loads(source.read_text(encoding='utf-8'))['data'].get('serializedPartModules', [])
    for module in modules:
        if module['Name'] == 'PartComponentModule_Gimbal':
            data = module['ModuleData'][0]['DataObject']
            config = {'transform': data.get('gimbalTransformName', 'thrustTransform'),
                    'response_speed': data.get('gimbalResponseSpeed', 10.0)
                    if data.get('useGimbalResponseSpeed', True) else None}
            # Some animated engines report torque through a childless proxy. Drive
            # their outer bell bones instead of turning that invisible transform.
            if nodes is not None and ('faux' in config['transform'].lower()
                                      or config['transform'] == 'thrustTransform'):
                config['visual_transforms'] = visual_names(nodes) or [config['transform']]
            return config
    return None


def visual_names(nodes):
    parents = {child: i for i, n in enumerate(nodes) for child in n.get('children', [])}
    candidates = {i for i, n in enumerate(nodes) if 'gimbal' in n.get('name', '').lower()
                  and not any(t in n['name'].lower() for t in ('faux', 'piston'))}
    selected = []
    for i in sorted(candidates):
        parent = parents.get(i)
        while parent is not None and parent not in candidates:
            parent = parents.get(parent)
        if parent is None:
            selected.append(nodes[i]['name'])
    return sorted(set(selected))


def effect_pivots(root, config):
    """Match exact effect transforms, including repeated names on clustered engines."""
    nodes = list(root.walk())
    indices = {id(n): i for i, n in enumerate(nodes)}
    names = [config['transform']]
    if 'faux' in config['transform'].lower() or config['transform'] == 'thrustTransform':
        names = visual_names([{'name': n.name, 'children': [indices[id(c)] for c in n.children]}
                              for n in nodes]) or names
    result = {}
    def walk(node, pivot):
        if node.name in names:
            pivot = [-node.world[0][3], node.world[1][3], node.world[2][3]]
        result[(node.name, tuple(v for row in node.world for v in row))] = pivot
        for child in node.children:
            walk(child, pivot)
    walk(root, None)
    return result


def tag_layers(root, layers, config):
    pivots = effect_pivots(root, config)
    for layer in layers:
        key = (layer['name'], tuple(v for row in layer['matrix'] for v in row))
        pivot = pivots.get(key)
        if pivot is not None:
            layer['gimbal_pivot'] = pivot


if __name__ == '__main__':
    # Upgrade existing generated assets without re-encoding their textures or meshes.
    import struct
    from export_web_assets import Exporter, DEFAULT_INSTALL
    from export_menu_scene import load_scene
    exporter = Exporter(Path(DEFAULT_INSTALL), ROOT / 'assets/web', 1024, False)
    count = 0
    for path in sorted((ROOT / 'assets/web/parts').glob('*.glb')):
        config = definition(path.stem)
        if config is None:
            continue
        blob = path.read_bytes()
        size = struct.unpack_from('<I', blob, 12)[0]
        document = json.loads(blob[20:20+size])
        document.setdefault('extras', {})['ose_gimbal'] = definition(path.stem, document['nodes'])
        encoded = json.dumps(document, separators=(',', ':')).encode()
        encoded += b' ' * (-len(encoded) % 4)
        tail = blob[20+size:]
        path.write_bytes(struct.pack('<4sII', b'glTF', 2, 20+len(encoded)+len(tail))
                         + struct.pack('<I4s', len(encoded), b'JSON') + encoded + tail)
        effects = ROOT / 'assets/web/fx/engines' / f'{path.stem}.json'
        if effects.exists():
            layers = json.loads(effects.read_text(encoding='utf-8'))
            env, _ = exporter.env_for_key(f'{path.stem}.prefab')
            for asset in env.assets:
                for root in load_scene(asset):
                    if root.name == path.stem:
                        tag_layers(root, layers, config)
            effects.write_text(json.dumps(layers, separators=(',', ':')), encoding='utf-8')
        count += 1
        print(path.stem, flush=True)
    print(f'Updated {count} gimbal rigs and their effects')
