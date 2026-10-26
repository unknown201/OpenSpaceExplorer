"""Package the original assets for fx_reference.cpp and compare its HDR pixels to WebGPU captures."""
from __future__ import annotations
import argparse,json,struct,subprocess
from pathlib import Path
import numpy as np
from PIL import Image,ImageDraw,ImageFont
ROOT=Path(__file__).resolve().parent.parent
FX=ROOT/'assets/web/fx'
OUT=ROOT/'.cache/fx-parity'

def u32(f,*values):f.write(struct.pack('<'+'I'*len(values),*values))

def prepare():
    data=json.loads((FX/'effects.json').read_text())
    OUT.mkdir(parents=True,exist_ok=True)
    textures=list(data['textures']);shaders=list(data['shaders']);meshes=list(data['meshes'])
    ids={'textures':{k:i+1 for i,k in enumerate(textures)},'shaders':{k:i for i,k in enumerate(shaders)},'meshes':{k:i for i,k in enumerate(meshes)}}
    (OUT/'ids.json').write_text(json.dumps(ids))
    with (OUT/'assets.bin').open('wb') as f:
        u32(f,0x41525846,len(textures)+1,len(shaders),len(meshes))
        u32(f,1,1,1,28,4);f.write(bytes([255]*4))
        for key in textures:
            spec=data['textures'][key];compression=spec['compression'];srgb=spec['srgb']
            formats={'bc1':72 if srgb else 71,'bc3':78 if srgb else 77,'bc4':80,'bc5':83,'bc6':95,'bc7':99 if srgb else 98,'rgba':29 if srgb else 28}
            u32(f,*spec['mips'][0],len(spec['mips']),formats[compression]);raw=(FX/spec['raw_uri']).read_bytes();at=0
            for w,h in spec['mips']:
                size=w*h*4 if compression=='rgba' else max(1,(w+3)//4)*max(1,(h+3)//4)*(8 if compression in ('bc1','bc4') else 16)
                u32(f,size);f.write(raw[at:at+size]);at+=size
        for key in shaders:
            u32(f,int(data['shaders'][key]['refraction']) | (int(data['shaders'][key].get('reentry',False))<<1))
            for stage in ('vertex','fragment'):
                b=(FX/f'shaders/{key}.{stage}.dxbc').read_bytes();u32(f,len(b));f.write(b)
        for key in meshes:
            spec=data['meshes'][key];u32(f,spec['vertices'],spec['indices']);f.write((FX/spec['uri']).read_bytes())

def compare():
    data=json.loads((FX/'effects.json').read_text());ids=json.loads((OUT/'ids.json').read_text())
    background_frame=OUT/'background.frame.bin';background_reference=OUT/'background.reference.rgba16'
    with background_frame.open('wb') as f:u32(f,0x46525846,512,512,0,1)
    subprocess.run([str(OUT/'fx_reference.exe'),str(OUT/'assets.bin'),str(background_frame),str(background_reference)],check=True)
    def pixels(path):return np.fromfile(path,dtype='<f2').reshape(512,512,4).astype(np.float32)[...,:3]
    background=pixels(background_reference)
    assert np.array_equal(background,pixels(OUT/'background.webgpu.rgba16')),'Fixture backgrounds differ before effects'
    results=[]
    for capture in sorted(OUT.glob('*.capture.json')):
        draws=sorted(json.loads(capture.read_text()),key=lambda d:not d['refraction'])
        frame=capture.with_suffix('.frame.bin');reference=capture.with_suffix('.reference.rgba16')
        with frame.open('wb') as f:
            u32(f,0x46525846,512,512,len(draws),1)
            for draw in draws:
                material=data['materials'][draw['material']]
                textures=material['textures'];distortion=textures.get('_DistortionTexture',textures.get('_RefractionTex'))
                u32(f,ids['meshes'][draw['mesh']],ids['shaders'][draw['shader']],*[ids['textures'].get(key,0) for key in (distortion,textures.get('_NoiseTexture'),textures.get('_TracesTexture'))])
                assert len(draw['uniform'])==432
                f.write(struct.pack('<432f',*draw['uniform']))
        subprocess.run([str(OUT/'fx_reference.exe'),str(OUT/'assets.bin'),str(frame),str(reference)],check=True)
        name=capture.name.removesuffix('.capture.json')
        actual=pixels(OUT/f'{name}.webgpu.rgba16')
        expected=pixels(reference)
        delta=np.abs(actual-expected)
        mask=np.maximum(np.abs(actual-background),np.abs(expected-background)).max(axis=2)>1e-4
        relative=float(np.sqrt(np.mean((actual[mask]-expected[mask])**2))/max(np.sqrt(np.mean((expected[mask]-background[mask])**2)),1e-8))
        result={'case':name,'effect_relative_rmse':relative,'max_error':float(delta.max()),'mean_effect_error':float(delta[mask].mean()),'effect_pixels':int(mask.sum()),'finite':bool(np.isfinite(actual).all())}
        results.append(result);print(result)
        for tag,image in [('webgpu',actual),('original',expected),('difference',delta*4)]:
            srgb=np.where(image<=0.0031308,image*12.92,1.055*np.maximum(image,0)**(1/2.4)-0.055)
            Image.fromarray((np.clip(srgb,0,1)*255).astype('uint8')).save(OUT/f'{name}.{tag}.png')
    (OUT/'results.json').write_text(json.dumps(results,indent=2)+'\n')
    assert results,'No GPU fixture captures found'
    assert all(r['finite'] and r['effect_pixels']>0 and r['effect_relative_rmse']<0.001 for r in results),'Original shader parity failed'
    contact_sheet(results)

def contact_sheet(results):
    selected=[('Swivel, sea level','engine_1v_methalox_swivel-p101.325-t1'),
        ('Swivel, vacuum','engine_1v_methalox_swivel-p0-t1'),
        ('SWERV, vacuum','engine_3v_hydrogen_swerv-p0-t1'),
        ('Flea, sea level','booster_1v_solid_flea-p101.325-t1'),
        ('Dawn, vacuum','engine_0v_xenon_dawn-p0-t1'),
        ('Panther, sea level','engine_1v_turbofan_methane_panther-p101.325-t1')]
    sheet=Image.new('RGB',(930,80+len(selected)*344),(18,22,30));draw=ImageDraw.Draw(sheet)
    font=ImageFont.load_default(size=16)
    draw.text((16,12),'Original engine shaders vs OSE | HDR before bloom | full throttle',font=font,fill='white')
    for column,label in enumerate(['Original DXBC / D3D11','OSE / WebGPU','Absolute difference x4']):
        draw.text((16+column*310,47),label,font=font,fill=(190,205,220))
    lookup={r['case']:r for r in results}
    for row,(label,name) in enumerate(selected):
        y=80+row*344;error=lookup[name]['effect_relative_rmse']*100
        draw.text((16,y),f'{label} | effect RMSE {error:.5f}%',font=font,fill='white')
        for column,tag in enumerate(['original','webgpu','difference']):
            preview=Image.open(OUT/f'{name}.{tag}.png').resize((300,300),Image.Resampling.LANCZOS)
            sheet.paste(preview,(10+column*310,y+28))
    sheet.save(OUT/'comparison.png')

if __name__=='__main__':
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument('--prepare',action='store_true');a=ap.parse_args()
    prepare() if a.prepare else compare()
