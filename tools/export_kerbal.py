"""Export the original's EVA Kerbal for the web client: the spacesuit body, the male head and eyes, and the
helmet, each its own prefab skinned to its own copy of the character skeleton. The pieces are placed with the
body prefab's skeleton (bone by name), so they line up as one Kerbal standing on y = 0, and written to
assets/web/kerbal/<piece>.glb with the planted flag (flag_portable).

    .venv/Scripts/python tools/export_kerbal.py
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter  # noqa: E402

# Piece and the bone it is worn on (the helmet prefab is attached to the helmet bone at run time).
PIECES = [
    ("body_spacesuit_01", None),
    ("head_m_01", None),
    ("eyes_m_01", None),
    ("helm_spacesuit_01", "bone_kerbal_helmet"),
    ("flag_portable", None),
]


def reference_skeleton(ex: Exporter, prefab: str) -> dict:
    import numpy as np

    env, _ = ex.env_for_key(f"{prefab}.prefab")
    transforms = {o.path_id: o.read() for o in env.objects if o.type.name == "Transform"}
    names = {}
    for o in env.objects:
        if o.type.name == "GameObject":
            try:
                names[o.path_id] = o.read().m_Name
            except Exception:  # noqa: BLE001
                pass

    def trs(tf):
        x, y, z, w = (tf.m_LocalRotation.x, tf.m_LocalRotation.y, tf.m_LocalRotation.z, tf.m_LocalRotation.w)
        r = np.array([[1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
                      [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
                      [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]])
        m = np.eye(4)
        m[:3, :3] = r * np.array([tf.m_LocalScale.x, tf.m_LocalScale.y, tf.m_LocalScale.z])
        m[:3, 3] = (tf.m_LocalPosition.x, tf.m_LocalPosition.y, tf.m_LocalPosition.z)
        return m

    out = {}
    for k, tf in transforms.items():
        m = np.eye(4)
        cur, hops = k, 0
        while cur in transforms and hops < 64:
            m = trs(transforms[cur]) @ m
            cur = transforms[cur].m_Father.path_id if transforms[cur].m_Father else 0
            hops += 1
        name = names.get(tf.m_GameObject.path_id)
        if name and name not in out:
            out[name] = m
    return out


def main() -> int:
    out = ROOT / "assets" / "web"
    ex = Exporter(Path(DEFAULT_INSTALL), out, 1024, False)
    ex.skeleton = reference_skeleton(ex, "body_spacesuit_01")
    print(f"reference skeleton: {len(ex.skeleton)} transforms")
    skeleton = ex.skeleton
    for piece, bone in PIECES:
        # The flag is not worn: no skeleton for it.
        ex.skeleton = skeleton if piece != "flag_portable" else None
        ex.attach = skeleton.get(bone) if bone else None
        r = ex.export_part(piece)
        if not r:
            print(f"{piece}: not found")
            continue
        src = out / "parts" / f"{piece}.glb"
        dst = out / "kerbal" / f"{piece}.glb"
        dst.parent.mkdir(parents=True, exist_ok=True)
        src.replace(dst)
        print(f"{piece}: {r['meshes']} meshes -> {dst.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
