import unittest
from compare_control_traces import compare

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

if __name__=='__main__':unittest.main()
