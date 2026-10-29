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
    if 'dt' in rows[0]:validate_intervals(rows)
    return rows

def validate_intervals(rows):
    """Reject missing/unaligned live physics intervals rather than fitting a time shift."""
    for i,row in enumerate(rows):
        if row['dt']<=0:raise ValueError('nonpositive physics interval')
        if not i:continue
        previous=rows[i-1]
        if abs(row['t']-previous['t']-previous['dt'])>1e-6:
            raise ValueError('noncontiguous physics intervals')
        if row['fixed_count']!=previous['fixed_count']+1:
            raise ValueError('missing physics tick')
        if any(abs(row[k]-previous['end_'+k])>1e-8 for k in ['wx','wy','wz']):
            raise ValueError('live angular velocity is discontinuous between intervals')

def unity_to_ose(rows):
    """Explicit Z reflection, preserving time, inputs and the original rows."""
    result=[]
    for row in rows:
        converted=dict(row)
        # Quaternion and angular velocity are axial; position/velocity are polar.
        for key in ['qx','qy','wx','wy','pz','vz','end_qx','end_qy','end_wx','end_wy',
                    'cached_wx','cached_wy','torque_x','torque_y']:
            if key in converted: converted[key]=-converted[key]
        result.append(converted)
    return result

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
    p.add_argument('--reference-unity',action='store_true',help='Reflect a recorder capture from Unity into the OSE main-body frame; never aligns time or pose')
    args=p.parse_args();reference=read(args.reference)
    if args.reference_unity:
        metadata=Path(str(args.reference)+'.metadata.txt').read_text()
        fields=dict(line.split('=',1) for line in metadata.splitlines() if '=' in line)
        if fields.get('handedness')!='Unity' or fields.get('frame')!='main_body_motion_frame':
            raise ValueError('Unity conversion requires the recorder main-body-frame metadata')
        reference=unity_to_ose(reference)
    result=compare(reference,read(args.candidate),args.angle_deg,args.omega,args.mass)
    print(json.dumps(result,indent=2));raise SystemExit(0 if result['passed'] else 1)

if __name__=='__main__':main()
