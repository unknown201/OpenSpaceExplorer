"""Preserve authored engine VFX: meshes/morph targets, material/shader logic and control curves."""
from __future__ import annotations
import hashlib
import io
import json
import math
import re
import struct
from pathlib import Path
from export_menu_scene import load_scene, inverse, mul, trs
from port_shader import programs, disassemble, wgsl

CRASH_PREFABS = tuple(f"fx_explosion_{fuel}_{environment}.prefab"
    for fuel in ("noFuel", "methalox", "solidFuel", "pressurized", "metallicHydrogen") for environment in ("atmosphere", "vacuum"))
CONTEXT_PREFABS = tuple(name+'.prefab' for name in (
    'fx_decouple_atm','fx_decouple_vacuum','fx_launchclamp_atm','fx_launchclamp_vacuum',
    'fx_parachutedeploy_atm','fx_parachutedeploy_vacuum','fx_footstep_default',
    'fx_kerbal_poof_default','fx_wheel_default','fx_surfaceImpact_default',
    'fx_surfaceImpact_liquid_noFuel_atmosphere','fx_surfaceImpact_liquid_noFuel_vacuum',
    'fx_surfaceImpact_solid_noFuel_atmosphere','fx_surfaceImpact_solid_noFuel_vacuum',
    'fx_launchfire_default','fx_detachment_noFuel_atmosphere','fx_detachment_noFuel_vacuum',
    'fx_dock_00','fx_undock_00','fx_science_default','fx_science_vacuum','fx_vapor_vortices'))
CONTINUOUS_PREFABS = {'fx_groundblast_atmosphere_01','fx_launchsmoke_default','fx_launchfire_default','fx_wheel_default','fx_vapor_vortices'}

class EngineFxExporter:
    def __init__(self, exporter, directory: Path):
        self.ex = exporter
        self.out = directory
        self.meshes = {}
        self.materials = {}
        self.particle_materials = {}
        self.shaders = {}
        self.textures = {}
        self.engines = {}
        self.audit = []
        self.reentry_settings = {}
        self.vapor_settings = {}
        self.texture_cache = {}
        self.material_cache = {}
        self.mesh_cache = {}
        for folder in ("meshes", "materials", "shaders", "textures", "engines"):
            (directory/folder).mkdir(parents=True, exist_ok=True)

    def texture(self, ptr):
        if not ptr or not ptr.path_id: return None
        obj = ptr.read()
        source = (obj.object_reader.assets_file.name,obj.object_reader.path_id)
        if source in self.texture_cache: return self.texture_cache[source]
        image = obj.image.convert("RGBA")
        b = io.BytesIO(); image.save(b, format="PNG")
        data = b.getvalue()
        key = re.sub(r"[^\w.-]", "_", obj.m_Name) + "_" + hashlib.sha256(data).hexdigest()[:12]
        uri = f"textures/{key}.png"
        if key not in self.textures:
            (self.out/uri).write_bytes(data)
            from UnityPy.export.Texture2DConverter import parse_image_data
            raw=obj.get_image_data();at=0;mips=[];rgba=bytearray()
            for level in range(obj.m_MipCount):
                w,h=max(1,obj.m_Width>>level),max(1,obj.m_Height>>level)
                fmt=obj.m_TextureFormat
                if fmt in (10,12,24,25,26,27): size=max(1,(w+3)//4)*max(1,(h+3)//4)*(8 if fmt in (10,26) else 16)
                elif fmt in (1,3,4,5): size=w*h*{1:1,3:3,4:4,5:4}[fmt]
                else: raise ValueError(f"Add exact mip decoding for texture format {fmt}: {obj.m_Name}")
                if fmt in (1,3,4,5):
                    mip=parse_image_data(raw[at:at+size],w,h,fmt,obj.object_reader.version,obj.object_reader.platform,obj.m_PlatformBlob,flip=False).convert('RGBA')
                    rgba.extend(mip.tobytes())
                at+=size;mips.append([w,h])
            compression={10:'bc1',12:'bc3',24:'bc6',25:'bc7',26:'bc4',27:'bc5'}.get(obj.m_TextureFormat,'rgba')
            raw_uri=f"textures/{key}.texture";(self.out/raw_uri).write_bytes(raw[:at] if compression!='rgba' else rgba)
            self.textures[key] = {"uri":uri,"raw_uri":raw_uri,"mips":mips,"srgb":obj.m_ColorSpace==1,"compression":compression,
                "name":obj.m_Name, "width":image.width,"height":image.height}
        self.texture_cache[source] = key
        return key

    def shader(self, shader):
        name = shader.m_ParsedForm.m_Name
        reentry = name == 'Reentry/Shockwave'
        key = name.split("/")[-1]
        if key in self.shaders: return key
        main_pass = next(p for p in shader.m_ParsedForm.m_SubShaders[0].m_Passes if p.progVertex.m_CommonParameters.m_ConstantBuffers)
        names = {index:name for name,index in main_pass.m_NameIndices}
        blobs = programs(shader)
        assemblies = [(bc,disassemble(bc)) for bc in blobs]
        vs = next((bc,a) for bc,a in assemblies if re.search(r"^vs_",a,re.M))
        fs = next((bc,a) for bc,a in assemblies if re.search(r"^ps_",a,re.M))
        bindings = {}
        textures = {}
        for stage,program in (("vertex",main_pass.progVertex),("fragment",main_pass.progFragment)):
            for buffer in program.m_CommonParameters.m_ConstantBuffers:
                if names.get(buffer.m_NameIndex) == "$Globals":
                    for vector in buffer.m_VectorParams:
                        bindings[names[vector.m_NameIndex]] = [vector.m_Index//4, vector.m_Dim]
            texture_names = {"_DistortionTexture":"distortion_texture", "_NoiseTexture":"noise_texture",
                "_TracesTexture":"traces_texture", "_CameraDepthTexture":"depth_texture",
                "_GrabTexture":"scene_texture", "_GrabTextureMobile":"scene_texture", "_RefractionTex":"distortion_texture"}
            if reentry:texture_names['_HeatGradient']='noise_texture'
            textures[stage] = {}
            for texture in program.m_CommonParameters.m_TextureParams:
                prop = names[texture.m_NameIndex]
                if prop not in texture_names: raise ValueError(f"Unhandled shader texture {name}: {prop}")
                textures[stage][texture.m_Index] = texture_names[prop]
        if reentry:textures['fragment'][0]='occlusion_texture'
        code = wgsl(vs[1],fs[1],textures['vertex'],textures['fragment'],reentry=reentry)
        (self.out/f"shaders/{key}.wgsl").write_text(code,encoding="utf-8")
        for stage,pair in (("vertex",vs),("fragment",fs)):
            (self.out/f"shaders/{key}.{stage}.dxbc").write_bytes(pair[0])
            (self.out/f"shaders/{key}.{stage}.asm").write_text(pair[1],encoding="utf-8")
        state = main_pass.m_State.rtBlend0
        self.shaders[key] = {"name":name,"uri":f"shaders/{key}.wgsl", "bindings":bindings,
            "gamma_colors":[p.m_Name for p in shader.m_ParsedForm.m_PropInfo.m_Props if p.m_Type==0],
            "src_blend":int(state.srcBlend.val),"dst_blend":int(state.destBlend.val),
            "refraction":"HeatRefraction" in name,
            "reentry":reentry,
            "vertex_sha256":hashlib.sha256(vs[0]).hexdigest(),"fragment_sha256":hashlib.sha256(fs[0]).hexdigest()}
        return key

    def particle_material(self, ptr):
        material=ptr.read()
        key=material.m_Name+"_"+str(material.object_reader.path_id)
        if key in self.particle_materials:return key
        clean=lambda v: v if math.isfinite(v) else 0.0
        data={"shader":material.m_Shader.read().m_ParsedForm.m_Name,
            "floats":{k:clean(v) for k,v in material.m_SavedProperties.m_Floats},
            "colors":{k:[clean(c.r),clean(c.g),clean(c.b),clean(c.a)] for k,c in material.m_SavedProperties.m_Colors},
            "textures":{k:self.texture(t.m_Texture) for k,t in material.m_SavedProperties.m_TexEnvs if t.m_Texture.path_id}}
        self.particle_materials[key]=data;return key

    def material(self, ptr):
        material = ptr.read()
        source = (material.object_reader.assets_file.name,material.object_reader.path_id)
        if source in self.material_cache: return self.material_cache[source]
        floats = dict(material.m_SavedProperties.m_Floats)
        colors = {name:[c.r,c.g,c.b,c.a] for name,c in material.m_SavedProperties.m_Colors}
        shader = self.shader(material.m_Shader.read())
        textures = {name:self.texture(te.m_Texture) for name,te in material.m_SavedProperties.m_TexEnvs
            if te.m_Texture and te.m_Texture.path_id}
        data = {"name":material.m_Name,"shader":shader,"floats":floats,"colors":colors,"textures":textures}
        key = material.m_Name + "_" + hashlib.sha256(json.dumps(data,sort_keys=True).encode()).hexdigest()[:12]
        self.materials[key] = data
        self.material_cache[source] = key
        return key

    def mesh(self, mesh, reentry=False):
        from UnityPy.helpers.MeshHelper import MeshHandler
        source = (mesh.object_reader.assets_file.name,mesh.object_reader.path_id,reentry)
        if source in self.mesh_cache: return self.mesh_cache[source]
        handler = MeshHandler(mesh); handler.process()
        n = len(handler.m_Vertices)
        # Original object coordinates are preserved: the shader displaces and bends in this frame.
        vertices = []
        color_scale=1.0/255.0 if reentry and handler.m_Colors and any(c>1.0 for color in handler.m_Colors for c in color) else 1.0
        targets = []
        for channel in mesh.m_Shapes.channels[:3]:
            if channel.frameCount != 1: raise ValueError(f"Multi-frame morph target: {mesh.m_Name}")
            shape = mesh.m_Shapes.shapes[channel.frameIndex]
            target = {}
            for delta in mesh.m_Shapes.vertices[shape.firstVertex:shape.firstVertex+shape.vertexCount]:
                target[delta.index] = [delta.vertex.x,delta.vertex.y,delta.vertex.z,delta.normal.x,delta.normal.y,delta.normal.z]
            targets.append(target)
        for i in range(n):
            row = list(handler.m_Vertices[i][:3]) + (list(handler.m_Normals[i][:3]) if handler.m_Normals else [0,0,1]) + (list(handler.m_UV0[i][:2]) if handler.m_UV0 else [0,0])
            for j in range(3):
                if reentry and j==0:
                    row.extend([c*color_scale for c in handler.m_Colors[i][:3]]+[0,0,0] if handler.m_Colors else [0.5,0.5,0.5,0,0,0])
                else:row.extend(targets[j].get(i,[0.0]*6) if j<len(targets) else [0.0]*6)
            vertices.extend(row)
        indices = [v for sub in handler.get_triangles() for triangle in sub for v in triangle]
        data = struct.pack(f"<{len(vertices)}f",*vertices) + struct.pack(f"<{len(indices)}I",*indices)
        key = re.sub(r"[^\w.-]","_",mesh.m_Name) + "_" + hashlib.sha256(data).hexdigest()[:12]
        if key not in self.meshes:
            uri = f"meshes/{key}.bin"
            (self.out/uri).write_bytes(data)
            self.meshes[key] = {"uri":uri,"vertices":n,"indices":len(indices),"stride":104,
                "bounds":[[min(v[j] for v in handler.m_Vertices) for j in range(3)],[max(v[j] for v in handler.m_Vertices) for j in range(3)]],
                "channels":[c.name for c in mesh.m_Shapes.channels]}
        self.mesh_cache[source] = key
        return key

    def engine(self, key):
        env,_ = self.ex.env_for_key(key)
        # Built-in spark primitives and Default-ParticleSystem materials live outside Addressables.
        for name in ('unity default resources','unity_builtin_extra'):
            if name not in env.files:
                env.load_file(str(self.ex.bdir.parents[2] / 'Resources' / name))
        part = key.removesuffix(".prefab")
        layers = []
        for sf in env.assets:
            for root in load_scene(sf):
                if root.name != part: continue
                # Instantiate(prefab, impactPosition, impactRotation) replaces the prefab root's
                # editor pose. Keep its scale and every child transform, not the preview offset.
                crash_origin = mul(trs((0,0,0),(0,0,0,1),tuple(getattr(root.tf.m_LocalScale,c) for c in 'xyz')),
                    inverse(root.world)) if part.startswith('fx_') else None
                manager = root.behaviour("ThrottleVFXManager") or {}
                events = {}
                if part.startswith('rcs_'):
                    for group, nozzle in enumerate(n for n in root.walk() if n.name in ('RCSthruster','rcsPort')):
                        events[nozzle.go.object_reader.path_id] = [[group,10]]
                if part == 'eva_kerbal':
                    for node in root.walk():
                        jetpack = node.behaviour('JetpackVFXManager')
                        if jetpack:
                            transforms = {n.tf.object_reader.path_id:n for n in root.walk()}
                            for group, effect in enumerate(jetpack['MoveEffectObjects']+jetpack['RotateEffectObjects']):
                                visual = transforms[effect['Visual']['m_PathID']]
                                events[visual.go.object_reader.path_id] = [[group,10]]
                for mode in manager.get("FXModeActionEvents",[]):
                    for event in mode["ActionEvents"]:
                        for effect in event["EngineEffects"]:
                            events.setdefault(effect["EffectReference"]["m_PathID"],[]).append([mode["EngineModeIndex"],event["ModeEvent"]])
                def walk(node, inherited):
                    matrix = mul(crash_origin,node.world) if crash_origin else node.world
                    context = events.get(node.go.object_reader.path_id,inherited)
                    control = node.behaviour("ThrottleBlendshapeData")
                    renderer = node.component("SkinnedMeshRenderer") if control else None
                    if renderer and renderer.m_Mesh.path_id:
                        layers.append({"name":node.name,"kind":"mesh","matrix":matrix,"events":context,
                            "mesh":self.mesh(renderer.m_Mesh.read()),"material":self.material(renderer.m_Materials[0]),"control":control})
                    particle = node.component("ParticleSystem")
                    trail = node.component('TrailRenderer')
                    if trail:
                        layers.append({'name':node.name,'kind':'trail','matrix':matrix,'events':context,
                            'material':self.particle_material(trail.m_Materials[0]),'mesh':'',
                            'control':{},'renderer':trail.object_reader.read_typetree()})
                    if particle:
                        # Keep the original module data available, including authored startup/stop bursts.
                        raw = particle.object_reader.read_typetree()
                        pr = node.component("ParticleSystemRenderer")
                        layers.append({"name":node.name,"kind":"particle","matrix":matrix,"events":context,
                            "local_scale":[node.tf.m_LocalScale.x,node.tf.m_LocalScale.y,node.tf.m_LocalScale.z],
                            "material":self.particle_material(pr.m_Materials[0]) if pr and pr.m_Materials and pr.m_Materials[0].path_id else "",
                            "mesh":self.mesh(pr.m_Mesh.read()) if pr and pr.m_RenderMode==4 and pr.m_Mesh.path_id else "",
                            "particle":raw,"renderer":pr.object_reader.read_typetree() if pr else {},
                            "parameter_mappings":(node.behaviour("VFXParticleParameterizer") or {}).get("ParameterMappingsList",[]),
                            "control":node.behaviour("ThrottleParticleSystemData") or {}})
                    for child in node.children: walk(child,context)
                # An explosion is a one-shot Play event, never a continuously running engine effect.
                walk(root,[[0,6],[0,10]] if part in CONTINUOUS_PREFABS else [[0,6]] if part.startswith('fx_') else [])
                from gimbal_metadata import definition, tag_layers
                gimbal = definition(part)
                if gimbal is not None:
                    tag_layers(root, layers, gimbal)
        self.engines[part] = layers
        self.audit.append({"engine":part,"mesh_layers":sum(l['kind']=='mesh' for l in layers),
            "particle_layers":sum(l['kind']=='particle' for l in layers),
            "running_mesh_layers":sum(l['kind']=='mesh' and any(e[1]==10 for e in l['events']) for l in layers)})

    def export(self, reentry=True):
        for key in sorted(self.ex.cat.string_keys()):
            if (key.startswith(("engine_","booster_","rcs_")) or key == 'eva_kerbal.prefab') and key.endswith(".prefab"):
                print(f"  authored FX: {key}",flush=True)
                self.engine(key)
        for key in ('fx_groundblast_atmosphere_01.prefab','fx_launchsmoke_default.prefab',*CRASH_PREFABS,*CONTEXT_PREFABS):
            self.engine(key)
        if reentry:
            from export_reentry_fx import export
            export(self)
        self.write()

    def write(self):
        engine_files = {}
        for name,layers in self.engines.items():
            # Colons name NTFS streams on Windows and disappear from normal archives.
            filename = name.replace(":", "_")
            uri = f"engines/{filename}.json"
            (self.out/uri).write_text(json.dumps(layers,separators=(',',':')),encoding="utf-8")
            engine_files[name] = uri
        settings={}
        env,_=self.ex.env_for_key('VFXGlobalSettings.asset')
        for obj in env.objects:
            if obj.type.name=='MonoBehaviour':
                try: raw=obj.read_typetree()
                except ValueError: continue # Unity built-in behaviours do not carry game typetrees.
                for key in ('MaxThrustForceForNormalizedParameterkN','MaxThrustForceDistanceForNormalizedParameterM','VacuumThresholdDensity'):
                    if key in raw:settings[key]=raw[key]
                if 'VaporHumidityThresholdPercentage' in raw:
                    self.vapor_settings={k:v for k,v in raw.items() if k.startswith('Vapor') or k=='VacuumThresholdDensity'}
        from export_web_assets import ROOT
        humidity={}
        for body in (ROOT/'assets/ksp2/data/celestialbody_json').glob('*.json'):
            source=json.loads(body.read_text(encoding='utf-8'))['data']
            if source.get('useAtmosphereHumidityCurve'):
                humidity[body.stem]=source['BodyAltitudeRelativeHumidityCurve']['fCurve']
        self.vapor_settings['HumidityCurves']=humidity
        data = {"version":3,"context_settings":settings,"reentry_settings":self.reentry_settings,"vapor_settings":self.vapor_settings,"engines":engine_files,"meshes":self.meshes,"materials":self.materials,
            "shaders":self.shaders,"textures":self.textures,"particle_materials":self.particle_materials,"audit":self.audit}
        (self.out/"effects.json").write_text(json.dumps(data,separators=(',',':')),encoding="utf-8")
        (self.out/"coverage.json").write_text(json.dumps(self.audit,indent=2)+"\n",encoding="utf-8")
        print(f"Authored FX: {len(self.engines)} engines, {len(self.meshes)} meshes, {len(self.materials)} materials, {len(self.textures)} full-resolution textures")
