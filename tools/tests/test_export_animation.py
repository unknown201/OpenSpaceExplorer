"""Synthetic exporter regressions: no game install or UnityPy required."""
import math
from pathlib import Path
import struct
import sys
from types import SimpleNamespace as NS
import unittest
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from export_animation import sampled_components, export_rig
from export_web_assets import Glb


def clip_data():
    raw = bytearray()
    for time, coeff in [(-math.inf, [0.,0.,0.,1.]), (0., [0.,0.,2.,1.]), (1., [0.,0.,0.,3.])]:
        raw.extend(struct.pack("<fII4f", time, 1, 0, *coeff))
    raw.extend(struct.pack("<fI", math.inf, 0))
    words = list(struct.unpack(f"<{len(raw)//4}I",raw))
    return {"m_Name":"open", "m_SampleRate":2., "m_MuscleClip":{
        "m_StartTime":0.,"m_StopTime":2.,"m_Clip":{"data":{
            "m_StreamedClip":{"data":words,"curveCount":1},
            "m_DenseClip":{"m_CurveCount":1,"m_FrameCount":3,"m_BeginTime":0.,"m_SampleRate":1.,"m_SampleArray":[2.,4.,6.]},
            "m_ConstantClip":{"data":[9.]}}}},
        "m_ClipBindingConstant":{"genericBindings":[{"typeID":4,"attribute":1,"path":zlib.crc32(b'bone')}]}}


class ExportAnimationTests(unittest.TestCase):
    def test_stream_dense_constant_index_spaces_and_terminal_values(self):
        times,rows=sampled_components(clip_data())
        self.assertEqual(times,[0.,0.5,1.,1.5,2.])
        self.assertEqual(rows,[[1.,2.,9.],[2.,3.,9.],[3.,4.,9.],[3.,5.,9.],[3.,6.,9.]])

    def test_times_are_unique_after_float32_encoding(self):
        c=clip_data()
        c['m_MuscleClip']['m_StopTime']=1.0000000001
        times,_=sampled_components(c)
        self.assertEqual(times,[0.,0.5,1.])

    def test_unbound_fbx_animator_cannot_steal_live_prefab_bones(self):
        def ptr(i):return NS(path_id=i)
        transforms={i:NS(m_GameObject=ptr(i),m_Father=ptr(parent)) for i,parent in [(1,0),(2,1),(3,0),(4,3)]}
        names={1:'prefab',2:'bone',3:'fbx',4:'bone'}
        def obj(kind,read=None,tree=None,identity=0):
            return NS(type=NS(name=kind),read=lambda:read,read_typetree=lambda:tree,path_id=identity)
        env=NS(objects=[obj('Animator',NS(m_GameObject=ptr(1),m_Controller=ptr(10))),
                        obj('Animator',NS(m_GameObject=ptr(3),m_Controller=ptr(0))),
                        obj('AnimationClip',tree=clip_data(),identity=20)])
        glb=Glb()
        glb.nodes=[{'name':names[i]} for i in range(1,5)]
        audit=export_rig(env,glb,transforms,names,{i:i-1 for i in transforms},[])
        self.assertEqual(audit['ose_animation_audit']['unresolved_bindings'],[])
        self.assertEqual(glb.animations[0]['channels'][0]['target'],{'node':1,'path':'translation'})
        a=glb.accessors[glb.animations[0]['samplers'][0]['output']]
        off=glb.buffer_views[a['bufferView']]['byteOffset']
        self.assertEqual(struct.unpack_from('<3f',glb.bin,off),(-1.,2.,9.))


if __name__=='__main__':unittest.main()
