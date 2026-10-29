import unittest
from compare_control_traces import compare, unity_to_ose, validate_intervals, read
from pathlib import Path
import hashlib,json

class TraceTests(unittest.TestCase):
    def rows(self):
        return [dict(t=t,pitch=0,yaw=0,roll=0,throttle=0,qx=0,qy=0,qz=0,qw=1,wx=0,wy=0,wz=0,mass_t=10) for t in [0,0.02]]
    def test_quaternion_double_cover(self):
        a=self.rows();b=self.rows()
        for r in b:r['qw']=-1
        self.assertTrue(compare(a,b)['passed'])
    def test_spin_error_fails(self):
        a=self.rows();b=self.rows();b[1]['wz']=0.1
        self.assertFalse(compare(a,b)['passed'])
    def test_inputs_and_time_cannot_be_silently_aligned(self):
        a=self.rows();b=self.rows();b[1]['pitch']=1
        with self.assertRaises(ValueError):compare(a,b)
        b=self.rows();b[1]['t']=0.04
        with self.assertRaises(ValueError):compare(a,b)
    def test_mass_difference_fails(self):
        a=self.rows();b=self.rows();b[1]['mass_t']=9.9
        self.assertFalse(compare(a,b)['passed'])
    def test_live_interval_evidence_is_contiguous_and_immutable(self):
        fixture=Path(__file__).parent/'ksp2-control-capture/fixtures/jumping-flea-interval.csv'
        rows=read(fixture)
        self.assertEqual(len(rows),600)
        manifest=json.loads(fixture.with_name('jumping-flea-interval-manifest.json').read_text())
        for name,digest in manifest['files'].items():
            self.assertEqual(hashlib.sha256((fixture.parent/name).read_bytes()).hexdigest(),digest)
        changed=[dict(row) for row in rows];changed[1]['wx']+=0.1
        with self.assertRaisesRegex(ValueError,'discontinuous'):validate_intervals(changed)
        changed=[dict(row) for row in rows];changed[1]['fixed_count']+=1
        with self.assertRaisesRegex(ValueError,'missing physics tick'):validate_intervals(changed)
        changed=[dict(row) for row in rows];changed[1]['t']+=0.001
        with self.assertRaisesRegex(ValueError,'noncontiguous'):validate_intervals(changed)
    def test_unity_reflection_distinguishes_axial_and_polar_vectors(self):
        row=dict(t=2,pitch=1,yaw=-1,roll=0.5,px=1,py=2,pz=3,vx=4,vy=5,vz=6,
                 qx=0.1,qy=0.2,qz=0.3,qw=0.9,wx=7,wy=8,wz=9,
                 end_wx=1,end_wy=2,end_wz=3,cached_wx=4,cached_wy=5,cached_wz=6,
                 torque_x=7,torque_y=8,torque_z=9,end_qx=0.1,end_qy=0.2,end_qz=0.3,end_qw=0.9)
        converted=unity_to_ose([row])[0]
        self.assertEqual([converted[k] for k in ['px','py','pz','vx','vy','vz']],[1,2,-3,4,5,-6])
        self.assertEqual([converted[k] for k in ['wx','wy','wz']],[ -7,-8,9])
        self.assertEqual([converted[k] for k in ['qx','qy','qz','qw']],[-0.1,-0.2,0.3,0.9])
        self.assertEqual([converted[k] for k in ['end_wx','end_wy','end_wz']],[-1,-2,3])
        self.assertEqual([converted[k] for k in ['cached_wx','cached_wy','cached_wz']],[-4,-5,6])
        self.assertEqual([converted[k] for k in ['torque_x','torque_y','torque_z']],[-7,-8,9])
        self.assertEqual([converted[k] for k in ['t','pitch','yaw','roll']],[2,1,-1,0.5])
        self.assertEqual(unity_to_ose([converted]),[row])
        self.assertEqual(row['pz'],3)

if __name__=='__main__':unittest.main()
