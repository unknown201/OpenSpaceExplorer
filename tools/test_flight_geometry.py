"""Analytic exporter checks; run with the project .venv Python."""
import json
from pathlib import Path
import unittest
import numpy as np
from export_flight_geometry import box_integrals,mesh_integrals,quaternion

class GeometryTests(unittest.TestCase):
    def test_tetrahedron_integrals_include_translation_and_cross_moments(self):
        vertices=np.array([[0,0,0],[1,0,0],[0,1,0],[0,0,1]],dtype=float)
        faces=np.array([[0,2,1],[0,1,3],[0,3,2],[1,2,3]])
        volume,first,second=mesh_integrals(vertices,faces)
        self.assertAlmostEqual(volume,1/6)
        np.testing.assert_allclose(first,np.ones(3)/24)
        np.testing.assert_allclose(second,(np.eye(3)+np.ones((3,3)))/120)
        offset=np.array([2,-3,4])
        v,f,s=mesh_integrals(vertices+offset,faces)
        self.assertAlmostEqual(v,volume)
        np.testing.assert_allclose(f,first+volume*offset)
        np.testing.assert_allclose(s,second+np.outer(first,offset)+np.outer(offset,first)+volume*np.outer(offset,offset))

    def test_box_affine_mass_and_covariance(self):
        world=np.eye(4);world[:3,:3]=[[0,-1,0],[1,0,0],[0,0,1]];world[:3,3]=[1,2,3]
        v,f,s=box_integrals(world,np.zeros(3),np.array([2,4,6]))
        self.assertAlmostEqual(v,48)
        np.testing.assert_allclose(f/v,[1,2,3])
        np.testing.assert_allclose(s/v-np.outer(f/v,f/v),np.diag([4/3,1/3,3]),atol=1e-14)

    def test_quaternion_handles_half_turn_and_quarter_turn(self):
        for angle in [0,np.pi/2,np.pi]:
            c,s=np.cos(angle),np.sin(angle)
            q=np.array(quaternion([[c,-s,0],[s,c,0],[0,0,1]]))
            expected=np.array([0,0,np.sin(angle/2),np.cos(angle/2)])
            self.assertAlmostEqual(abs(np.dot(q,expected)),1)

    def test_committed_geometry_is_complete_and_physically_valid(self):
        root=Path(__file__).resolve().parents[1]
        parts=json.loads((root/'assets/own/parts-catalog.json').read_text())
        sidecar=json.loads((root/'assets/own/flight-geometry.json').read_text())
        self.assertEqual(len(parts),len(sidecar))
        for p in parts:
            g=p['flight_geometry'];self.assertEqual(g,sidecar[p['id']])
            if g['inertia_per_t'] is not None:
                tensor=np.array([[v[k] for k in 'xyz'] for v in g['inertia_per_t']]).T
                np.testing.assert_allclose(tensor,tensor.T,atol=1e-8)
                self.assertGreater(np.linalg.eigvalsh(tensor).min(),0,p['id'])
            for mode in g['engines']:
                self.assertTrue(mode,p['id']);self.assertAlmostEqual(sum(n['weight'] for n in mode),1,places=5,msg=p['id'])
            for n in [n for m in g['engines'] for n in m]+g['rcs']:
                self.assertAlmostEqual(sum(v*v for v in n['direction'].values()),1,places=6,msg=p['id'])
                self.assertGreater(n['weight'],0)
            if g.get('surface') and p['ctrl_range_deg']>0:
                a=np.array(list(g['surface']['axis'].values()));n=np.array(list(g['surface']['normal'].values()))
                deployment=g['surface'].get('deploy')
                if deployment and deployment['separate']:
                    d=np.array([deployment['axis'][k] for k in 'xyz']);angle=deployment['angle_rad']
                    n=n*np.cos(angle)+np.cross(d,n)*np.sin(angle)+d*np.dot(d,n)*(1-np.cos(angle))
                self.assertLess(abs(a@n),1e-5,p['id'])

if __name__=='__main__':unittest.main()
