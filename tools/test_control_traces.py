import unittest
from compare_control_traces import compare, unity_to_ose

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
    def test_unity_reflection_distinguishes_axial_and_polar_vectors(self):
        row=dict(t=2,pitch=1,yaw=-1,roll=0.5,px=1,py=2,pz=3,vx=4,vy=5,vz=6,
                 qx=0.1,qy=0.2,qz=0.3,qw=0.9,wx=7,wy=8,wz=9)
        converted=unity_to_ose([row])[0]
        self.assertEqual([converted[k] for k in ['px','py','pz','vx','vy','vz']],[1,2,-3,4,5,-6])
        self.assertEqual([converted[k] for k in ['wx','wy','wz']],[ -7,-8,9])
        self.assertEqual([converted[k] for k in ['qx','qy','qz','qw']],[-0.1,-0.2,0.3,0.9])
        self.assertEqual([converted[k] for k in ['t','pitch','yaw','roll']],[2,1,-1,0.5])
        self.assertEqual(unity_to_ose([converted]),[row])
        self.assertEqual(row['pz'],3)

if __name__=='__main__':unittest.main()
