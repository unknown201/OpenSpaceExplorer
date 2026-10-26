// Authored contextual recipes and every independently controlled RCS/EVA nozzle, on real WebGPU.
import {chromium} from 'playwright';
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const out=path.join(root,'.cache/vfx-fix/flight-audit');fs.mkdirSync(out,{recursive:true});
const library=JSON.parse(fs.readFileSync(path.join(root,'assets/web/fx/effects.json')));
const browser=await chromium.launch({executablePath:process.env.OSE_CHROMIUM_EXECUTABLE,args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist']});
try {
 const page=await browser.newPage({viewport:{width:650,height:650}}),errors=[];
 page.on('pageerror',e=>errors.push(e.message));
 page.on('console',m=>{if(m.type()==='error')errors.push(m.text());});
 await page.goto(process.env.OSE_BASE_URL??'http://127.0.0.1:16921');
 await page.locator('#status').filter({hasText:'client loaded'}).waitFor();
 await page.evaluate(async()=>{window.fx=await import('/ose-client.js');await window.fx.fx_fixture_setup();});
 const results=[];
 for(const part of Object.keys(library.engines).filter(p=>p.startsWith('fx_')||p.startsWith('rcs_')||p==='eva_kerbal')) {
  const layers=JSON.parse(fs.readFileSync(path.join(root,'assets/web/fx',library.engines[part])));
  const modes=[...new Set(layers.flatMap(l=>l.events.map(e=>e[0])))];
  await page.waitForFunction(part=>{window.fx.fx_fixture_draw(part,0,101.325,0,0,35,0.4);return window.fx.fx_fixture_ready(part);},part,{timeout:30000});
  for(const mode of modes) {
   const continuous=layers.some(l=>l.events.some(e=>e[0]===mode&&e[1]===10));
   const expected=layers.filter(l=>l.kind==='mesh'&&l.events.some(e=>e[0]===mode&&e[1]===10)).length;
   const result=await page.evaluate(async({part,mode,continuous})=>{
    window.fx.fx_fixture_reset();
    const draw=(throttle,t)=>part.startsWith('fx_')?window.fx.fx_fixture_draw(part,throttle,101.325,t,0,35,0.4):window.fx.fx_fixture_group(part,throttle,101.325,t,mode);
    draw(0,0);let peak=0,mesh=0;
    for(let tick=1;tick<=(part.includes('surfaceImpact')?2500:800);tick++) {
     const layers=draw(tick<150||!continuous?1:0,tick*0.02);
     if(tick===25)mesh=layers;
     peak=Math.max(peak,window.fx.fx_fixture_particle_count());
     if(tick%40===0)await new Promise(r=>setTimeout(r,0));
    }
    return {peak,mesh,remaining:window.fx.fx_fixture_particle_count()};
   },{part,mode,continuous});
   assert.equal(result.mesh,expected,`${part} mode ${mode}: original mesh layers`);
   // Launch smoke intentionally requires nonzero normalized thrust, supplied by the flight scene.
   const emitter=layers.some(l=>l.kind==='particle'&&l.particle.EmissionModule.enabled);
   if(emitter&&!['fx_launchsmoke_default','fx_launchfire_default'].includes(part)) assert.ok(result.peak+result.mesh>0,`${part} mode ${mode}: no visible effect`);
   assert.equal(result.remaining,0,`${part} mode ${mode}: particles failed to expire`);
   results.push({part,mode,...result});
  }
  // Capture an early frame after a clean restart at the correct preview distance.
  await page.evaluate(part=>{window.fx.fx_fixture_reset();window.fx.fx_fixture_draw(part,0,101.325,0,0,35,0.4);for(let i=1;i<=12;i++)window.fx.fx_fixture_draw(part,1,101.325,i*0.02,0,35,0.4);},part);
  await page.locator('#fx-fixture').screenshot({path:path.join(out,`${part}.png`)});
  console.log(`${part}: ${modes.length} independent group(s) verified`);
 }
 await page.waitForFunction(()=>{window.fx.fx_fixture_vapor(0,false);return window.fx.fx_fixture_ready('fx_vapor_vortices');},null,{timeout:30000});
 const vapor=await page.evaluate(async()=>{
  window.fx.fx_fixture_reset();window.fx.fx_fixture_vapor(0,false);
  let peak=0;for(let i=1;i<=50;i++)peak=Math.max(peak,window.fx.fx_fixture_vapor(i*0.02,true));
  return peak;
 });
 assert.ok(vapor>0,'humid-air wingtip ribbon did not form');
 await page.locator('#fx-fixture').screenshot({path:path.join(out,'wingtip-wet.png')});
 const dry=await page.evaluate(()=>{let count=0;for(let i=51;i<=100;i++)count=window.fx.fx_fixture_vapor(i*0.02,false);return count;});
 assert.equal(dry,0,'wingtip ribbon did not expire in dry air/vacuum');
 console.log(`wingtip condensation: ${vapor} ribbon segments in humid air; zero after cutoff`);
 assert.deepEqual(errors,[]);
 fs.writeFileSync(path.join(out,'results.json'),JSON.stringify(results,null,2));
 console.log(`${results.length} effect/group lifecycle checks passed without GPU errors`);
} finally {await browser.close();}
