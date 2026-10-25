"""Export what the client needs to draw planets up close: surface textures, biome detail, atmosphere, clouds, ocean.

For every celestial body this reads the PQS surface material (`<body>_mat`, found through the body's PQSData
`materialSettings.surfaceMaterial`), the AtmosphereModel, the VolumeCloudConfiguration layers and the ocean
material from the install's `celestialbody-shared-<body>` and `celestialbody-simulation-<body>` bundles and
writes `assets/web/planets/<Body>/`:

    albedo.jpg          the material's `_AlbedoScaledTex` (the scaled albedo without water where there is one),
                        equirectangular, row 0 at the north pole, column 0 at longitude -180, 4096 x 2048
    normal.png          `_NormalScaledTex` (DXT5nm: x in alpha, y in green), 2048 x 1024
    biome.png           `_BiomeMaskTex` (four biome weights in RGBA), 2048 x 1024
    detail_a<i>.png     slice i of `_SmallAlbedoArray` (albedo in RGB, height in A), 512 x 512
    detail_n<i>.png     slice i of `_SmallNormalArray` (normal x in A, y in G, smoothness and AO in R and B), 512
    clouds.png          the first enabled cumulus layer's `bakedScaledTexture` cubemap, unwrapped like albedo
    atmo_transmittance.f16, atmo_irradiance.f16, atmo_scattering.f16
                        the AtmosphereModel's precomputed Bruneton LUTs as little-endian half floats, RGBA
                        (1024 x 128, 64 x 16 and 256 x 128 x 32 texels)
    planet.json         the parameters the shaders use (see docs/reference-notes/planet-rendering.md)

and `assets/web/planets/terrain.json` + `terrain.bin`, copies of `assets/own/terrain/` (the server's height maps), so
the client builds its terrain from exactly the heights the server lands on.

Usage: .venv/Scripts/python -I tools/export_planets.py [--install PATH] [--out assets/web/planets] [--bodies Kerbin,Mun]
"""
from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path

DEFAULT_INSTALL = r"C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2"
BODIES = ["Kerbin", "Mun", "Minmus", "Duna", "Ike", "Eve", "Gilly", "Moho", "Dres", "Jool", "Laythe", "Vall",
          "Tylo", "Bop", "Pol", "Eeloo"]


class Assets:
    """Every object of a body's bundles, by path id (ids are 64-bit hashes, unique across these bundles)."""

    def __init__(self, paths: list[Path]):
        import UnityPy

        self.env = UnityPy.load(*[str(p) for p in paths])
        self.by_id = {}
        for o in self.env.objects:
            self.by_id[o.path_id] = o

    def get(self, pptr: dict | None):
        if not pptr:
            return None
        pid = pptr.get("m_PathID") if isinstance(pptr, dict) else getattr(pptr, "path_id", 0)
        if not pid:
            return None
        return self.by_id.get(pid)

    def find_mono(self, key: str):
        for o in self.env.objects:
            if o.type.name != "MonoBehaviour":
                continue
            try:
                tt = o.read_typetree()
            except Exception:
                continue
            if key in tt:
                yield tt


def vec(c: dict) -> list[float]:
    if "r" in c:
        return [c["r"], c["g"], c["b"], c["a"]]
    return [c.get(k, 0.0) for k in ("x", "y", "z", "w") if k in c]


def material_props(mat_tt: dict) -> tuple[dict, dict, dict]:
    sp = mat_tt["m_SavedProperties"]
    tex = {k: v["m_Texture"] for k, v in sp["m_TexEnvs"]}
    floats = {k: v for k, v in sp["m_Floats"]}
    colors = {k: vec(v) for k, v in sp["m_Colors"]}
    return tex, floats, colors


def save_image(img, path: Path, size: tuple[int, int] | None, mode: str | None = None, quality: int = 90):
    from PIL import Image

    if mode:
        img = img.convert(mode)
    if size and img.size != size:
        img = img.resize(size, Image.LANCZOS)
    if path.suffix == ".jpg":
        img.convert("RGB").save(path, quality=quality)
    else:
        img.save(path, optimize=True)


def half_bytes(tex_obj, shape: tuple[int, ...]) -> bytes:
    """A float RGBA texture's raw data as little-endian half floats (WebGPU filters rgba16float, not rgba32float)."""
    import numpy as np

    tt = tex_obj.read_typetree()
    raw = tt.get("image data") or b""
    if not raw:
        raw = tex_obj.read().image_data
    a = np.frombuffer(bytes(raw), dtype="<f4").reshape(shape)
    return a.astype("<f2").tobytes()


GRAPHICS_TO_TEXTURE_FORMAT = {97: 10, 96: 10, 100: 12, 101: 12, 102: 26, 8: 4, 4: 4}


def array_slices(arr):
    """Mip 0 of every slice of a Texture2DArray as PIL images (the data streams from the bundle's .resS)."""
    from UnityPy.enums import TextureFormat
    from UnityPy.export.Texture2DConverter import parse_image_data
    from UnityPy.helpers.ResourceReader import get_resource_data

    data = arr.image_data
    if not data:
        sd = arr.m_StreamData
        data = get_resource_data(sd.path, arr.object_reader.assets_file, sd.offset, sd.size)
    fmt = TextureFormat(GRAPHICS_TO_TEXTURE_FORMAT[arr.m_Format])
    per_slice = len(data) // arr.m_Depth
    w, h = arr.m_Width, arr.m_Height
    bpp = {10: 0.5, 12: 1.0, 26: 0.5, 4: 4.0}[fmt.value]
    mip0 = int(w * h * bpp)
    out = []
    for i in range(arr.m_Depth):
        chunk = data[i * per_slice:i * per_slice + mip0]
        out.append(parse_image_data(chunk, w, h, fmt, arr.object_reader.version, arr.object_reader.platform, None,
                                    flip=True))
    return out


def cubemap_to_equirect(cube, width: int):
    """Unwrap a Unity cubemap into our equirectangular layout: column 0 at longitude -180, row 0 at the north pole,
    where the body-fixed Unity direction of (lat, lon) is (cos lat cos lon, sin lat, cos lat sin lon)."""
    import numpy as np
    from PIL import Image
    from UnityPy.export.Texture2DConverter import parse_image_data

    data = cube.get_image_data()
    n = int(cube.m_ImageCount or 6)
    per_face = len(data) // n
    faces = []
    for i in range(n):
        chunk = data[i * per_face:(i + 1) * per_face]
        if int(cube.m_TextureFormat) == 17:  # RGBAHalf; UnityPy's converter overflows on values above 1
            w = cube.m_Width
            px = np.frombuffer(bytes(chunk[:w * w * 8]), dtype="<f2").reshape(w, w, 4).astype(np.float32)
            faces.append((np.clip(px, 0.0, 1.0) * 255 + 0.5).astype(np.uint8)[::-1])
            continue
        img = parse_image_data(chunk, cube.m_Width, cube.m_Height, cube.m_TextureFormat,
                               cube.object_reader.version, cube.object_reader.platform, cube.m_PlatformBlob, flip=True)
        faces.append(np.asarray(img.convert("RGBA")))
    size = faces[0].shape[0]
    height = width // 2
    lon = ((np.arange(width) + 0.5) / width - 0.5) * 2 * np.pi
    lat = (0.5 - (np.arange(height) + 0.5) / height) * np.pi
    cl = np.cos(lat)[:, None]
    dx = cl * np.cos(lon)[None, :]
    dy = np.sin(lat)[:, None] * np.ones((1, width))
    dz = cl * np.sin(lon)[None, :]
    ax, ay, az = np.abs(dx), np.abs(dy), np.abs(dz)
    sel_x = (ax >= ay) & (ax >= az)
    sel_y = (ay > ax) & (ay >= az)
    sel_z = ~sel_x & ~sel_y
    out = np.zeros((height, width, 4), dtype=np.uint8)
    for mask, face, u, v, mag in [
        (sel_x & (dx > 0), 0, -dz, -dy, ax),
        (sel_x & (dx <= 0), 1, dz, -dy, ax),
        (sel_y & (dy > 0), 2, dx, dz, ay),
        (sel_y & (dy <= 0), 3, dx, -dz, ay),
        (sel_z & (dz > 0), 4, dx, -dy, az),
        (sel_z & (dz <= 0), 5, -dx, -dy, az),
    ]:
        s = (u / np.maximum(mag, 1e-9) + 1) * 0.5
        t = 1 - (v / np.maximum(mag, 1e-9) + 1) * 0.5
        # bilinear within the face (nearest texels alias the noisy coverage into speckle)
        fx = np.clip(s[mask] * size - 0.5, 0, size - 1)
        fy = np.clip(t[mask] * size - 0.5, 0, size - 1)
        x0 = np.floor(fx).astype(np.int32)
        y0 = np.floor(fy).astype(np.int32)
        x1 = np.minimum(x0 + 1, size - 1)
        y1 = np.minimum(y0 + 1, size - 1)
        ax_ = (fx - x0)[:, None]
        ay_ = (fy - y0)[:, None]
        f = faces[face].astype(np.float32)
        top = f[y0, x0] * (1 - ax_) + f[y0, x1] * ax_
        bot = f[y1, x0] * (1 - ax_) + f[y1, x1] * ax_
        out[mask] = np.clip(top * (1 - ay_) + bot * ay_ + 0.5, 0, 255).astype(np.uint8)
    return Image.fromarray(out, "RGBA")


SURFACE_VECTORS = [
    "_AlbedoScaledFadeParams", "_NormalScaledFadeParams", "_TriplanarUVScaleOffset", "_BiomeCutoffs",
]
SURFACE_FLOATS = ["_TriplanarContrast", "_StochasticScale", "_Radius"]
OCEAN_COLORS = ["_DeepWaterColor", "_ShallowColor", "_DiffuseColor", "_MainColor", "_SkyColor", "_SubsurfaceColor",
                "_FoamColor", "_UnderWaterDiffuseColor"]
OCEAN_FLOATS = ["_ShallowMaxDepth", "_ShallowColorDistribution", "_Smoothness", "_SpecularStrength", "_FresnelPower",
                "_TransparentVisibleDepth", "_DetailNormalTilingSize", "_LargeNormalMapTilingSize_1",
                "_LargeNormalMapTilingSize_2", "_LargeNormalMoveSpeed_1", "_LargeNormalStrength_1",
                "_LargeNormalStrength_2", "_DetailNormalMoveSpeed", "_DetailNormalStrength",
                "_ShorelineFoamMaxDepth", "_FoamShapeTilingSize", "_FoamSpeed", "_FoamVisibility",
                "_CausticsStrength", "_CausticsMaxVisibleDepth", "_CausticsTilingSize", "_CausticsAnimationSpeed"]


def ocean_spectrum(a: Assets) -> dict | None:
    """The body's Crest `OceanWaveSpectrum` (the FFT waves' source): wind, fetch, direction spread and the log10
    power of each of 14 wavelength octaves from 2^-4 m (disabled octaves dropped to the floor)."""
    for tt in a.find_mono("_powerLog"):
        return {
            "name": tt.get("m_Name"),
            "wind_speed": tt.get("_windSpeed", 10.0),
            "fetch_m": tt.get("_fetch", 500000.0),
            "direction_variance_deg": tt.get("_waveDirectionVariance", 90.0),
            "power_log": [(-8.0 if d else p) for p, d in zip(tt["_powerLog"], tt.get("_powerDisabled", []))],
            "chop_scales": tt.get("_chopScales"),
            "multiplier": tt.get("_multiplier", 1.0),
        }
    return None


def export_body(bid: str, bundles: Path, out_root: Path, args) -> dict | None:
    low = bid.lower()
    paths = [bundles / f"celestialbody-{k}-{low}_assets_all.bundle" for k in ("shared", "simulation")]
    paths = [p for p in paths if p.exists()]
    if not paths:
        return None
    a = Assets(paths)
    pqs = next(a.find_mono("heightMapInfo"), None)
    if pqs is None:
        print(f"{bid}: no PQSData")
        return None
    out = out_root / bid
    out.mkdir(parents=True, exist_ok=True)
    info: dict = {"id": bid, "version": 1}
    hmi = pqs["heightMapInfo"]
    info["pqs"] = {
        "height_map_scale_m": hmi["heightMapScale"],
        "scaled_to_local_transition_m": hmi["scaledToLocalTransition"],
        "scaled_to_local_blend_m": hmi["scaledToLocalBlend"],
        "world_space_tile_m": pqs["materialSettings"]["worldSpaceTile"],
    }
    mat_obj = a.get(pqs["materialSettings"]["surfaceMaterial"])
    if mat_obj is not None:
        mat = mat_obj.read_typetree()
        tex, floats, colors = material_props(mat)
        surf: dict = {"material": mat["m_Name"]}
        for k in SURFACE_VECTORS:
            if k in colors:
                surf[k.strip("_")] = colors[k]
        for k in SURFACE_FLOATS:
            if k in floats:
                surf[k.strip("_")] = floats[k]
        # per biome channel: four small texture slots and their rules
        biomes = {}
        for ch in "RGBA":
            slots = colors.get(f"_SmallBiome{ch}")
            if slots is None:
                continue
            b = {"slots": [int(s) for s in slots]}
            for key in ("UVScale", "Contrast", "Saturation", "NormalStrength", "HeightWeight", "WeightSoftness",
                        "AOStrength", "GlossStrength"):
                if f"_Small{key}{ch}" in colors:
                    b[key] = colors[f"_Small{key}{ch}"]
            for key in ("HeightEnable", "SlopeEnable"):
                if f"_SmallBiome{key}{ch}" in colors:
                    b[key] = colors[f"_SmallBiome{key}{ch}"]
            layers = []
            for i in range(1, 5):
                layer = {}
                for key in ("HeightParams", "SlopeParams", "GradMapWeights", "CurvMapWeights"):
                    v = colors.get(f"_SmallBiome{ch}{key}{i}")
                    layer[key] = v if v is not None else [0.0, 0.0, 0.0, 0.0]
                layer["Tint"] = colors.get(f"_SmallTint{ch}{i}", [1.0, 1.0, 1.0, 1.0])
                layers.append(layer)
            b["layers"] = layers
            biomes[ch] = b
        surf["biomes"] = biomes

        def tex_obj(name):
            return a.get(tex.get(name))

        alb = tex_obj("_AlbedoScaledTex")
        if alb is not None:
            img = alb.read().image
            save_image(img, out / "albedo.jpg", (args.albedo, args.albedo // 2), "RGB")
            # mean linear albedo, area-weighted (equirectangular rows shrink with cos(latitude)), for exposure
            import numpy as np

            small = np.asarray(img.convert("RGB").resize((256, 128))).astype(np.float64) / 255.0
            lin = np.where(small <= 0.04045, small / 12.92, ((small + 0.055) / 1.055) ** 2.4).mean(axis=2)
            wts = np.cos((0.5 - (np.arange(128) + 0.5) / 128) * np.pi)[:, None] * np.ones((1, 256))
            surf["mean_albedo"] = float((lin * wts).sum() / wts.sum())
            surf["albedo_source"] = alb.read().m_Name
        nrm = tex_obj("_NormalScaledTex")
        if nrm is not None:
            save_image(nrm.read().image, out / "normal.png", (args.albedo // 2, args.albedo // 4), "RGBA")
            surf["normal_source"] = nrm.read().m_Name
        bm = tex_obj("_BiomeMaskTex")
        if bm is not None:
            save_image(bm.read().image, out / "biome.png", (args.albedo // 2, args.albedo // 4), "RGBA")
        for name, prefix in (("_SmallAlbedoArray", "detail_a"), ("_SmallNormalArray", "detail_n")):
            arr = tex_obj(name)
            if arr is None:
                continue
            imgs = array_slices(arr.read())
            for i, img in enumerate(imgs):
                save_image(img, out / f"{prefix}{i}.png", (args.detail, args.detail), "RGBA")
            surf[f"{prefix}_count"] = len(imgs)
        info["surface"] = surf
    # atmosphere
    atmo = next(a.find_mono("RayleighScattering"), None)
    if atmo is not None:
        p = {k: (vec(v) if isinstance(v, dict) and not ("m_PathID" in v) else v) for k, v in atmo.items()
             if not (isinstance(v, dict) and "m_PathID" in v) and not k.startswith("m_")}
        for key, fname, shape in (("TransmittanceTexture", "atmo_transmittance.f16", (128, 1024, 4)),
                                  ("IrradianceTexture", "atmo_irradiance.f16", (16, 64, 4)),
                                  ("ScatteringTexture", "atmo_scattering.f16", (32, 128, 256, 4))):
            o = a.get(atmo.get(key))
            if o is not None:
                data = half_bytes(o, shape)
                (out / fname).write_bytes(data)
                import gzip

                (out / (fname + ".gz")).write_bytes(gzip.compress(data, compresslevel=6))
        info["atmosphere"] = p
    # clouds: the first enabled cumulus layer with a baked scaled texture
    for cfg in a.find_mono("cumulusList"):
        for layer in cfg["cumulusList"]:
            cube = a.get(layer.get("bakedScaledTexture"))
            if not layer.get("isEnable") or cube is None:
                continue
            img = cubemap_to_equirect(cube.read(), args.clouds)
            # alpha: the coverage; RGB: the layer's colour map where the config enables one (Eve's purple and pink
            # swirls, `cloudColorMap` x `colorMapIntensity`), else plain white (the baked RGB is not a colour)
            from PIL import Image

            a_ = img.getchannel("A")
            cmap = a.get(layer.get("cloudColorMap")) if cfg.get("enableColorMap") else None
            if cmap is not None:
                r_, g_, b_, _ = cubemap_to_equirect(cmap.read(), args.clouds).split()
            else:
                r_ = g_ = b_ = Image.new("L", a_.size, 255)
            Image.merge("RGBA", (r_, g_, b_, a_)).save(out / "clouds.png", optimize=True)
            info["clouds"] = {
                "config": cfg["m_Name"],
                "layer": layer["layerName"],
                "height_range_m": [layer["cloudHeightRange"]["x"], layer["cloudHeightRange"]["y"]],
                "planet_radius_m": cfg.get("planetRadius"),
                "movement_speed": layer.get("movementSpeed"),
                "wind_direction": vec(layer.get("windDirection", {"x": 0, "y": 0})),
                "density": layer.get("cloudsDensity"),
                "source": cube.read().m_Name,
                "color_map_intensity": cfg.get("colorMapIntensity") if cmap is not None else None,
            }
            break
        if "clouds" in info:
            break
    # ocean
    for o in a.env.objects:
        if o.type.name != "Material":
            continue
        tt = o.read_typetree()
        if "ocean" in tt["m_Name"] and "_DeepWaterColor" in {k for k, _ in tt["m_SavedProperties"]["m_Colors"]}:
            tex, floats, colors = material_props(tt)
            ocean = {"material": tt["m_Name"]}
            # foam.png: the foam shape in red, the caustics pattern in green
            from PIL import Image

            foam = a.get(tex.get("_FoamShapeTexture"))
            caus = a.get(tex.get("_CausticsTexture"))
            if foam is not None:
                fr = foam.read().image.convert("L").resize((1024, 1024), Image.LANCZOS)
                cg = (caus.read().image.convert("L").resize((1024, 1024), Image.LANCZOS) if caus is not None
                      else Image.new("L", (1024, 1024), 0))
                Image.merge("RGB", (fr, cg, Image.new("L", (1024, 1024), 0))).save(out / "foam.png", optimize=True)
                ocean["foam"] = foam.read().m_Name
                if caus is not None:
                    ocean["caustics"] = caus.read().m_Name
            for k in OCEAN_COLORS:
                if k in colors:
                    ocean[k.strip("_")] = colors[k]
            for k in OCEAN_FLOATS:
                if k in floats:
                    ocean[k.strip("_")] = floats[k]
            spectrum = ocean_spectrum(a)
            if spectrum:
                ocean["spectrum"] = spectrum
            info["ocean"] = ocean
            break
    # The original's per-body lighting (scaled bundle: CelestialBodyLighting ambient colours, the
    # post-process auto exposure range)
    scaled = bundles / f"celestialbody-scaled-{low}_assets_all.bundle"
    if scaled.exists():
        sa = Assets([scaled])
        light = {}
        for tt in sa.find_mono("data"):
            d = tt["data"]
            if isinstance(d, dict) and "ambientDay" in d:
                light["ambient_day"] = vec(d["ambientDay"])[:3]
                light["ambient_night"] = vec(d["ambientNight"])[:3]
        for tt in sa.find_mono("autoExposurePropertiesDay"):
            day = tt["autoExposurePropertiesDay"]
            light["auto_exposure"] = bool(tt.get("autoExposureEnabled"))
            light["exposure_day_ev"] = [day["minLuminance"], day["maxLuminance"], day["keyValue"]]
        if light:
            info["lighting"] = light
    # The scaled material's flow (Jool's bands drift: `_FlowmapSpeed1`, `_FlowmapStrength1`)
    for o in list(a.env.objects) + (list(sa.env.objects) if scaled.exists() else []):
        if o.type.name != "Material":
            continue
        mt = o.read_typetree()
        if mt.get("m_Name") != f"{low}_scaled":
            continue
        fl = dict(mt["m_SavedProperties"]["m_Floats"])
        if fl.get("_FlowmapSpeed1", 0.0) > 0.0 and "surface" in info:
            info["surface"]["flow"] = {"speed": fl["_FlowmapSpeed1"], "strength": fl.get("_FlowmapStrength1", 0.0)}
    (out / "planet.json").write_text(json.dumps(info, indent=1), encoding="utf-8")
    print(f"{bid}: surface={'surface' in info} atmosphere={'atmosphere' in info} clouds={'clouds' in info} "
          f"ocean={'ocean' in info}")
    return info


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default="assets/web/planets")
    ap.add_argument("--terrain", default="assets/own/terrain")
    ap.add_argument("--bodies", default=",".join(BODIES))
    ap.add_argument("--albedo", type=int, default=4096)
    ap.add_argument("--detail", type=int, default=512)
    ap.add_argument("--clouds", type=int, default=4096)
    args = ap.parse_args()
    bundles = Path(args.install) / "KSP2_x64_Data/StreamingAssets/aa/StandaloneWindows64"
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(Path(args.terrain) / "index.json", out / "terrain.json")
    shutil.copyfile(Path(args.terrain) / "terrain.bin", out / "terrain.bin")
    index = {"version": 1, "bodies": []}
    for bid in [b for b in args.bodies.split(",") if b]:
        info = export_body(bid, bundles, out, args)
        if info:
            index["bodies"].append({k: (k in info) for k in ("surface", "atmosphere", "clouds", "ocean")} | {"id": bid})
    if args.bodies == ",".join(BODIES):
        (out / "index.json").write_text(json.dumps(index, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
