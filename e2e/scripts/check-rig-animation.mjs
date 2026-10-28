// Runs against a local server with exported assets. Uses the production GLB loader, shader and draw path.
import {chromium} from 'playwright';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const out=path.join(root,'.cache/animation-audit/browser');
fs.mkdirSync(out,{recursive:true});
const browser=await chromium.launch({channel:'chromium',executablePath:process.env.OSE_CHROMIUM_EXECUTABLE,args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist',...(process.platform==='win32'?['--disable-dawn-features=use_dxc']:[])]});
try {
  const page=await browser.newPage({viewport:{width:650,height:650}});
  const errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  page.on('console',m=>{if(m.type()==='error') errors.push(m.text());});
  await page.goto(process.env.OSE_BASE_URL??'http://127.0.0.1:7796');
  await page.waitForFunction(()=>window.wasmBindings?.rig_fixture_draw);
  await page.evaluate(async()=>{window.rigFixture=window.wasmBindings;await window.rigFixture.fx_fixture_setup();});
  for(const part of ['solararray_2v_accordion','landinggear_1v','landingleg_0v','engine_1v_methalox_swivel','engine_2v_methalox_poodle','engine_3v_methalox_mammoth']) {
    await page.waitForFunction(part=>window.rigFixture.rig_fixture_draw(part,0),part,{timeout:30000});
    // Wait for the asynchronously fetched materials before comparing deterministic poses.
    await page.waitForTimeout(1500);
    const hashes=[];
    for(const fraction of [0,0.5,1]) {
      await page.evaluate(({part,fraction})=>window.rigFixture.rig_fixture_draw(part,fraction),{part,fraction});
      const png=await page.locator('#fx-fixture').screenshot({path:path.join(out,`${part}-${fraction}.png`)});
      hashes.push(crypto.createHash('sha256').update(png).digest('hex'));
    }
    if(new Set(hashes).size!==3) throw new Error(`${part}: opposing instance poses did not change`);
    // Compare each GPU pixel of the pair with renders of the individual instances at the same poses.
    // Reusing one skinned vertex buffer for both draws fails this even if all three pose images differ.
    const captures=[];
    for(const mask of [0,1,2,3]) {
      const encoded=await page.evaluate(async({part,mask})=>{
        window.rigFixture.rig_fixture_instances(part,0,mask);
        const bytes=await window.rigFixture.fx_fixture_pixels();
        let s='';for(let i=0;i<bytes.length;i+=32768)s+=String.fromCharCode(...bytes.subarray(i,i+32768));
        return btoa(s);
      },{part,mask});
      captures.push(Buffer.from(encoded,'base64'));
    }
    let mismatch=0, visible=0;
    for(let i=0;i<captures[0].length;i+=8) {
      const [background,left,right,pair]=captures.map(b=>b.subarray(i,i+8));
      const hasLeft=!left.equals(background),hasRight=!right.equals(background);
      if(hasLeft||hasRight) visible++;
      const valid=hasLeft&&hasRight ? pair.equals(left)||pair.equals(right) : pair.equals(hasLeft?left:hasRight?right:background);
      if(!valid) mismatch++;
    }
    if(visible<100||mismatch>visible*0.001) throw new Error(`${part}: instance independence failed (${mismatch}/${visible} pixels)`);
    console.log(`${part}: three poses differ; two independent instances rendered`);
  }
  if(errors.length) throw new Error(errors.join('\n'));
} finally {await browser.close();}
