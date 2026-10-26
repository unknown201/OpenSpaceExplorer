// Exercise real server-authoritative flight, a fueled crash and a solid-booster burnout through the UI.
import {chromium} from 'playwright';
import fs from 'node:fs';import path from 'node:path';import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const out=path.join(root,'.cache/vfx-fix/live-flight');fs.mkdirSync(out,{recursive:true});
const browser=await chromium.launch({executablePath:process.env.OSE_CHROMIUM_EXECUTABLE,args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist']});
try {
 const page=await browser.newPage({viewport:{width:1280,height:850}}),errors=[],requests=[];
 page.on('pageerror',e=>errors.push(e.message));page.on('console',m=>{if(m.type()==='error')errors.push(m.text());});
 page.on('request',r=>{if(r.url().endsWith('flame.glb'))requests.push(r.url());});
 await page.goto(process.env.OSE_BASE_URL??'http://127.0.0.1:16922');
 await page.locator('#status').filter({hasText:'client loaded'}).waitFor();
 await page.locator('[data-ose="player-name"]').fill('particle regression');
 await page.locator('[data-ose="play"]').click();
 await page.locator('[data-ose="session-join"]').first().waitFor();
 if(!(await page.locator('#create-name').isVisible()))await page.locator('#create-details > summary').click();
 await page.selectOption('#create-mode','Sandbox');await page.fill('#create-name','Particle regression');
 await page.locator('[data-ose="create"]').click();await page.locator('[data-ose="ksc"]').waitFor({state:'visible'});
 const build=async(parts,name)=>{
  await page.locator('[data-ose="ksc-vab"]').click();await page.locator('[data-ose="vab-clear"]').click();
  for(const part of parts){await page.locator('[data-ose="vab-search"]').fill(part);await page.locator('[data-ose="vab-part"]').first().click();}
  await page.locator('[data-ose="vab-name"]').fill(name);await page.locator('[data-ose="vab-launch"]').click();
  await page.locator('[data-ose="flight"].open').waitFor();await page.waitForTimeout(3500);
  if(await page.locator('#ftue-off').isVisible())await page.locator('#ftue-off').click();
 };
 const state=()=>page.locator('[data-ose="flight"]').evaluate(e=>({
  altitude:Number(e.getAttribute('data-ose-altitude')),situation:e.getAttribute('data-ose-situation'),
  particles:Number(e.getAttribute('data-ose-particles')),assets:e.getAttribute('data-ose-effect-assets')??''
 }));
 await build(['Tin Can','FL-T400','LV-T45'],'Impact test');
 const start=await state();await page.locator('[data-ose="flight-throttle-full"]').click();await page.locator('[data-ose="flight-stage"]').click();
 await page.waitForFunction(alt=>Number(document.querySelector('[data-ose="flight"]').getAttribute('data-ose-altitude'))>alt+35,start.altitude,{timeout:30000});
 await page.locator('[data-ose="flight-throttle-cut"]').click();
 await page.waitForFunction(()=>document.querySelector('[data-ose="flight"]').getAttribute('data-ose-situation')==='Crashed',null,{timeout:45000});
 let peak=0,explosion=false;
 for(let tick=0;tick<60;tick++){
  const s=await state();peak=Math.max(peak,s.particles);explosion ||= s.assets.includes('fx_explosion_');
  if(tick===0)assert.equal(await page.locator('#flight-report.open').count(),0,'the crash report must allow the explosion to play first');
  if([0,4,12].includes(tick))await page.screenshot({path:path.join(out,`impact-${tick}.png`)});
  await page.waitForTimeout(50);
 }
 assert.ok(explosion&&peak>0,`crash did not render its burst: peak=${peak}`);
 console.log(`fueled live crash: original explosion recipe active, peak ${peak} particles`);
 await page.locator('#fh-ksc').click();await page.locator('[data-ose="ksc"]').waitFor({state:'visible'});
 await build(['Tin Can','Flea'],'Burnout test');
 await page.locator('[data-ose="flight-throttle-full"]').click();await page.locator('[data-ose="flight-stage"]').click();
 await page.waitForFunction(()=>document.querySelector('[data-ose="flight"]').getAttribute('data-ose-effect-assets')?.includes('booster_1v_solid_flea'),null,{timeout:20000});
 await page.locator('[data-ose="flight-throttle-cut"]').click();await page.waitForTimeout(600);
 assert.ok((await state()).assets.includes('booster_1v_solid_flea'),'a fuel-fed solid must keep its exhaust with the lever at zero');
 await page.waitForFunction(()=>!document.querySelector('[data-ose="flight"]').getAttribute('data-ose-effect-assets')?.includes('booster_1v_solid_flea'),null,{timeout:70000});
 await page.screenshot({path:path.join(out,'solid-burnout.png')});
 assert.equal(requests.length,0,'the legacy flame cone must never be requested');
 assert.deepEqual(errors,[]);console.log('solid burnout: exhaust follows fuel-fed thrust; legacy cone never requested');
 fs.writeFileSync(path.join(out,'results.json'),JSON.stringify({crashPeak:peak,explosion,legacyConeRequests:requests,errors},null,2));
} finally {await browser.close();}
