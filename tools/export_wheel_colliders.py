"""Where each wheeled part's wheel sits, from the part models (`assets/web/parts/<id>.glb`).

KSP2 rolls a wheel on the Unity WheelCollider of its prefab's `wheelCol` transform, not at the part's origin:
the Mk1 main gear (landinggear_1v) holds its wheel 0.52 m forward of where the part is attached, and the jet's
load on each wheel follows from that (strike taxi -geo capture: nose 11.3 kN, mains 32.7 and 33.5 kN). This
writes the `wheelCol` position of every part that has one, in the part's own Unity frame (glTF mirrors X; it is
mirrored back), metres, to the millimetre, as `assets/own/wheel-colliders.json`: `{part id: [x, y, z]}`.

    python tools/export_wheel_colliders.py --parts-dir D:/Projects/OpenSpaceExplorer/assets/web/parts
    python tools/export_wheel_colliders.py --base-url http://localhost:7790/assets/parts
"""
from __future__ import annotations

import argparse
import json
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from export_collision_hulls import mat_mul, node_matrix, read_glb  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
WHEEL_NODE = "wheelcol"


def wheel_position(data: bytes) -> list[float] | None:
    """The `wheelCol` node's origin in the model's root frame, Unity axes, or None without one."""
    doc, _ = read_glb(data)
    nodes = doc.get("nodes", [])
    roots = doc["scenes"][doc.get("scene", 0)]["nodes"]
    eye = [[1 if i == j else 0 for j in range(4)] for i in range(4)]
    stack = [(i, eye) for i in roots]
    while stack:
        i, parent = stack.pop()
        n = nodes[i]
        m = mat_mul(parent, node_matrix(n))
        if (n.get("name") or "").lower() == WHEEL_NODE:
            return [round(-m[0][3], 3), round(m[1][3], 3), round(m[2][3], 3)]
        stack += [(c, m) for c in n.get("children", [])]
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--parts-dir", type=Path, help="folder of <part id>.glb")
    parser.add_argument("--base-url", help="URL of a server's /assets/parts folder")
    parser.add_argument("--catalog", type=Path, default=ROOT / "assets/own/parts-catalog.json")
    parser.add_argument("--out", type=Path, default=ROOT / "assets/own/wheel-colliders.json")
    args = parser.parse_args()
    if not args.parts_dir and not args.base_url:
        parser.error("give --parts-dir or --base-url")
    catalog = json.loads(args.catalog.read_text(encoding="utf-8"))
    wheels: dict[str, list[float]] = {}
    missing = []
    for part in catalog:
        gear = part.get("landing_gear")
        if not gear or gear.get("leg"):
            continue
        pid = part["id"]
        try:
            if args.parts_dir:
                data = (args.parts_dir / f"{pid}.glb").read_bytes()
            else:
                with urllib.request.urlopen(f"{args.base_url.rstrip('/')}/{pid}.glb", timeout=60) as r:
                    data = r.read()
            at = wheel_position(data)
        except Exception as e:  # noqa: BLE001 - a part without a model rolls at its origin
            missing.append(f"{pid} ({e})")
            continue
        if at is None:
            missing.append(f"{pid} (no wheelCol)")
            continue
        wheels[pid] = at
    args.out.write_text(json.dumps(wheels, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(f"{len(wheels)} wheels -> {args.out}")
    if missing:
        print(f"{len(missing)} left at their origin: " + ", ".join(missing), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
