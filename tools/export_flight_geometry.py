"""Export flight force transforms and collider mass integrals from the user's KSP2 install.

Writes a committed geometry sidecar and updates the committed catalog. Read-only on the install.
Mesh inertia uses the closed authored collision mesh; unsupported/open shapes are audited.
"""
from pathlib import Path
import argparse
import json
import math
import numpy as np
from export_web_assets import Exporter, DEFAULT_INSTALL, ROOT
from export_menu_scene import load_scene
from gimbal_metadata import visual_names


def vector(v):
    return np.array([getattr(v, c) for c in 'xyz'], dtype=float)


def obj(v):
    return dict(zip('xyz', map(float, v)))


def quaternion(matrix):
    m=np.array(matrix, dtype=float)[:3,:3]
    m=m / np.linalg.norm(m,axis=0)
    # Davenport's symmetric eigenproblem avoids singular trace branches at half turns.
    k=np.array([[m[0,0]-m[1,1]-m[2,2],m[0,1]+m[1,0],m[0,2]+m[2,0],m[2,1]-m[1,2]],
                [m[0,1]+m[1,0],m[1,1]-m[0,0]-m[2,2],m[1,2]+m[2,1],m[0,2]-m[2,0]],
                [m[0,2]+m[2,0],m[1,2]+m[2,1],m[2,2]-m[0,0]-m[1,1],m[1,0]-m[0,1]],
                [m[2,1]-m[1,2],m[0,2]-m[2,0],m[1,0]-m[0,1],np.trace(m)]])
    _,v=np.linalg.eigh(k)
    q=v[:,-1]
    return (q if q[3]>=0 else -q).tolist()


def mesh_integrals(vertices, triangles):
    """Signed tetrahedra: volume, first moment and raw second moment, exact for closed meshes."""
    v=np.asarray(vertices,dtype=float);f=np.asarray(triangles,dtype=int).reshape(-1,3)
    a,b,c=v[f[:,0]],v[f[:,1]],v[f[:,2]]
    volumes=np.einsum('ij,ij->i',a,np.cross(b,c))/6
    s=a+b+c
    volume=volumes.sum()
    if abs(volume)<1e-10: raise ValueError('zero-volume collision mesh')
    first=np.einsum('i,ij->j',volumes,s)/4
    second=sum(np.einsum('i,ij,ik->jk',volumes,u,u) for u in [a,b,c,s])/20
    if volume<0: volume,first,second=-volume,-first,-second
    return volume,first,second


def box_integrals(world, center, size):
    w=np.asarray(world);c=w[:3,:3]@center+w[:3,3]
    volume=abs(np.linalg.det(w[:3,:3]))*float(np.prod(size))
    covariance=w[:3,:3]@np.diag(size*size/12)@w[:3,:3].T
    return volume,volume*c,volume*(covariance+np.outer(c,c))


def extract(ex, part, source):
    env,_=ex.env_for_key(part+'.prefab')
    roots=[r for sf in env.assets for r in load_scene(sf) if r.name==part] if env else []
    out={'mass_offset_m':source.get('coMassOffset',dict(x=0.,y=0.,z=0.)),
         'pressure_offset_m':source.get('coPressureOffset',dict(x=0.,y=0.,z=0.)),
         'lift_offset_m':source.get('coLiftOffset',dict(x=0.,y=0.,z=0.)),
         'control_rotation':None,'inertia_per_t':None,'inertia_source':'bounds_fallback','engines':[],'rcs':[],'gimbals':[]}
    if not roots:return out
    root=roots[0];nodes=list(root.walk());parents={id(c):n for n in nodes for c in n.children}
    modules={m['Name']:m['ModuleData'][0]['DataObject'] for m in source.get('serializedPartModules',[]) if m.get('ModuleData')}
    docking=modules.get('PartComponentModule_DockingNode')
    if docking:
        control=next((n for n in nodes if n.name==docking.get('ControlTransformName','controlNode')),None)
        if control is not None:out['control_rotation']=quaternion(control.world)
    surface=next((e['DataObject'] for m in source.get('serializedPartModules',[]) for e in m.get('ModuleData',[]) if e['Name']=='Data_ControlSurface'),None)
    if surface:
        hinge=next((n for n in nodes if n.name==surface.get('CtrlSurfacePivotTransformName')),None)
        axis=np.eye(3)['XYZ'.index(surface.get('CtrlTransformRotAxis','X'))]
        normal=np.eye(3)['XYZ'.index(surface.get('CtrlTransformDir','Z'))]*surface.get('CtrlTransformSign',-1.)
        if hinge is not None:
            w=np.array(hinge.world);axis=w[:3,:3]@axis;lift=next((n for n in nodes if n.name==surface.get('CtrlSurfaceLiftTransformName')),hinge);normal=np.array(lift.world)[:3,:3]@normal;axis/=np.linalg.norm(axis);normal/=np.linalg.norm(normal);pivot=w[:3,3]
        else:
            pivot=np.array([surface.get('CtrlSurfacePivotOffset',{}).get(c,0.) for c in 'xyz'])
            # Procedural parts instantiate a separate mesh option at runtime. Their
            # neutral lift axis is part-space -X and span is Z; do not use a missing
            # bone's untransformed local axes (which would rotate about the normal).
            if '_procedural_' in part:
                normal=np.array([-1.,0.,0.]);axis=np.array([0.,0.,1.])
        if hinge is not None and '_procedural_' in part:
            visual=np.array([[0.,-1.,0.],[1.,0.,0.],[0.,0.,1.]])
            axis=visual@axis;normal=visual@normal;pivot=visual@pivot
        deploy_name=surface.get('CtrlSurfaceDeployTransformName','default')
        separate=deploy_name not in ('default',surface.get('CtrlSurfacePivotTransformName'))
        deploy_node=next((n for n in nodes if n.name==deploy_name),None) if separate else hinge
        deploy_axis=np.eye(3)['XYZ'.index(surface.get('DeployTransformRotAxis','X'))]
        if deploy_node is not None:
            parent=parents.get(id(deploy_node));parent_world=np.array(parent.world) if parent is not None else np.eye(4)
            deploy_axis=parent_world[:3,:3]@deploy_axis;deploy_axis/=np.linalg.norm(deploy_axis)
            deploy_pivot=np.array(deploy_node.world)[:3,3]
        else:deploy_axis=axis;deploy_pivot=pivot
        deploy={'axis':obj(deploy_axis),'pivot_m':obj(deploy_pivot),'separate':separate,
            'angle_rad':math.radians(surface.get('CtrlSurfaceDeployAngle',90.) if surface.get('UseCtrlSurfaceDeployAngle',False) else surface.get('CtrlSurfaceRange',15.)),
            'min_control_angle_rad':math.radians(surface.get('CtrlSurfaceMinimumLockAngleForControl',0.))}
        out['surface']={'deploy':deploy,'fixed_lift_coeff':0. if surface.get('DisableLiftingSurfaceForce',False) else None,
            'moving_lift_coeff':surface.get('CtrlSurfaceArea',0.) if surface.get('DisableLiftingSurfaceForce',False) else None,'normal':obj(normal),'axis':obj(axis),'pivot_m':obj(pivot),'speed':surface.get('ActuatorSpeedNormalScale',10.),
            'exponential':surface.get('UseExponentialSpeed',True),'ignore_sas':surface.get('IgnoreSASInputs',False)}
    config=modules.get('PartComponentModule_Gimbal');gimbal_nodes=[]
    if config:
        name=config.get('gimbalTransformName','thrustTransform');names=[name]
        if 'faux' in name.lower() or name=='thrustTransform':
            indices={id(n):i for i,n in enumerate(nodes)}
            names=visual_names([{'name':n.name,'children':[indices[id(c)] for c in n.children]} for n in nodes]) or names
        gimbal_nodes=[n for n in nodes if n.name in names]
        for n in gimbal_nodes:
            base=config.get('gimbalRange',10.)
            limits=[config.get(k,-1.) for k in ['gimbalRangeXP','gimbalRangeXN','gimbalRangeYP','gimbalRangeYN']]
            out['gimbals'].append({'position_m':obj(np.array(n.world)[:3,3]),'rotation':quaternion(n.world),
                'limits_rad':[math.radians(x if x>0 else base) for x in limits],
                'response_speed':config.get('gimbalResponseSpeed',10.) if config.get('useGimbalResponseSpeed',True) else None,
                'axis_mult':[config.get('xMult',1.),config.get('yMult',1.)]})
    def nozzle(n,axis,weight):
        w=np.array(n.world);direction=w[:3,:3]@axis;direction/=np.linalg.norm(direction)
        cur=n;owner=None
        while cur is not None:
            if cur in gimbal_nodes: owner=gimbal_nodes.index(cur);break
            cur=parents.get(id(cur))
        return {'position_m':obj(w[:3,3]),'direction':obj(direction),'weight':weight,'gimbal':owner}
    for mode in modules.get('PartComponentModule_Engine',{}).get('engineModes',[]):
        entries=mode.get('ThrustTransformNamesMultipliers') or []
        if entries:
            jets=[nozzle(n,np.array([0.,0.,-1.]),e['ThrustTransformMultiplier']) for e in entries for n in nodes if n.name==e['ThrustTransformName']]
        else:
            selected=[n for n in nodes if n.name==mode.get('thrustVectorTransformName','thrustTransform')]
            jets=[nozzle(n,np.array([0.,0.,-1.]),1/len(selected)) for n in selected]
        out['engines'].append(jets)
    if 'PartComponentModule_RCS' in modules:
        r=modules['PartComponentModule_RCS']
        out['rcs_tuning']={'use_lever':r.get('useLever',False),'full_thrust':r.get('fullThrust',{}).get('storedValue',False),'full_thrust_min':r.get('fullThrustMin',0.2),'shielded_can_activate':r.get('shieldedCanActivate',False)}
        out['rcs']=[nozzle(n,np.array([0.,-1.,0.]),1.) for n in nodes if n.name in ('RCSthruster','rcsPort')]
    integrals=[];audit=[]
    for n in nodes:
        cur=n;active=True
        while cur is not None:
            active &= cur.active;cur=parents.get(id(cur))
        if not active:continue
        for ptr in n.components():
            kind=ptr.type.name
            if kind not in ('BoxCollider','MeshCollider','SphereCollider','CapsuleCollider'):continue
            collider=ptr.read()
            if not collider.m_Enabled or collider.m_IsTrigger:continue
            w=np.array(n.world,dtype=float)
            try:
                if kind=='BoxCollider':integrals.append(box_integrals(w,vector(collider.m_Center),vector(collider.m_Size)))
                elif kind=='SphereCollider':
                    c=w[:3,:3]@vector(collider.m_Center)+w[:3,3];r=collider.m_Radius
                    volume=4*math.pi*r**3/3*abs(np.linalg.det(w[:3,:3]));cov=w[:3,:3]@np.eye(3)*(r*r/5)@w[:3,:3].T
                    integrals.append((volume,volume*c,volume*(cov+np.outer(c,c))))
                elif kind=='MeshCollider':
                    if not collider.m_Mesh or not collider.m_Mesh.path_id:continue
                    mesh=collider.m_Mesh.read()
                    positions,_,_,triangles=ex.mesh_arrays(mesh,None)
                    vertices=np.array(positions).reshape(-1,3);vertices[:,0]*=-1
                    # mesh_arrays reflects X and reverses winding; restore both to Unity.
                    triangles=np.array(triangles).reshape(-1,3)[:,::-1]
                    world_vertices=vertices@w[:3,:3].T+w[:3,3]
                    world_vertices,remap=np.unique(np.round(world_vertices,10),axis=0,return_inverse=True)
                    triangles=remap[triangles]
                    if collider.m_Convex:
                        from scipy.spatial import ConvexHull
                        hull=ConvexHull(world_vertices)
                        triangles=hull.simplices.copy()
                        for face,equation in zip(triangles,hull.equations):
                            a,b,c=world_vertices[face]
                            if np.dot(np.cross(b-a,c-a),equation[:3])<0: face[[1,2]]=face[[2,1]]
                    edges=np.sort(np.concatenate([triangles[:,[0,1]],triangles[:,[1,2]],triangles[:,[2,0]]]),axis=1)
                    _,counts=np.unique(edges,axis=0,return_counts=True)
                    if not np.all(counts==2):raise ValueError('open/non-manifold collision mesh')
                    integrals.append(mesh_integrals(world_vertices,triangles))
                else:
                    r=collider.m_Radius;length=max(0.,collider.m_Height-2*r)
                    cylinder=math.pi*r*r*length;hemisphere=2*math.pi*r**3/3;volume=cylinder+2*hemisphere
                    axial=cylinder*r*r/2+2*hemisphere*2*r*r/5
                    transverse=cylinder*(3*r*r+length*length)/12+2*hemisphere*(83*r*r/320+(length/2+3*r/8)**2)
                    inertia=np.full(3,transverse);inertia[collider.m_Direction]=axial
                    covariance=np.diag((inertia.sum()/2-inertia)/volume)
                    c=w[:3,:3]@vector(collider.m_Center)+w[:3,3]
                    covariance=w[:3,:3]@covariance@w[:3,:3].T
                    volume*=abs(np.linalg.det(w[:3,:3]))
                    integrals.append((volume,volume*c,volume*(covariance+np.outer(c,c))))
            except (ValueError,IndexError,AttributeError) as e:audit.append(str(e))
    if integrals and not audit:
        volume=sum(i[0] for i in integrals);first=sum((i[1] for i in integrals),np.zeros(3));second=sum((i[2] for i in integrals),np.zeros((3,3)))
        com=np.array([out['mass_offset_m'][c] for c in 'xyz'])
        covariance=second/volume-np.outer(first/volume,com)-np.outer(com,first/volume)+np.outer(com,com)
        inertia=np.eye(3)*np.trace(covariance)-covariance
        if np.linalg.eigvalsh(inertia).min()>0:
            out['inertia_per_t']=[obj(inertia[:,i]) for i in range(3)];out['inertia_source']='authored_colliders'
    if audit:out['inertia_source']='bounds_fallback: '+', '.join(sorted(set(audit)))
    return out


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--install',default=DEFAULT_INSTALL);args=parser.parse_args()
    catalog=ROOT/'assets/own/parts-catalog.json';parts=json.loads(catalog.read_text(encoding='utf-8'))
    ex=Exporter(Path(args.install),ROOT/'assets/web',1024,False);result={}
    for i,p in enumerate(parts):
        source=json.loads((ROOT/'assets/ksp2/data/parts'/f"{p['id']}.json").read_text(encoding='utf-8'))['data']
        result[p['id']]=extract(ex,p['id'],source);p['flight_geometry']=result[p['id']]
        ex._envs.clear()
        if i%25==0:print(i,p['id'],flush=True)
    (ROOT/'assets/own/flight-geometry.json').write_text(json.dumps(result,separators=(',',':')),encoding='utf-8')
    catalog.write_text(json.dumps(parts,separators=(',',':')),encoding='utf-8')
    print('Exported',len(result),'parts;',sum(v['inertia_per_t'] is not None for v in result.values()),'collider inertias')


if __name__=='__main__':main()
