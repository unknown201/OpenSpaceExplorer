// Inspect an iss_assembly checkpoint in the real client. Use only a separate localhost test server.
import {createRequire} from 'node:module';
import fs from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
const {chromium}=createRequire(import.meta.url)('playwright');
const base=process.env.OSE_BASE_URL||'http://127.0.0.1:7892';
if(!/^http:\/\/(localhost|127\.0\.0\.1):/.test(base))throw Error('Use an isolated local checkpoint server');
const out=path.resolve(process.env.OSE_TEST_OUTPUT||'.cache/iss-validation/browser');
await fs.mkdir(out,{recursive:true});
const browser=await chromium.launch({headless:true,channel:'chromium',executablePath:process.env.OSE_CHROMIUM_EXECUTABLE,
  args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist','--disable-dawn-features=use_dxc']});
const page=await browser.newPage({viewport:{width:1920,height:1080}});
page.setDefaultTimeout(60_000);const errors=[];page.on('pageerror',e=>errors.push(e.message));
const assets=[];page.on('response',r=>{if(/solararray|\.ose/.test(r.url()))assets.push({url:r.url(),status:r.status()});});
page.on('console',m=>{if(m.type()==='warning'||m.type()==='error')console.log(m.type(),m.text());});
try{
  if(process.env.OSE_MODEL_OVERRIDE){
    for(const id of ['solararray_3v_large','solararray_1v_1x6']){
      await page.route(`**/assets/parts/${id}.glb`,r=>r.fulfill({path:path.join(process.env.OSE_MODEL_OVERRIDE,'parts',`${id}.glb`),contentType:'model/gltf-binary'}));
    }
  }
  await page.goto(base);
  await page.locator('[data-ose="player-name"]').fill('ISS assembly inspection');
  if(await page.locator('#play').isVisible())await page.locator('#play').click();
  await page.locator('[data-ose="session-join"]').first().click();
  const hints=page.getByRole('button',{name:'No more hints',exact:true});
  if(await hints.isVisible())await hints.click();
  await page.locator('[data-ose="ksc-tracking"]').click();
  const vessel=process.env.OSE_CAPTURE_VESSEL||'1';
  await page.locator(`[data-ose="ts-vessel"][data-ose-id="${vessel}"]`).click();
  await page.locator(`[data-ose-ts-fly="${vessel}"]`).click();
  await page.waitForFunction(()=>document.querySelector('#flight-hud')?.classList.contains('open')&&!document.querySelector('#loading')?.classList.contains('show'),null,{timeout:180_000});
  if(await hints.isVisible())await hints.click();
  if(process.env.OSE_CAPTURE_PORT){
    await page.locator('#rdv-panel > summary').click();
    await page.selectOption('#rdv-destination','1');
    await page.selectOption('#rdv-own-port',process.env.OSE_CAPTURE_OWN_PORT);
    await page.selectOption('#rdv-target-port',process.env.OSE_CAPTURE_PORT);
    await page.locator('#rdv-plan').click();
    await page.waitForFunction(()=>['Match velocity','Port alignment'].includes(document.querySelector('#rdv-panel')?.getAttribute('data-phase')));
    assert.match(await page.locator('#rdv-values').innerText(),/Port range/);
  }
  await page.waitForTimeout(2000);
  await fs.writeFile(path.join(out,'asset-responses.json'),JSON.stringify(assets,null,2));
  assert.equal(await page.locator('[data-ose="gpu"]').getAttribute('data-ose-renderer'),'webgpu');
  const name=process.env.OSE_CAPTURE_PORT?'iss-approach':'iss-completed';
  await page.screenshot({path:path.join(out,`${name}.png`)});
  // Orbit the actual flight camera to inspect the assembled topology from a second angle.
  await page.mouse.move(1100,500);await page.mouse.down({button:'right'});
  await page.mouse.move(1360,650,{steps:24});await page.mouse.up({button:'right'});
  await page.waitForTimeout(1000);
  await page.screenshot({path:path.join(out,`${name}-rotated.png`)});
  assert.deepEqual(errors,[]);console.log(JSON.stringify({pass:true,vessel,renderer:'webgpu',errors}));
}catch(error){await page.screenshot({path:path.join(out,'capture-failure.png')});throw error;}
finally{await browser.close();}
