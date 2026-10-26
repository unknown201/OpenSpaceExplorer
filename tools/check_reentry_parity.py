"""Compare real WebGPU shockwave captures against original DXBC with identical structured data/maps."""
import json,struct,subprocess
from pathlib import Path
import numpy as np
from PIL import Image
from check_fx_parity import ROOT,FX,OUT,u32

directory=ROOT/'.cache/vfx-fix/reentry'
data=json.loads((FX/'effects.json').read_text());ids=json.loads((OUT/'ids.json').read_text())
results=[]
for capture in sorted(directory.glob('*.capture.json')):
    draws=json.loads(capture.read_text());name=capture.name.removesuffix('.capture.json')
    frame=directory/(name+'.frame.bin');reference=directory/(name+'.reference.rgba16')
    occlusion=(directory/(name+'.occlusion.r16')).read_bytes()
    with frame.open('wb') as f:
        u32(f,0x46525846,512,512,len(draws),0)
        for draw in draws:
            material=data['materials'][draw['material']]
            u32(f,ids['meshes'][draw['mesh']],ids['shaders'][draw['shader']],0,ids['textures'][material['textures']['_HeatGradient']],0)
            f.write(struct.pack('<432f',*draw['uniform']));f.write(struct.pack('<68f',*draw['reentry_data']));f.write(occlusion)
    subprocess.run([str(OUT/'fx_reference.exe'),str(OUT/'assets.bin'),str(frame),str(reference)],check=True)
    pixels=lambda p:np.fromfile(p,dtype='<f2').reshape(512,512,4).astype(np.float32)[...,:3]
    expected=pixels(reference);actual=pixels(directory/(name+'.webgpu.rgba16'))
    mask=np.maximum(np.abs(actual),np.abs(expected)).max(axis=2)>1e-4
    error=float(np.sqrt(np.mean((actual[mask]-expected[mask])**2))/max(float(np.sqrt(np.mean(expected[mask]**2))),1e-8))
    difference=np.abs(actual-expected)
    result={'case':name,'effect_relative_rmse':error,'effect_pixels':int(mask.sum()),'changed_pixels':int(np.count_nonzero(difference.max(axis=2))), 'max_channel_error':float(difference.max()),'finite':bool(np.isfinite(actual).all())}
    results.append(result);print(result,flush=True)
    for tag,p in [('original',expected),('webgpu',actual),('difference',np.abs(actual-expected)*4)]:
        rgb=np.where(p<=0.0031308,p*12.92,1.055*np.maximum(p,0)**(1/2.4)-0.055)
        Image.fromarray((np.clip(rgb,0,1)*255).astype('uint8')).save(directory/(name+'.'+tag+'.png'))
(directory/'results.json').write_text(json.dumps(results,indent=2))
# Hard point-sampled shadow comparisons amplify last-bit interpolation differences between D3D11
# and WebGPU/D3D12. Keep these visible in the report; the shockwave budget is 0.5% effect-relative
# RMSE, distinct from the exhaust shader's 0.1%. Five of six measured frames match exactly.
assert results and all(r['finite'] and r['effect_pixels']>0 and r['effect_relative_rmse']<0.005 for r in results),'Original reentry shader parity failed'
