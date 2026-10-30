import unittest
from compare_control_traces import compare, unity_to_ose, validate_intervals, read
from pathlib import Path
import csv,hashlib,json,math

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
    def test_translation_and_enable_inputs_cannot_be_silently_changed(self):
        for key in ['translate_x','translate_y','translate_z','rcs']:
            a=self.rows();b=self.rows();a[1][key]=1
            for row in a+b:row.update(px=0,py=0,pz=0,vx=0,vy=0,vz=0)
            with self.assertRaisesRegex(ValueError,'RCS input'):compare(a,b)
    def test_translation_requires_and_checks_linear_motion(self):
        a=self.rows();b=self.rows()
        for rows in [a,b]:
            for row in rows:
                row.update(translate_x=1,translate_y=0,translate_z=0,rcs=1)
        with self.assertRaisesRegex(ValueError,'position and linear velocity'):compare(a,b)
        for rows in [a,b]:
            for row in rows:row.update(px=0,py=0,pz=0,vx=0,vy=0,vz=0)
        self.assertTrue(compare(a,b)['passed'])
        self.assertTrue(compare(a,b)['linear_motion_checked'])
        b[1]['px']=0.51
        self.assertFalse(compare(a,b)['passed'])
        b[1]['px']=0;b[1]['vz']=0.051
        self.assertFalse(compare(a,b)['passed'])
    def test_rcs_evidence_is_contiguous_and_immutable(self):
        root=Path(__file__).parent/'ksp2-control-capture/fixtures'
        manifest=json.loads((root/'gumball-rcs-manifest.json').read_text())
        for name,digest in manifest['files'].items():
            self.assertEqual(hashlib.sha256((root/name).read_bytes()).hexdigest(),digest)
        for name in ['gumball-rcs','gumball-rcs-repeat']:
            rows=read(root/(name+'.csv'))
            self.assertEqual(len(rows),600)
            self.assertTrue(all(r['rcs']==1 and r['sas']==0 for r in rows))
    def test_live_two_nozzle_capture_charges_one_shared_request(self):
        root=Path(__file__).parent/'ksp2-control-capture/fixtures'
        for name in ['gumball-rcs','gumball-rcs-repeat']:
            primary=read(root/(name+'.csv'))
            with (root/(name+'.csv.rcs.csv')).open(newline='') as f:
                nozzles=[{k:float(v) for k,v in r.items()} for r in csv.DictReader(f)]
            self.assertEqual(len(nozzles),600*8)
            checked=0
            for i in range(1,600):
                previous=nozzles[(i-1)*8:i*8];current=nozzles[i*8:(i+1)*8]
                active=lambda rows:[r for r in rows if sum(r[k]**2 for k in ['fx','fy','fz'])>0.5]
                if len(active(previous))!=2 or len(active(current))!=2:continue
                # Last active nozzle owns the common handle; two applied forces do
                # not consume two units of this flow. No OSE trace is used here.
                last=active(previous)[-1]
                flow=math.sqrt(sum(last[k]**2 for k in ['fx','fy','fz']))/(240*9.80665)
                self.assertAlmostEqual(previous[0]['requirement'],flow,delta=1e-8)
                drawn=previous[0]['remaining']-current[0]['remaining']
                self.assertAlmostEqual(drawn,flow*primary[i]['dt'],delta=1e-9)
                checked+=1
            self.assertGreater(checked,100)
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
                 torque_x=7,torque_y=8,torque_z=9,end_qx=0.1,end_qy=0.2,end_qz=0.3,end_qw=0.9,
                 end_px=10,end_py=20,end_pz=30,end_vx=40,end_vy=50,end_vz=60,
                 rcs_force_x=1,rcs_force_y=2,rcs_force_z=3,rcs_torque_x=4,rcs_torque_y=5,rcs_torque_z=6)
        converted=unity_to_ose([row])[0]
        self.assertEqual([converted[k] for k in ['px','py','pz','vx','vy','vz']],[1,2,-3,4,5,-6])
        self.assertEqual([converted[k] for k in ['wx','wy','wz']],[ -7,-8,9])
        self.assertEqual([converted[k] for k in ['qx','qy','qz','qw']],[-0.1,-0.2,0.3,0.9])
        self.assertEqual([converted[k] for k in ['end_wx','end_wy','end_wz']],[-1,-2,3])
        self.assertEqual([converted[k] for k in ['cached_wx','cached_wy','cached_wz']],[-4,-5,6])
        self.assertEqual([converted[k] for k in ['torque_x','torque_y','torque_z']],[-7,-8,9])
        self.assertEqual([converted[k] for k in ['end_px','end_py','end_pz','end_vx','end_vy','end_vz']],[10,20,-30,40,50,-60])
        self.assertEqual([converted[k] for k in ['rcs_force_x','rcs_force_y','rcs_force_z']],[1,2,-3])
        self.assertEqual([converted[k] for k in ['rcs_torque_x','rcs_torque_y','rcs_torque_z']],[-4,-5,6])
        self.assertEqual([converted[k] for k in ['t','pitch','yaw','roll']],[2,1,-1,0.5])
        self.assertEqual(unity_to_ose([converted]),[row])
        self.assertEqual(row['pz'],3)

if __name__=='__main__':unittest.main()
