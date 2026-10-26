// Real WebGPU crash recipes, including their delayed emissions and once-only lifecycle.
// Run against a local build after tools/export_particles.py.
import {chromium} from 'playwright';
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const out=path.join(root,'.cache/vfx-fix/browser');
fs.mkdirSync(out,{recursive:true});
const browser=await chromium.launch({executablePath:process.env.OSE_CHROMIUM_EXECUTABLE,args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist']});
try {
  for(const fueled of [false,true]) for(const pressure of [101.325,0]) {
    const page=await browser.newPage({viewport:{width:650,height:650}});
    const errors=[];
    page.on('pageerror',e=>errors.push(e.message));
    page.on('console',m=>{if(m.type()==='error')errors.push(m.text());});
    await page.goto(process.env.OSE_BASE_URL??'http://127.0.0.1:16920');

    await page.locator("#status").filter({hasText:"client loaded"}).waitFor();
    await page.evaluate(async()=>{window.fx=await import("/ose-client.js");await window.fx.default();await window.fx.fx_fixture_setup();});
    // Normal flight preloads crash resources without emitting an explosion.
    for(let i=0;i<60;i++) {
      assert.equal(await page.evaluate(({fueled,pressure})=>window.fx.fx_fixture_crash(false,fueled,pressure,0),{fueled,pressure}),0);
      await page.waitForTimeout(50);
    }
    const counts=[];
    const name=`${fueled?'fueled':'dry'}-${pressure?'atmosphere':'vacuum'}`;
    for(let tick=1;tick<=1050;tick++) {
      const count=await page.evaluate(({pressure,tick})=>window.fx.fx_fixture_crash(true,false,pressure,tick*0.02),{pressure,tick});
      counts.push(count);
      if([5,15,35,75].includes(tick)) {
        await page.locator('#fx-fixture').screenshot({path:path.join(out,`${name}-${tick}.png`)});
      }
    }
    assert.ok(Math.max(...counts)>0,`${name}: crash must emit visible particles`);
    assert.ok(counts.slice(900).every(n=>n===0),`${name}: explosion must expire without replaying`);
    assert.deepEqual(errors,[],`${name}: browser errors`);
    console.log(`${name}: peak ${Math.max(...counts)} particles; delayed layers rendered; expired without replay`);
    fs.writeFileSync(path.join(out,`${name}.counts.json`),JSON.stringify(counts));
    await page.close();
  }
  const page=await browser.newPage();
  await page.goto(process.env.OSE_BASE_URL??'http://127.0.0.1:16920');

  await page.locator("#status").filter({hasText:"client loaded"}).waitFor();
    await page.evaluate(async()=>{window.fx=await import("/ose-client.js");await window.fx.default();await window.fx.fx_fixture_setup();});
  for(let i=0;i<60;i++) {
    await page.evaluate(()=>window.fx.fx_fixture_draw('engine_1v_methalox_swivel',1,101.325,0,0,35,0.4));
    await page.waitForTimeout(50);
  }
  for(let tick=1;tick<=50;tick++) await page.evaluate(t=>window.fx.fx_fixture_draw('engine_1v_methalox_swivel',1,101.325,t*0.02,0,35,0.4),tick);
  for(let tick=51;tick<=850;tick++) {
    const layers=await page.evaluate(t=>window.fx.fx_fixture_draw('engine_1v_methalox_swivel',0,101.325,t*0.02,0,35,0.4),tick);
    assert.equal(layers,0,'exhaust mesh must disappear immediately at cutoff');
  }
  assert.equal(await page.evaluate(()=>window.fx.fx_fixture_particle_count()),0,'remaining particles must expire after cutoff');
  await page.locator('#fx-fixture').screenshot({path:path.join(out,'engine-cutoff.png')});
  console.log('engine cutoff: zero exhaust layers immediately; zero particles after expiry');
} finally {await browser.close();}
