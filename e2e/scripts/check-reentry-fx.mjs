// Original Reentry/Shockwave geometry, arithmetic and upstream radial-depth occlusion on WebGPU.
import {chromium} from 'playwright';
import fs from 'node:fs';import path from 'node:path';import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const out=path.join(root,'.cache/vfx-fix/reentry');fs.mkdirSync(out,{recursive:true});
const browser=await chromium.launch({executablePath:process.env.OSE_CHROMIUM_EXECUTABLE,args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist']});
try {
 const page=await browser.newPage({viewport:{width:650,height:650}}),errors=[];
 page.on('pageerror',e=>errors.push(e.message));page.on('console',m=>{if(m.type()==='error')errors.push(m.text());});
 await page.goto(process.env.OSE_BASE_URL??'http://127.0.0.1:16921');
 await page.locator('#status').filter({hasText:'client loaded'}).waitFor();
 await page.evaluate(async()=>{window.fx=await import('/ose-client.js');await window.fx.fx_fixture_setup();});
 let animated=0;
 for(const part of ['pod_1v_conical_crew','engine_1v_methalox_swivel','fueltank_1v_inline_methalox_1x4']) {
  await page.waitForFunction(part=>{window.fx.fx_fixture_reentry(part,0,1,0);return window.fx.fx_fixture_ready('reentry:'+part);},part,{timeout:30000});
  for(const [name,speed,density] of [['cold',40,1],['vacuum',3000,0],['hot',3000,0.05]]) {
   const count=await page.evaluate(({part,speed,density})=>window.fx.fx_fixture_reentry(part,speed,density,0.2),{part,speed,density});
   assert.ok(name==='hot'?count>0:count===0,`${part}: ${name} produced ${count} layers`);
   await page.locator('#fx-fixture').screenshot({path:path.join(out,`${part}-${name}.png`)});
  }
  const frames=[];
  for(const time of [0.3,1.3]) {
   await page.evaluate(({part,time})=>window.fx.fx_fixture_reentry(part,3000,0.05,time),{part,time});
   fs.writeFileSync(path.join(out,`${part}-${time}.capture.json`),await page.evaluate(()=>window.fx.fx_fixture_capture()));
   for(const type of ['hdr','occlusion']) {
    const encoded=await page.evaluate(async type=>{
     const b=await (type==='hdr'?window.fx.fx_fixture_pixels():window.fx.fx_fixture_occlusion_pixels());
     let s='';for(let i=0;i<b.length;i+=32768)s+=String.fromCharCode(...b.subarray(i,i+32768));return btoa(s);
    },type);
    fs.writeFileSync(path.join(out,`${part}-${time}.${type==='hdr'?'webgpu.rgba16':'occlusion.r16'}`),Buffer.from(encoded,'base64'));
   }
   frames.push(await page.locator('#fx-fixture').screenshot({path:path.join(out,`${part}-animated-${time}.png`)}));
  }
  if(!frames[0].equals(frames[1])) animated++;
  console.log(`${part}: cold/vacuum off, hot shockwave active; original records and occlusion captured`);
 }
 assert.deepEqual(errors,[]);
 assert.ok(animated>0,'original noise animation did not change any shape');
} finally {await browser.close();}
