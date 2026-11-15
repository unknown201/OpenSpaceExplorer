import {chromium} from 'playwright';
import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const base=process.env.OSE_BASE_URL??'http://127.0.0.1:7797';
const out=path.join(root,'.cache/part-controls/visuals');
await fs.mkdir(out,{recursive:true});
const catalog=JSON.parse(await fs.readFile(path.join(root,'assets/own/parts-catalog.json'),'utf8'));
const parts=catalog.filter(p=>p.electric.solar_deployable||p.landing_gear?.retractable||p.deployable?.antenna&&p.deployable?.extendable).map(p=>p.id);
const browser=await chromium.launch({channel:'chromium',headless:true,args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist','--disable-dawn-features=use_dxc']});
const page=await browser.newPage({viewport:{width:700,height:700}});
const errors=[];
page.on('pageerror',e=>errors.push(e.message));
page.on('console',m=>{if(m.type()==='error') errors.push(m.text());});
try {
  await page.goto(base);
  await page.waitForFunction(()=>window.wasmBindings?.light_fixture_draw);
  await page.evaluate(()=>window.wasmBindings.fx_fixture_setup());
  const canvas=page.locator('#fx-fixture');
  for(const part of parts) {
    await page.waitForFunction(p=>window.wasmBindings.rig_fixture_draw(p,0),part);
    await page.waitForTimeout(500);
    const hashes=[];
    for(const fraction of [0,0.5,1]) {
      await page.evaluate(({part,fraction})=>window.wasmBindings.rig_fixture_draw(part,fraction),{part,fraction});
      hashes.push(crypto.createHash('sha256').update(await canvas.screenshot({path:path.join(out,`${part}-${fraction}.png`)})).digest('hex'));
    }
    assert.equal(new Set(hashes).size,3,`${part}: rendered deployment must move`);
    console.log(`${part}: three distinct rendered poses`);
  }
  for(const part of catalog.filter(p=>p.light&&!p.landing_gear).map(p=>p.id)) {
    await page.waitForFunction(p=>window.wasmBindings.light_fixture_draw(p,false),part);
    await page.waitForTimeout(500);
    const hashes=[];
    for(const on of [false,true]) {
      await page.evaluate(({part,on})=>window.wasmBindings.light_fixture_draw(part,on),{part,on});
      hashes.push(crypto.createHash('sha256').update(await canvas.screenshot({path:path.join(out,`${part}-${on?'on':'off'}.png`)})).digest('hex'));
    }
    assert.notEqual(hashes[0],hashes[1],`${part}: powered lamp must change the rendered result`);
    const receiver=[];
    for(const on of [false,true]) {
      await page.evaluate(({part,on})=>window.wasmBindings.light_fixture_receiver(part,on),{part,on});
      receiver.push(crypto.createHash('sha256').update(await canvas.screenshot({path:path.join(out,`${part}-receiver-${on?'on':'off'}.png`)})).digest('hex'));
    }
    assert.notEqual(receiver[0],receiver[1],`${part}: lamp must illuminate another surface`);
    console.log(`${part}: visible emission and illumination of another surface`);
  }
  assert.deepEqual(errors,[]);
  console.log(`PASS: ${parts.length} deployable rigs and seven lamps, production WebGPU path`);
} finally {await browser.close();}
