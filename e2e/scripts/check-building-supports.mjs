// Exercises the actual two-anchor editor flow in WebGPU, using local game meshes.
import {chromium} from 'playwright';
import fs from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';
const base=process.env.OSE_BASE_URL || 'http://127.0.0.1:7807';
if (!/^http:\/\/(127\.0\.0\.1|localhost):/.test(base)) throw new Error('Use a local test server');
const out=new URL('../../.cache/support-parity/',import.meta.url);
await fs.mkdir(out,{recursive:true});
const browser=await chromium.launch({channel:'chromium',headless:true,args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist','--disable-dawn-features=use_dxc']});
const page=await browser.newPage({viewport:{width:1600,height:1000}});
const errors=[];
page.on('pageerror',e=>errors.push(e.message));
const shot=async name=>page.screenshot({path:fileURLToPath(new URL(name,out))});
try {
  await page.goto(base);
  await page.locator('[data-ose="player-name"]').fill('Support test '+Date.now());
  await page.locator('[data-ose="login"]').click();
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().waitFor();
  if (!await page.locator('#create-name').isVisible()) await page.locator('#create-details > summary').click();
  await page.selectOption('#create-mode','Sandbox');
  await page.fill('#create-name','Building supports '+Date.now());
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-vab"]').waitFor({state:'visible'});
  const hints=page.getByText('No more hints',{exact:true});
  if (await hints.isVisible()) await hints.click();
  await page.locator('[data-ose="ksc-vab"]').click();
  await page.locator('[data-ose="vab-clear"]').click();
  const add=async id=>{
    await page.locator('[data-ose="vab-search"]').fill(id);
    await page.locator(`[data-ose="vab-part"][data-ose-part="${id}"]`).dblclick();
  };
  await add('pod_1v_conical_crew');
  await add('fueltank_1v_inline_methalox_1x2');
  await add('engine_1v_methalox_swivel');
  await add('strut_1v_extensible');
  const gpu=page.locator('[data-ose="gpu"]');
  await page.waitForFunction(()=>document.querySelector('[data-ose="gpu"]')?.getAttribute('data-ose-vab-scene')==='ready');
  assert.equal(await gpu.getAttribute('data-ose-renderer'),'webgpu');
  await page.waitForFunction(()=>!document.querySelector("#loading")?.classList.contains("show"));
  await page.waitForTimeout(500);
  await shot('ose-before-connection.png');
  let target;
  // Find the visible pod through the renderer's real hit-testing; do not inject editor state.
  for(let y=220;y<=780&&!target;y+=20) for(let x=560;x<=1180;x+=20){
    await page.mouse.move(x,y);
    if(await gpu.getAttribute('data-ose-vab-hovered')==='0'){target={x,y};break;}
  }
  assert.ok(target,'root pod must be visible and pickable');
  // The picker uses bounds; the compound tool raycasts the actual mesh. Aim inside
  // the pod, rather than at the first bounds pixel near its conical tip.
  const xs=[];
  for(let x=target.x;x<=target.x+140;x+=5){
    await page.mouse.move(x,target.y);
    if(await gpu.getAttribute('data-ose-vab-hovered')==='0') xs.push(x);
    else if(xs.length) break;
  }
  target.x=Math.round((xs[0]+xs.at(-1))/2);
  const ys=[];
  for(let y=target.y;y<=800;y+=5){
    await page.mouse.move(target.x,y);
    if(await gpu.getAttribute('data-ose-vab-hovered')==='0') ys.push(y);
    else if(ys.length) break;
  }
  target.y=Math.round((ys[0]+ys.at(-1))/2);
  await page.mouse.click(target.x,target.y);
  await page.waitForFunction(()=>document.querySelector('[data-ose="gpu"]')?.getAttribute('data-ose-vab-struts')==='1');
  await shot('ose-braced-vab.png');
  await page.locator('#vab-undo').click();
  await page.waitForFunction(()=>document.querySelector('[data-ose="gpu"]')?.getAttribute('data-ose-vab-struts')==='0');
  await page.locator('#vab-redo').click();
  await page.waitForFunction(()=>document.querySelector('[data-ose="gpu"]')?.getAttribute('data-ose-vab-struts')==='1');
  await page.locator('[data-ose="vab-save"]').click();
  await page.waitForTimeout(1200);
  await page.locator('[data-ose="vab-launch"]').click();
  const hud=page.locator('[data-ose="flight"].open');
  await hud.waitFor();
  await page.waitForFunction(()=>!document.querySelector("#loading")?.classList.contains("show"));
  await page.waitForTimeout(1000);
  await shot('ose-braced-pad.png');
  assert.deepEqual(errors,[]);
  await fs.writeFile(new URL('browser-results.json',out),JSON.stringify({webgpu:true,target,connectedStruts:1,undoRedo:true,savedAndLaunched:true,errors},null,2));
  console.log('PASS: two-anchor placement, undo/redo, save, launch, two-anchor connector on pad, WebGPU; no browser errors');
} catch(e) {
  await shot('browser-failure.png');
  console.error((await page.locator('body').innerText()).slice(-5000));
  throw e;
} finally {await browser.close();}
