import copy
import unittest
from apply_control_capture_geometry import apply


class CaptureGeometryTests(unittest.TestCase):
    def fixture(self):
        craft={'parts':[{'guid':'p','part_id':'wing','local_position':dict(x=0,y=0,z=0),
                         'local_rotation':dict(x=0,y=0,z=0,w=1)}]}
        row=dict(part_id='wing',px=0,py=0,pz=0,qx=0,qy=0,qz=0,qw=1,iqx=0,iqy=0,iqz=0,iqw=1,
                 com_x=1,com_y=2,com_z=3,dry_t=2,ix=5,iy=10,iz=15)
        return craft,row

    def test_unscales_runtime_tensor_without_changing_mass_or_mount(self):
        craft,row=self.fixture();pose=copy.deepcopy(craft['parts'][0]['local_position'])
        apply(craft,[row],scalar=5)
        f=craft['parts'][0]['flight']
        self.assertEqual(f['dry_mass_t'],2)
        self.assertEqual(f['inertia_per_t'],[dict(x=1,y=0,z=0),dict(x=0,y=2,z=0),dict(x=0,y=0,z=3)])
        self.assertEqual(craft['parts'][0]['local_position'],pose)

    def test_rejects_different_craft_and_invalid_measurements(self):
        for key,value in [('part_id','other'),('px',1),('qw',0),('dry_t',float('nan')),('ix',-1)]:
            craft,row=self.fixture();row[key]=value
            with self.subTest(key=key),self.assertRaises(ValueError):apply(craft,[row])


if __name__=='__main__':unittest.main()
