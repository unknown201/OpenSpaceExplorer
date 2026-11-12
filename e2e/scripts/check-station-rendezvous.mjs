// Real-client validation against a local server loaded with iss_assembly's actual flight checkpoint.
// First run tools/build_iss_test_crafts.py and the ose-sim iss_assembly example (see verification note).
import { createRequire } from 'node:module';
import fs from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
const {chromium}=createRequire(import.meta.url)('playwright');

const base=process.env.OSE_BASE_URL||'http://127.0.0.1:7891';
if (!/^http:\/\/(127\.0\.0\.1|localhost):/.test(base)) throw new Error('Use an isolated local ISS test server');
const out=path.resolve(process.env.OSE_TEST_OUTPUT||'.cache/iss-guidance/browser');
await fs.mkdir(out,{recursive:true});
const browser=await chromium.launch({headless:true,channel:'chromium',
  executablePath:process.env.OSE_CHROMIUM_EXECUTABLE,
  args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist','--disable-dawn-features=use_dxc']});
const page=await browser.newPage({viewport:{width:1600,height:1000}});
const errors=[];page.on('pageerror',e=>errors.push(e.message));page.setDefaultTimeout(45_000);
const wait=async fn=>{await page.waitForFunction(fn,null,{timeout:45_000});};
async function join(){
  if(await page.locator('#play').isVisible()) await page.locator('#play').click();
  const join=page.locator('[data-ose="session-join"]').first();await join.waitFor();await join.click();
  const hints=page.getByRole('button',{name:'No more hints',exact:true});if(await hints.isVisible())await hints.click();
}
async function flyDelivery(){
  await page.locator('[data-ose="ksc-tracking"]').click();
  await page.locator('[data-ose="ts-vessel"][data-ose-id="2"]').click();
  await page.locator('[data-ose-ts-fly="2"]').click();
  await page.locator('#flight-hud.open').waitFor();
  await page.waitForFunction(()=>!document.querySelector('#loading')?.classList.contains('show'),null,{timeout:180_000});
  const hints=page.getByRole('button',{name:'No more hints',exact:true});
  if(await hints.isVisible()) await hints.click();
  if(!await page.locator('#rdv-panel').getAttribute('open')){
    if(await page.locator('#rdv-panel').evaluate(e=>!e.open)) await page.locator('#rdv-panel > summary').click();
  }
}
try{
  await page.goto(base);
  await page.locator('[data-ose="player-name"]').fill('ISS rendezvous test');
  await join();await flyDelivery();
  await page.selectOption('#rdv-destination','1');
  await page.locator('#rdv-plan').click();
  await wait(()=>document.querySelector('#rdv-panel')?.getAttribute('data-phase')==='Intercept burn');
  assert.match(await page.locator('#rdv-values').innerText(),/Δv remaining/);
  assert.equal(await page.locator('#rdv-level').inputValue(),'guided');
  await wait(()=>document.querySelector('#fh-throttle-pct')?.textContent==='0%');
  await page.locator('#fh-sas-maneuver').click();
  await wait(()=>document.querySelector('#rdv-hold')?.getAttribute('aria-pressed')==='true');
  assert.match(await page.locator('#fh-sas-maneuver').getAttribute('class'),/on/);
  assert.equal(await page.locator('#fh-throttle-pct').textContent(),'0%','attitude hold does not apply thrust');
  await page.keyboard.down('w');await page.waitForTimeout(200);await page.keyboard.up('w');
  await wait(()=>document.querySelector('#rdv-hold')?.getAttribute('aria-pressed')==='false');
  await page.selectOption('#rdv-level','off');
  await wait(()=>document.querySelector('#rdv-panel')?.getAttribute('data-level')==='off');
  assert.equal(await page.locator('#rdv-destination').inputValue(),'1','ordinary station target retained');
  await page.selectOption('#rdv-level','minimal');
  await page.waitForTimeout(300);
  assert.equal(await page.locator('#rdv-help').isVisible(),false);
  await page.screenshot({path:path.join(out,'intercept-guidance.png')});
  await page.reload();await join();await flyDelivery();
  await wait(()=>document.querySelector('#rdv-destination')?.value==='1');
  assert.equal(await page.locator('#rdv-level').inputValue(),'minimal','preference persists');
  await page.selectOption('#rdv-level','guided');
  await page.screenshot({path:path.join(out,'intercept-after-reload.png')});
  // A real, deliberately early manual burst validates authoritative delta-v tracking.
  await page.locator('#fh-sas-maneuver').click();
  await page.waitForFunction(()=>{
    const labels=[...document.querySelectorAll('#rdv-values dt')];
    return parseFloat(labels.find(e=>e.textContent==='Cue error')?.nextElementSibling?.textContent||'999')<3;
  },null,{timeout:120_000});
  await page.screenshot({path:path.join(out,'maneuver-radial-aligned.png')});
  const remaining=()=>page.evaluate(()=>parseFloat([...document.querySelectorAll('#rdv-values dt')].find(e=>e.textContent==='Δv remaining').nextElementSibling.textContent));
  const before=await remaining();
  await page.keyboard.down('ShiftLeft');await page.waitForTimeout(500);await page.keyboard.up('ShiftLeft');
  await page.waitForTimeout(2000);
  await page.keyboard.down('ControlLeft');await page.waitForTimeout(1000);await page.keyboard.up('ControlLeft');
  await wait(()=>document.querySelector('#fh-throttle-pct')?.textContent==='0%');
  await page.waitForTimeout(500);
  assert.ok(await remaining()<before-0.3,'a manually flown burn reduces remaining delta-v');
  // Launch preparation is optional; the normal launch action still places the selected vehicle on pad.
  await page.locator('#fh-ksc').click();
  await page.locator('[data-ose="ksc-launchpad"]').click();
  await page.locator('[data-ose="craft"]').filter({hasText:'Stock12_StartRocket'}).first().click();
  await page.selectOption('#lp-rdv-destination','1');await page.locator('#lp-rdv-enable').check();
  await page.locator('[data-ose="lp-launch"]').click();
  await wait(()=>document.querySelector('#rdv-panel')?.getAttribute('data-phase')==='Launch plane');
  assert.equal(await page.locator('#fh-throttle-pct').textContent(),'0%');
  assert.match(await page.locator('#rdv-help').textContent(),/Plane opportunity only/);
  await page.waitForFunction(()=>!document.querySelector('#loading')?.classList.contains('show'),null,{timeout:180_000});
  await page.screenshot({path:path.join(out,'launch-plane-guidance.png')});
  assert.deepEqual(errors,[]);
  const report={pass:true,renderer:await page.locator('[data-ose="gpu"]').getAttribute('data-ose-renderer'),
    checks:['actual ISS flight checkpoint','fuel-budgeted advisory intercept','maneuver radial follows rendezvous cue','attitude-only hold','manual steering override','Off retains station','Minimal hides instructions','save/reload preferences and destination','manually flown burn updates delta-v','optional pad guidance','manual throttle and staging preserved'],errors};
  await fs.writeFile(path.join(out,'report.json'),JSON.stringify(report,null,2));
  console.log(JSON.stringify(report));
}catch(error){await page.screenshot({path:path.join(out,'failure.png')});console.error((await page.locator('body').innerText()).slice(-8000));throw error;}
finally{await browser.close();}
