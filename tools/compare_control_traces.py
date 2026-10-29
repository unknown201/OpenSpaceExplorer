"""Compare matched 50 Hz flight CSV captures without hiding initial-state differences.

Columns: t,pitch,yaw,roll,throttle,qx,qy,qz,qw,wx,wy,wz,mass_t.
Quaternions and angular velocity must already use the same inertial frame and
handedness. Inputs and initial mass must match. Never use an OSE self-comparison
as evidence of KSP2 runtime parity.
"""
import argparse,csv,json,math
from pathlib import Path

def read(path):
    with open(path,newline='',encoding='utf-8-sig') as stream: rows=[{k:float(v) for k,v in r.items()} for r in csv.DictReader(stream)]
    if len(rows)<2:raise ValueError('trace requires at least two samples')
    for i,r in enumerate(rows):
        if not all(math.isfinite(v) for v in r.values()):raise ValueError('nonfinite trace')
        if i and r['t']<=rows[i-1]['t']:raise ValueError('timestamps must increase')
    return rows

def compare(a,b,angle_deg=0.5,omega=0.01,mass=1e-6):
    if len(a)!=len(b):raise ValueError('sample counts differ; resample captures explicitly before comparing')
    angles=[];rates=[];masses=[]
    for x,y in zip(a,b):
        if abs(x['t']-y['t'])>1e-6:raise ValueError('timestamps differ')
        if any(abs(x[k]-y[k])>1e-6 for k in ['pitch','yaw','roll','throttle']):raise ValueError('applied input traces differ')
        q=lambda r:[r[k] for k in ['qx','qy','qz','qw']]
        qa,qb=q(x),q(y);na=math.sqrt(sum(v*v for v in qa));nb=math.sqrt(sum(v*v for v in qb))
        if min(na,nb)<1e-9:raise ValueError('zero quaternion')
        dot=abs(sum(u*v for u,v in zip(qa,qb))/(na*nb))
        angles.append(math.degrees(2*math.acos(min(1,dot))))
        rates.append(math.sqrt(sum((x[k]-y[k])**2 for k in ['wx','wy','wz'])))
        masses.append(abs(x['mass_t']-y['mass_t']))
    result={'samples':len(a),'duration_s':a[-1]['t']-a[0]['t'],'max_angle_deg':max(angles),'rms_angle_deg':math.sqrt(sum(x*x for x in angles)/len(a)),'max_omega_rad_s':max(rates),'max_mass_t':max(masses)}
    result['passed']=result['max_angle_deg']<=angle_deg and result['max_omega_rad_s']<=omega and result['max_mass_t']<=mass
    return result

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('reference',type=Path);p.add_argument('candidate',type=Path)
    p.add_argument('--angle-deg',type=float,default=0.5);p.add_argument('--omega',type=float,default=0.01);p.add_argument('--mass',type=float,default=1e-6)
    args=p.parse_args();result=compare(read(args.reference),read(args.candidate),args.angle_deg,args.omega,args.mass)
    print(json.dumps(result,indent=2));raise SystemExit(0 if result['passed'] else 1)

if __name__=='__main__':main()
