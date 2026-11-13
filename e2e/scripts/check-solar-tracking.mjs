// Requires the current WASM build and exported game assets. Exercises the production per-draw pose.
import {chromium} from 'playwright';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const out=process.env.OSE_AUDIT_OUT??path.join(root,'.cache/services-parity/solar-browser');
fs.mkdirSync(out,{recursive:true});
const catalog=JSON.parse(fs.readFileSync(path.join(root,'assets/own/parts-catalog.json'),'utf8'));
const browser=await chromium.launch({channel:'chromium',executablePath:process.env.OSE_CHROMIUM_EXECUTABLE,
  args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist',...(process.platform==='win32'?['--disable-dawn-features=use_dxc']:[])]});
try {
  const page=await browser.newPage({viewport:{width:650,height:650}});
  const errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  page.on('console',m=>{if(m.type()==='error') errors.push(m.text());});
  await page.goto(process.env.OSE_BASE_URL??'http://127.0.0.1:17865');
  await page.waitForFunction(()=>window.wasmBindings?.rig_fixture_solar);
  await page.evaluate(async()=>{await window.wasmBindings.fx_fixture_setup();});
  const results=[];
  for(const part of ['solararray_1v_shielded_1x6','solararray_2v_accordion']) {
    const pivot=catalog.find(p=>p.id===part).flight_geometry.solar.pivot_node;
    await page.waitForFunction(({part,pivot})=>window.wasmBindings.rig_fixture_solar(part,pivot,30,-60,3),{part,pivot});
    await page.waitForTimeout(1500); // asynchronously loaded materials must settle before pixel comparisons
    const capture=async(left,right,mask)=>Buffer.from(await page.evaluate(async args=>{
      window.wasmBindings.rig_fixture_solar(...args);
      const bytes=await window.wasmBindings.fx_fixture_pixels();
      let s='';for(let i=0;i<bytes.length;i+=32768)s+=String.fromCharCode(...bytes.subarray(i,i+32768));
      return btoa(s);
    },[part,pivot,left,right,mask]),'base64');
    const captures=[];
    for(const mask of [0,1,2,3]) captures.push(await capture(30,-60,mask));
    let mismatch=0,visible=0;
    for(let i=0;i<captures[0].length;i+=8) {
      const [background,left,right,pair]=captures.map(b=>b.subarray(i,i+8));
      const a=!left.equals(background),b=!right.equals(background);
      if(a||b) visible++;
      const valid=a&&b ? pair.equals(left)||pair.equals(right) : pair.equals(a?left:b?right:background);
      if(!valid) mismatch++;
    }
    if(visible<100||mismatch>visible*0.001) throw new Error(`${part}: shared pose contamination ${mismatch}/${visible}`);
    if(captures[1].equals(await capture(-60,-60,1))) throw new Error(`${part}: tracking did not change rendered pose`);
    await capture(30,-60,3);
    await page.locator('#fx-fixture').screenshot({path:path.join(out,`${part}.png`)});
    results.push({part,pivot,angles_deg:[30,-60],visible_pixels:visible,mismatched_pixels:mismatch});
  }
  if(errors.length) throw new Error(errors.join('\n'));
  fs.writeFileSync(path.join(out,'results.json'),JSON.stringify(results,null,2)+'\n');
  console.log(JSON.stringify(results,null,2));
} finally {await browser.close();}
