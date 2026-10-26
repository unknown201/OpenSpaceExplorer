// Captures deterministic production-renderer frames for the original DXBC comparison tool.
import {chromium} from 'playwright';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const out=path.join(root,'.cache/fx-parity');fs.mkdirSync(out,{recursive:true});
const base=process.env.OSE_BASE_URL??'http://127.0.0.1:7794';
const browser=await chromium.launch({executablePath:process.env.OSE_CHROMIUM_EXECUTABLE,args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist']});
try {
 const page=await browser.newPage({viewport:{width:700,height:700}});
 const errors=[];
 page.on('console',m=>{if(m.type()==='error')errors.push(m.text());});
 page.on('pageerror',e=>{throw e;});
 await page.goto(base);
 await page.locator('#status').filter({hasText:'client loaded'}).waitFor();
 await page.evaluate(async()=>{window.fxFixture=await import('/ose-client.js');await window.fxFixture.fx_fixture_setup();});
 const savePixels=async name=>{
  const encoded=await page.evaluate(async()=>{const bytes=await window.fxFixture.fx_fixture_pixels();let s='';for(let i=0;i<bytes.length;i+=32768)s+=String.fromCharCode(...bytes.subarray(i,i+32768));return btoa(s);});
  fs.writeFileSync(path.join(out,`${name}.webgpu.rgba16`),Buffer.from(encoded,'base64'));
 };
 await page.evaluate(()=>window.fxFixture.fx_fixture_draw('',0,0,1.25,0,10,0));
 await savePixels('background');
 const library=JSON.parse(fs.readFileSync(path.join(root,'assets/web/fx/effects.json'),'utf8'));
 const cases=[];
 for(const [part,center,distance] of [
  ['engine_1v_methalox_swivel',-7,32],['engine_1v_methalox_terrier',-7,32],
  ['engine_3v_hydrogen_swerv',-22,95],['booster_1v_solid_flea',-10,50],
  ['engine_0v_xenon_dawn',-1,8],['engine_1v_turbofan_methane_panther',-6,30]]) {
  for(const pressure of [101.325,1,0])for(const throttle of [1,0.35])cases.push({part,center,distance,pressure,throttle,time:1.25,yaw:0.4});
 }
 for(const c of cases){
  const layers=JSON.parse(fs.readFileSync(path.join(root,'assets/web/fx',library.engines[c.part]),'utf8'));
  const expected=layers.filter(l=>l.kind==='mesh'&&l.events.some(e=>e[0]===0&&e[1]===10)).length;
  let count=0;
  for(let i=0;i<250;i++){
   count=await page.evaluate(c=>window.fxFixture.fx_fixture_draw(c.part,c.throttle,c.pressure,c.time,c.center,c.distance,c.yaw),c);
   if(count===expected)break;
   await page.waitForTimeout(50);
  }
  if(count!==expected)throw new Error(`${c.part}: ${count}/${expected} layers ready`);
  const name=`${c.part}-p${c.pressure}-t${c.throttle}`;
  const capture=await page.evaluate(()=>window.fxFixture.fx_fixture_capture());
  fs.writeFileSync(path.join(out,`${name}.capture.json`),capture);
  await savePixels(name);
  await page.locator('#fx-fixture').screenshot({path:path.join(out,`${name}.bloom.png`)});
  console.log(`${name}: ${count} original layers captured`);
 }
 if(errors.length)throw new Error(errors.join('\n'));
}finally{await browser.close();}
