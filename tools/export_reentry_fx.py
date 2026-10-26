"""Export the original ReentryMesh LOD0 geometry, vertex colors, shockwave shader and settings."""
from pathlib import Path
import json
from UnityPy.classes import PPtr
from export_menu_scene import load_scene
from export_web_assets import ROOT

def export(fx,meshes=True):
    env,_=fx.ex.env_for_key('VFXGlobalSettings.asset')
    settings_obj=None
    for obj in env.objects:
        if obj.type.name!='MonoBehaviour':continue
        try:raw=obj.read_typetree()
        except ValueError:continue
        if 'ReentrySettings' in raw:
            settings_obj=PPtr(**raw['ReentrySettings'],assetsfile=obj.assets_file).deref()
            break
    if settings_obj is None:raise ValueError('Missing original reentry settings')
    settings=settings_obj.read_typetree()
    material=fx.material(PPtr(**settings['ReentryMaterial'],assetsfile=settings_obj.assets_file))
    # PhysicsSettings defaults used by CalculateReentryIntensity (also used by ose-sim thermal).
    settings['FluxScale']=500.0;settings['MachExponent']=3.0;settings['DensityExponent']=0.53
    fx.reentry_settings=settings
    gradients={}
    for body,key in [('Eve','reentry_eve_01.exr'),('Jool','reentry_jool_01.exr'),('Laythe','reentry_laythe_01.exr')]:
        gradient_env,_=fx.ex.env_for_key(key)
        for obj in gradient_env.objects:
            if obj.type.name=='Texture2D' and obj.read().m_Name==key.removesuffix('.exr'):
                gradients[body]=fx.texture(PPtr(m_FileID=0,m_PathID=obj.path_id,assetsfile=obj.assets_file));break
    settings['GradientTextures']=gradients
    if not meshes:return
    parts=json.loads((ROOT/'assets/own/parts-catalog.json').read_text(encoding='utf-8'))
    total=0
    for index,part in enumerate(parts):
        key=part['id'];env,_=fx.ex.env_for_key(key+'.prefab')
        if env is None:continue
        layers=[]
        occluders=set()
        for sf in env.assets:
            for root in load_scene(sf):
                if root.name!=key:continue
                renderers={}
                for node in root.walk():
                    renderer=node.component('MeshRenderer') or node.component('SkinnedMeshRenderer')
                    if renderer:renderers[renderer.object_reader.path_id]=(node,renderer)
                for node in root.walk():
                    reentry=node.behaviour('ReentryMesh')
                    if not reentry:continue
                    for source in reentry['ReentryGroupData']['Sources']:
                        source_id=source['Renderer']['m_PathID']
                        if source_id in occluders or source_id not in renderers:continue
                        occluders.add(source_id)
                        source_node,source_renderer=renderers[source_id]
                        source_filter=source_node.component('MeshFilter')
                        source_mesh=source_filter.m_Mesh if source_filter else source_renderer.m_Mesh
                        try:source_mesh=source_mesh.read()
                        except FileNotFoundError as error:
                            if 'unity default resources' not in str(error):raise
                            env.load_file(str(fx.ex.bdir.parents[2]/'Resources'/'unity default resources'))
                            source_mesh=source_mesh.read()
                        # Some procedural source renderers have an empty mesh until runtime.
                        # Their baked reentry geometry remains available below.
                        if source_mesh.m_VertexData.m_VertexCount==0:continue
                        layers.append({'name':source_node.name,'kind':'occlusion','matrix':source_node.world,'events':[],
                            'mesh':fx.mesh(source_mesh),'material':'','control':{}})
                    candidates=[renderers[p['m_PathID']] for p in reentry['ReentryRenderers'] if p['m_PathID'] in renderers]
                    selected=[p for p in candidates if p[0].name.lower() in ('lod_0','lod0')]
                    if not selected:selected=candidates[:1]
                    for mesh_node,renderer in selected:
                        mesh_filter=mesh_node.component('MeshFilter')
                        mesh_ptr=mesh_filter.m_Mesh if mesh_filter else renderer.m_Mesh
                        layers.append({'name':node.name,'kind':'mesh','matrix':mesh_node.world,'events':[[0,10]],
                            'mesh':fx.mesh(mesh_ptr.read(),reentry=True),'material':material,
                            'control':{'shockwave_distance':reentry['_shockwaveDist']}})
        if layers:
            fx.engines['reentry:'+key]=layers;total+=len(layers)
        if index%40==0:print(f'  reentry meshes: {index}/{len(parts)} parts ({total} groups)',flush=True)
    print(f'  reentry meshes: {len(parts)} parts scanned; {total} render/occlusion groups',flush=True)

if __name__=='__main__':
    import sys
    from export_engine_fx import EngineFxExporter
    from export_web_assets import Exporter,DEFAULT_INSTALL
    directory=ROOT/'assets/web/fx'
    data=json.loads((directory/'effects.json').read_text())
    fx=EngineFxExporter(Exporter(Path(DEFAULT_INSTALL),ROOT/'assets/web',8192,False),directory)
    for name in ('meshes','materials','particle_materials','textures','shaders','audit'):
        setattr(fx,name,data[name])
    fx.shaders.pop('Shockwave',None)
    shader_only='--shader-only' in sys.argv
    context_only='--context-only' in sys.argv
    fx.engines={name:json.loads((directory/uri).read_text()) for name,uri in data['engines'].items() if shader_only or context_only or not name.startswith('reentry:')}
    if context_only:
        fx.engine('fx_vapor_vortices.prefab')
    export(fx,meshes=not (shader_only or context_only))
    fx.write()
