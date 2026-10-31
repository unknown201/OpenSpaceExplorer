"""Export the original main menu backdrop (the `kspFlow.unity` scene) to web assets, read-only on the install.

What the game shows behind its main menu: a scaled Kerbin with two cloud layers and an atmosphere, turning
slowly, a stock vessel in the foreground spinning on its axis (one of three, picked at random), the sun and
three fill lights, a starfield cubemap. This script reads that scene from `boot-ksp_scenes_all.bundle` and
writes, under `assets/web/` (gitignored, pushed to the server with tools/push_assets.py):

    menu/scene.json      camera, lights, Kerbin and cloud placement with spin rates, atmosphere, the vessels as
                         lists of part placements (the parts themselves are the `parts/<id>.glb` exports)
    menu/kerbin.glb      the menu sphere with the `kerbin_menu_d` / `_n` textures
    menu/clouds.glb      the same sphere, the cloud cubemap unwrapped to a panorama, alpha blended
    menu/sky.glb         an inverted sphere with the starfield cubemap unwrapped to a panorama
    ui/<sprite>.png      the logo and the frames the menu is drawn with (from `ui_assets_all.bundle`), and the
                         flight HUD's navball atlas and marker icons (from `hudinstruments_assets_all.bundle`)

Frames: everything is written in the glTF frame the part exports use (Unity's X negated, quaternion y and z
negated, see export_web_assets.py). Spin rates are degrees per second about the world Y axis, already
sign-flipped for the mirrored frame.

    .venv/Scripts/python tools/export_menu_scene.py
    .venv/Scripts/python tools/export_menu_scene.py --max-texture 2048 --sky 2048
"""
from __future__ import annotations

import argparse
import json
import math
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter, Glb, normalize, to_gltf_pos, to_gltf_quat  # noqa: E402

SCENE_BUNDLE = "boot-ksp_scenes_all.bundle"
UI_BUNDLE = "ui_assets_all.bundle"
HUD_BUNDLE = "hudinstruments_assets_all.bundle"
HUD_TEXTURES = {
    "Flight-Navball-Texture", "Flight-Navball-Texture_Orbit", "Flight-Navball-Shadow", "Flight-Navball-Crosshair",
    "ICO-Navball-Prograde", "ICO-Navball-Retrograde", "ICO-Navball-Normal", "ICO-Navball-AntiNormal",
    "ICO-Navball-RadialIn", "ICO-Navball-RadialOut", "ICO-Navball-Maneuver", "ICO-Navball-Target", "ICO-Navball-AntiTarget",
}
UI_SPRITES = [
    "sno_logo-wht_red_2x", "WB-ELE-Button-Rounded", "WB-ELE-Box-Rounded", "WB-ELE-Box-Rounded-Stroke",
    "WB-ELE-Box 1", "Frame-6px-rad-fill", "Frame-6px-rad-border", "WB-ELE-PanelBG-Right", "BTN-FrontEnd-back",
    "Divider-DashSlash", "Divider-EqualSign", "ELE-tile-highlighted", "ELE-Placeholder-Flag", "flag_rocket",
    "bg2", "Background", "Tile-Dash-002", "WB-ICO-Check", "WB-ICO-Close-X", "ICO-Edit", "ICO-Launchpad",
    "Toggle-BG", "Toggle-Puck", "Scrollbar-Handle", "Scrollbar-Path", "cursor_arrow@2x", "logo_pd",
    "ICO-VAB-Med", "ICO-Training-Center-Med", "ICO-RDCenter-Med", "ICO-Tracking-Station-Med", "ICO-Mission-Control-Med",
    "ICO-Launchpad-Med", "ICO-Runway-16", "ICO-BoatLaunch-24", "ICO-Flag-2x", "ICO-Map-KSC-16x16", "ICO-Arrow-Pixel-Med",
    "Frame-6px-rad-shade", "Border-6pxRadius-ThiccBottom", "DIV-Period-002-1x", "TILE-TextPixelGrid", "SHAPE-Pixel-000-Fill-Med",
]


# ---------------------------------------------------------------------------------------------------------------
# small matrix helpers (Unity frame, column vectors)


def quat_to_mat(q):
    x, y, z, w = q
    return [
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ]


def mat_to_quat(m):
    t = m[0][0] + m[1][1] + m[2][2]
    if t > 0:
        s = math.sqrt(t + 1.0) * 2
        return normalize(((m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, 0.25 * s))
    if m[0][0] > m[1][1] and m[0][0] > m[2][2]:
        s = math.sqrt(1.0 + m[0][0] - m[1][1] - m[2][2]) * 2
        return normalize((0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s))
    if m[1][1] > m[2][2]:
        s = math.sqrt(1.0 + m[1][1] - m[0][0] - m[2][2]) * 2
        return normalize(((m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s))
    s = math.sqrt(1.0 + m[2][2] - m[0][0] - m[1][1]) * 2
    return normalize(((m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s, (m[1][0] - m[0][1]) / s))


def trs(t, q, s):
    r = quat_to_mat(q)
    return [
        [r[0][0] * s[0], r[0][1] * s[1], r[0][2] * s[2], t[0]],
        [r[1][0] * s[0], r[1][1] * s[1], r[1][2] * s[2], t[1]],
        [r[2][0] * s[0], r[2][1] * s[1], r[2][2] * s[2], t[2]],
        [0.0, 0.0, 0.0, 1.0],
    ]


def mul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(4)) for j in range(4)] for i in range(4)]


def inverse(m):
    # General 4x4 inverse by Gauss-Jordan; the matrices here are small TRS products.
    n = 4
    a = [row[:] + [1.0 if i == j else 0.0 for j in range(n)] for i, row in enumerate(m)]
    for c in range(n):
        p = max(range(c, n), key=lambda r: abs(a[r][c]))
        a[c], a[p] = a[p], a[c]
        piv = a[c][c]
        a[c] = [v / piv for v in a[c]]
        for r in range(n):
            if r != c and a[r][c] != 0.0:
                f = a[r][c]
                a[r] = [rv - f * cv for rv, cv in zip(a[r], a[c])]
    return [row[n:] for row in a]


def decompose(m):
    """-> (translation, quaternion, scale) of a Unity-frame TRS matrix.

    A mirrored transform (negative determinant: a prefab placed with a negative scale, as the KSC's dock
    warehouse, crane, HVAC units and the pads' hypergolic facilities are) keeps its mirror in a negative X scale
    so the rest is a proper rotation; read as a rotation the reflection came out tilted and turned."""
    t = (m[0][3], m[1][3], m[2][3])
    s = [math.sqrt(sum(m[i][j] ** 2 for i in range(3))) or 1.0 for j in range(3)]
    det = (m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
           - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
           + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]))
    if det < 0:
        s[0] = -s[0]
    r = [[m[i][j] / s[j] for j in range(3)] for i in range(3)]
    return t, mat_to_quat(r), tuple(s)


def to_gltf_trs(m):
    t, q, s = decompose(m)
    return {"pos": list(to_gltf_pos(t)), "rot": list(to_gltf_quat(q)), "scale": [float(x) for x in s]}


def transform_dir(m, v):
    return tuple(sum(m[i][k] * v[k] for k in range(3)) for i in range(3))


def gltf_dir(v):
    return [-float(v[0]), float(v[1]), float(v[2])]


# ---------------------------------------------------------------------------------------------------------------
# scene walk


class Node:
    def __init__(self, go, tf):
        self.go = go
        self.tf = tf
        self.name = go.m_Name
        self.active = bool(go.m_IsActive)
        self.children: list[Node] = []
        self.world = None

    def local(self):
        t = self.tf
        return trs(
            (t.m_LocalPosition.x, t.m_LocalPosition.y, t.m_LocalPosition.z),
            normalize((t.m_LocalRotation.x, t.m_LocalRotation.y, t.m_LocalRotation.z, t.m_LocalRotation.w)),
            (t.m_LocalScale.x, t.m_LocalScale.y, t.m_LocalScale.z),
        )

    def components(self):
        for c in self.go.m_Components:
            yield c.component if hasattr(c, "component") else c

    def component(self, type_name):
        for cc in self.components():
            if cc.type.name == type_name:
                return cc.read()
        return None

    def behaviour(self, class_name):
        for cc in self.components():
            if cc.type.name != "MonoBehaviour":
                continue
            try:
                r = cc.read()
                if r.m_Script.read().m_ClassName == class_name:
                    return cc.read_typetree()
            except Exception:  # noqa: BLE001
                continue
        return None

    def find(self, name, path=False):
        if self.name == name:
            return self
        for c in self.children:
            f = c.find(name)
            if f:
                return f
        return None

    def walk(self):
        yield self
        for c in self.children:
            yield from c.walk()


def load_scene(sf):
    """Build the Node tree of one serialized scene file; returns the roots with world matrices filled in."""
    objs = list(sf.objects.values())
    tfs = {o.path_id: o.read() for o in objs if o.type.name in ("Transform", "RectTransform")}
    nodes = {}
    tf_to_node = {}
    for o in objs:
        if o.type.name != "GameObject":
            continue
        go = o.read()
        tf = None
        tf_id = None
        for c in go.m_Components:
            cc = c.component if hasattr(c, "component") else c
            if cc.path_id in tfs:
                tf, tf_id = tfs[cc.path_id], cc.path_id
        if tf is None:
            continue
        n = Node(go, tf)
        nodes[o.path_id] = n
        tf_to_node[tf_id] = n
    roots = []
    for tf_id, n in tf_to_node.items():
        father = n.tf.m_Father.path_id if n.tf.m_Father else 0
        if father and father in tf_to_node:
            tf_to_node[father].children.append(n)
        else:
            roots.append(n)

    def fill(n, parent):
        n.world = mul(parent, n.local())
        for c in n.children:
            fill(c, n.world)

    identity = trs((0, 0, 0), (0, 0, 0, 1), (1, 1, 1))
    for r in roots:
        fill(r, identity)
    return roots


# ---------------------------------------------------------------------------------------------------------------
# cubemap -> equirectangular panorama


def cubemap_faces(cube, version, platform):
    """Decode the six mip-0 faces of a Unity Cubemap as PIL images (+X, -X, +Y, -Y, +Z, -Z)."""
    from UnityPy.export.Texture2DConverter import parse_image_data

    data = cube.get_image_data()
    n = int(cube.m_ImageCount or 6)
    per_face = len(data) // n
    faces = []
    for i in range(n):
        img = parse_image_data(data[i * per_face : (i + 1) * per_face], cube.m_Width, cube.m_Height, cube.m_TextureFormat, version, platform, cube.m_PlatformBlob, flip=True)
        faces.append(img.convert("RGBA"))
    return faces


def cubemap_to_panorama(faces, width: int, flip_faces: bool):
    """Sample the cubemap into a width x width/2 panorama whose column is Unity longitude atan2(x, z)
    (the sphere mesh's U) and whose row is latitude from the north pole down (the mesh's 1 - V)."""
    import numpy as np

    height = width // 2
    size = faces[0].width
    px = (np.arange(width) + 0.5) / width
    py = (np.arange(height) + 0.5) / height
    lon = px * 2 * np.pi
    lat = (0.5 - py) * np.pi
    cl = np.cos(lat)[:, None]
    dx = cl * np.sin(lon)[None, :]
    dy = np.sin(lat)[:, None] * np.ones((1, width))
    dz = cl * np.cos(lon)[None, :]
    ax, ay, az = np.abs(dx), np.abs(dy), np.abs(dz)
    out = np.zeros((height, width, 4), dtype=np.uint8)
    arrs = [np.asarray(f) for f in faces]
    # DirectX / Unity cubemap face conventions: (face, u, v, major axis magnitude)
    sel_x = (ax >= ay) & (ax >= az)
    sel_y = (ay > ax) & (ay >= az)
    sel_z = ~sel_x & ~sel_y
    conds = [
        (sel_x & (dx > 0), 0, -dz, -dy, ax),
        (sel_x & (dx <= 0), 1, dz, -dy, ax),
        (sel_y & (dy > 0), 2, dx, dz, ay),
        (sel_y & (dy <= 0), 3, dx, -dz, ay),
        (sel_z & (dz > 0), 4, dx, -dy, az),
        (sel_z & (dz <= 0), 5, -dx, -dy, az),
    ]
    for mask, face, u, v, mag in conds:
        s = (u / np.maximum(mag, 1e-9) + 1) * 0.5
        t = (v / np.maximum(mag, 1e-9) + 1) * 0.5
        if flip_faces:
            t = 1 - t
        col = np.clip((s * size).astype(np.int32), 0, size - 1)
        row = np.clip((t * size).astype(np.int32), 0, size - 1)
        out[mask] = arrs[face][row[mask], col[mask]]
    from PIL import Image

    return Image.fromarray(out, "RGBA")


# ---------------------------------------------------------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default=str(ROOT / "assets" / "web"))
    ap.add_argument("--max-texture", type=int, default=2048, help="Kerbin and cloud textures (the planet fills the screen)")
    ap.add_argument("--sky", type=int, default=4096, help="width of the starfield panorama")
    ap.add_argument("--no-flip-faces", action="store_true", help="do not flip cubemap faces vertically (Unity stores them bottom-up; flipping is what makes the panorama seamless)")
    ap.add_argument("--no-ui", action="store_true")
    a = ap.parse_args()
    out = Path(a.out)
    (out / "menu").mkdir(parents=True, exist_ok=True)
    (out / "ui").mkdir(parents=True, exist_ok=True)
    t0 = time.time()
    ex = Exporter(Path(a.install), out, a.max_texture, verbose=True)
    env = ex.env(SCENE_BUNDLE)
    bundle = next(iter(env.files.values()))
    scene_file = None
    for name, f in bundle.files.items():
        if hasattr(f, "objects") and f.objects and any(o.type.name == "Skybox" for o in f.objects.values()):
            scene_file = f
    if scene_file is None:
        sys.exit("kspFlow scene not found in the bundle")
    version = scene_file.version
    platform = scene_file.target_platform
    roots = load_scene(scene_file)
    flow = next(r for r in roots if r.name == "FlowManager")

    def need(name):
        n = flow.find(name)
        if n is None:
            sys.exit(f"scene node {name} not found")
        return n

    scene: dict = {"generated": time.strftime("%Y-%m-%dT%H:%M:%S"), "source": "kspFlow.unity"}

    # Camera: Unity cameras look along their +Z with +Y up.
    cam = need("Camera.Scaled")
    camc = cam.component("Camera")
    t, _, _ = decompose(cam.world)
    scene["camera"] = {
        "pos": list(to_gltf_pos(t)),
        "forward": gltf_dir(transform_dir(cam.world, (0, 0, 1))),
        "up": gltf_dir(transform_dir(cam.world, (0, 1, 0))),
        "fov_deg": float(camc.field_of_view),
        "near": float(camc.near_clip_plane),
        "far": float(camc.far_clip_plane),
        "background": [camc.m_BackGroundColor.r, camc.m_BackGroundColor.g, camc.m_BackGroundColor.b],
    }

    # Lights: a directional light shines along its +Z; the renderer wants the direction towards the light.
    lights = []
    for name in ("Sun", "Backlight", "KerbalLight", "ShipLight"):
        n = flow.find(name)
        if n is None:
            continue
        lc = n.component("Light")
        d = transform_dir(n.world, (0, 0, 1))
        lights.append({"name": name, "to_light": gltf_dir((-d[0], -d[1], -d[2])), "color": [lc.m_Color.r, lc.m_Color.g, lc.m_Color.b], "intensity": float(lc.m_Intensity)})
    scene["lights"] = lights

    # Kerbin: the mesh node (the inner "Kerbin" with the MeshFilter) and its spin.
    kerbin = None
    for n in need("Kerbin").walk():
        if n.name == "Kerbin" and n.component("MeshFilter") is not None:
            kerbin = n
    if kerbin is None:
        sys.exit("Kerbin mesh node not found")
    rot = kerbin.behaviour("RotateAtSpeed") or {}
    mf = kerbin.component("MeshFilter")
    mesh = mf.m_Mesh.read()
    positions, normals, uvs, tris = ex.mesh_arrays(mesh)
    mesh_radius = max(math.sqrt(x * x + y * y + z * z) for x, y, z in positions)
    mr = kerbin.component("MeshRenderer")
    kerbin_mat = mr.m_Materials[0].read()
    textures = {}
    for prop, slot in (("_MainTex", "baseColor"), ("_NormalMap", "normal")):
        for pname, te in kerbin_mat.m_SavedProperties.m_TexEnvs:
            if pname == prop and te.m_Texture and te.m_Texture.path_id:
                try:
                    tex = te.m_Texture.read()
                except Exception:  # noqa: BLE001
                    continue
                uri = ex.export_texture_object(tex)
                if uri:
                    textures[slot] = uri
    glb = Glb()
    gl_tex = {k: glb.image_uri("../" + v, Path(v).stem) for k, v in textures.items()}
    mat = glb.material("kerbin_menu", gl_tex)
    m = glb.mesh(mesh.m_Name, positions, normals, uvs, tris, mat)
    glb.roots.append(glb.node("Kerbin", (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), m, []))
    kb = glb.write(out / "menu" / "kerbin.glb", {"mesh_radius": mesh_radius, "source": SCENE_BUNDLE})
    k = to_gltf_trs(kerbin.world)
    scene["kerbin"] = {**k, "file": "menu/kerbin.glb", "bytes": kb, "mesh_radius": mesh_radius, "spin_deg_s": -float(rot.get("YSpeed", 0.0)), "textures": sorted(textures)}
    print(f"kerbin: {len(positions)} vertices, textures {sorted(textures)}, at {k['pos']}, scale {k['scale']}")

    # Clouds: SgtCloudsphere layers under the Kerbin mesh node; same sphere mesh, the cubemap as a panorama.
    clouds = []
    cloud_png = None
    for n in kerbin.children:
        sgt = n.behaviour("SgtCloudsphere")
        if not sgt:
            continue
        if cloud_png is None:
            ref = sgt.get("MainTex", {})
            cube = None
            for o in env.objects:
                if o.type.name == "Cubemap" and o.path_id == ref.get("m_PathID"):
                    cube = o.read()
            if cube is None:
                for o in env.objects:
                    if o.type.name == "Cubemap" and "cloud" in o.read().m_Name.lower():
                        cube = o.read()
            if cube is not None:
                faces = cubemap_faces(cube, version, platform)
                pano = cubemap_to_panorama(faces, min(a.max_texture, 4096), not a.no_flip_faces)
                cloud_png = "textures/kerbin_menu_clouds.png"
                pano.save(out / cloud_png)
                print(f"clouds: cubemap {cube.m_Name} {cube.m_Width}px -> panorama {pano.width}x{pano.height}")
        local = n.local()
        _, q, _ = decompose(local)
        spin = n.behaviour("RotateAtSpeed") or {}
        clouds.append({
            "name": n.name,
            "radius": float(sgt.get("Radius", 1000.0)) / mesh_radius,
            "rot": list(to_gltf_quat(q)),
            "spin_deg_s": -float(spin.get("YSpeed", 0.0)),
            "softness": float(sgt.get("Softness", 0.0)),
            "brightness": float(sgt.get("Brightness", 1.0)),
        })
    if cloud_png:
        glb = Glb()
        tex = glb.image_uri("../" + cloud_png, "kerbin_menu_clouds")
        mat = glb.material("clouds", {"baseColor": tex})
        glb.materials[mat]["alphaMode"] = "BLEND"
        glb.materials[mat]["doubleSided"] = False
        m = glb.mesh("clouds", positions, normals, uvs, tris, mat)
        glb.roots.append(glb.node("Clouds", (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), m, []))
        glb.write(out / "menu" / "clouds.glb", {"mesh_radius": mesh_radius, "source": SCENE_BUNDLE})
        scene["clouds"] = {"file": "menu/clouds.glb", "layers": clouds}
    atm = None
    for n in kerbin.children:
        atm = n.behaviour("SgtAtmosphere")
        if atm:
            break
    if atm:
        scene["atmosphere"] = {
            "height": float(atm.get("height", 39.0)) / mesh_radius,
            "brightness": float(atm.get("brightness", 1.0)),
            "color": [atm["color"]["r"], atm["color"]["g"], atm["color"]["b"]] if isinstance(atm.get("color"), dict) else [1, 1, 1],
            "inner_fog": float(atm.get("innerFog", 0.0)),
            "outer_fog": float(atm.get("outerFog", 0.0)),
        }

    # Sky: the starfield cubemap behind an inverted sphere (its triangles reversed so the inside is the front).
    sky = None
    for o in ex.env("boot-ksp_assets_all.bundle").objects:
        if o.type.name == "Cubemap":
            r = o.read()
            if "starfield" in r.m_Name.lower():
                sky = (r, o)
    if sky:
        cube, o = sky
        faces = cubemap_faces(cube, o.assets_file.version, o.assets_file.target_platform)
        pano = cubemap_to_panorama(faces, a.sky, not a.no_flip_faces)
        sky_png = "textures/skybox_spacescape_starfield.png"
        pano.convert("RGB").save(out / sky_png)
        glb = Glb()
        tex = glb.image_uri("../" + sky_png, "skybox_spacescape_starfield")
        mat = glb.material("sky", {"baseColor": tex})
        inv = [tris[i + 2 - j] for i in range(0, len(tris), 3) for j in range(3)]
        inv_n = [(-x, -y, -z) for x, y, z in normals] if normals else []
        m = glb.mesh("sky", positions, inv_n, uvs, inv, mat)
        glb.roots.append(glb.node("Sky", (0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0), m, []))
        glb.write(out / "menu" / "sky.glb", {"mesh_radius": mesh_radius, "source": "boot-ksp_assets_all.bundle"})
        skynode = need("Skybox")
        _, q, _ = decompose(skynode.world)
        scene["sky"] = {"file": "menu/sky.glb", "mesh_radius": mesh_radius, "rot": list(to_gltf_quat(q))}
        print(f"sky: {cube.m_Name} {cube.m_Width}px -> panorama {pano.width}x{pano.height}")

    # Vessels: each MM_Vessel_* under VesselPivot is a tree of part prefab instances ("<id>(Clone)");
    # record each part's transform relative to the vessel node so the client draws parts/<id>.glb there.
    catalog = {p["id"] for p in json.load(open(ROOT / "assets/own/parts-catalog.json", encoding="utf-8"))}
    exported = set()
    idx = out / "index.json"
    if idx.exists():
        exported = {p["id"] for p in json.load(open(idx, encoding="utf-8")).get("parts", [])}
    vessels = []
    pivot = need("VesselPivot")
    for v in pivot.children:
        if not v.name.startswith("MM_Vessel"):
            continue
        spin = v.behaviour("RotateAtSpeed") or {}
        inv_v = inverse(v.world)
        parts = []
        missing = []
        team = None

        def visit(n):
            nonlocal team
            tc = n.behaviour("ApplyTeamColors")
            if tc and team is None:
                base, acc = tc.get("baseOverrideColor", {}), tc.get("accentOverrideColor", {})
                team = {"base": [base.get("r", 1), base.get("g", 1), base.get("b", 1)], "accent": [acc.get("r", 1), acc.get("g", 1), acc.get("b", 1)]}
            if n.name.endswith("(Clone)"):
                pid = n.name[: -len("(Clone)")]
                if pid in catalog:
                    rel = mul(inv_v, n.world)
                    entry = {"id": pid, **to_gltf_trs(rel), "active": n.active}
                    parts.append(entry)
                    if pid not in exported:
                        missing.append(pid)
                    return
            for c in n.children:
                visit(c)

        visit(v)
        vessels.append({"name": v.name, **to_gltf_trs(v.world), "default": v.active, "spin_deg_s": -float(spin.get("YSpeed", 0.0)), "team": team, "parts": parts, "missing_parts": sorted(set(missing))})
        print(f"vessel {v.name}: {len(parts)} parts" + (f", not exported: {sorted(set(missing))}" if missing else ""))
    scene["vessels"] = vessels

    # UI sprites.
    if not a.no_ui:
        uenv = ex.env(UI_BUNDLE)
        wanted = set(UI_SPRITES)
        got = []
        for o in uenv.objects:
            if o.type.name != "Sprite":
                continue
            s = o.read()
            if s.m_Name in wanted and s.m_Name not in got:
                try:
                    s.image.save(out / "ui" / f"{s.m_Name}.png")
                    got.append(s.m_Name)
                except Exception as e:  # noqa: BLE001
                    print(f"  sprite {s.m_Name}: {e}")
        # Flight HUD textures (the navball atlas and its markers) from the HUD instruments bundle.
        henv = ex.env(HUD_BUNDLE)
        for o in henv.objects:
            if o.type.name != "Texture2D":
                continue
            t = o.read()
            if t.m_Name in HUD_TEXTURES and t.m_Name not in got:
                try:
                    t.image.save(out / "ui" / f"{t.m_Name}.png")
                    got.append(t.m_Name)
                except Exception as e:  # noqa: BLE001
                    print(f"  texture {t.m_Name}: {e}")
        scene["ui_sprites"] = sorted(got)
        print(f"ui: {len(got)} sprites" + (f", missing {sorted(wanted - set(got))}" if wanted - set(got) else ""))

    (out / "menu" / "scene.json").write_text(json.dumps(scene, indent=1), encoding="utf-8")
    if idx.exists():
        index = json.load(open(idx, encoding="utf-8"))
        index["menu"] = {"file": "menu/scene.json", "generated": scene["generated"], "vessels": [v["name"] for v in vessels]}
        idx.write_text(json.dumps(index, indent=1), encoding="utf-8")
    print(f"done in {time.time() - t0:.0f} s: {out / 'menu' / 'scene.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
