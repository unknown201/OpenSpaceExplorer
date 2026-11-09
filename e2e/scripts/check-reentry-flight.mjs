// Real server snapshots -> flight renderer, using an isolated upper-atmosphere save.
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {createServer} from 'node:net';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';
const root=fileURLToPath(new URL('../..',import.meta.url));
const out=path.join(root,'.cache/reentry-flight');
await fs.mkdir(out,{recursive:true});
const altitude=Number(process.env.OSE_ALTITUDE??52000),speed=Number(process.env.OSE_SPEED??3000);
const vertical=Number(process.env.OSE_VERTICAL??-300);
const tag=process.env.OSE_CASE??(process.env.OSE_BEFORE?'before':'after');
const expectPlasma=altitude<70000&&speed>1000;
const crafts=JSON.parse(await fs.readFile(path.join(root,'assets/own/stock-craft.json'),'utf8'));
const pod=crafts.find(c=>c.parts[0].part_id==='pod_1v_conical_crew').parts[0];
const craft={name:'Atmospheric test pod',description:'Unpowered atmospheric pass',parts:[pod],stages:[]};
const save={version:1,tick:0,warp:1,crafts:[{id:1000,owner:1,craft}],vessels:[{
 id:1,name:craft.name,owner:1,body:'Kerbin',burn:null,craft,crew:[[0,'Jebediah Kerman']],
 elements:{semi_major_axis:600000+altitude,eccentricity:0,inclination_deg:0,longitude_of_ascending_node_deg:0,argument_of_periapsis_deg:0,mean_anomaly_at_epoch:0,epoch:0},
 flight:{spec:{stages:[],drag_area_m2:1,drag_cd:0.2},stage:0,propellant_t:0,mass_t:1,
 r:{x:600000+altitude,y:0,z:0},v:{x:vertical,y:speed+(600000+altitude)*2*Math.PI/21549.425,z:0},throttle:0,attitude:'SurfaceRetrograde',landed:false,crashed:false,coasting_since:null}
}],campaign:{id:'shared',name:'Atmospheric comparison',started_tick:0,members:[],mode:'Sandbox',
 difficulty:process.env.OSE_HEAT_TEST?[{id:'HeatPartDestruction',value:{Bool:false}}]:[]}};
const savePath=path.join(out,'save.json');
await fs.writeFile(savePath,JSON.stringify(save));
const port=process.env.OSE_REENTRY_PORT?Number(process.env.OSE_REENTRY_PORT):await new Promise((resolve,reject)=>{
 const listener=createServer();listener.on('error',reject);
 listener.listen(0,'127.0.0.1',()=>{const port=listener.address().port;listener.close(()=>resolve(port));});
}),base=`http://127.0.0.1:${port}`;
const server=spawn(process.env.OSE_SERVER??path.join(root,'target/debug/ose-server.exe'),[
 '--listen',`127.0.0.1:${port}`,'--static-dir',process.env.OSE_STATIC_DIR??path.join(root,'target/reentry-web'),
 '--assets-dir',path.join(root,'assets/web'),'--save',savePath,'--db',':memory:','--save-interval-s','3600'
],{cwd:root,windowsHide:true,stdio:['ignore','pipe','pipe']});
let log='';server.stdout.on('data',b=>log+=b);server.stderr.on('data',b=>log+=b);
let browser;
try {
 for(let i=0;;i++) {
  if(server.exitCode!==null)throw new Error(log);
  if(await fetch(`${base}/health`).then(r=>r.ok).catch(()=>false))break;
  if(i>100)throw new Error(`Server did not start: ${log}`);
  await new Promise(r=>setTimeout(r,100));
 }
 browser=await chromium.launch({channel:'chromium',args:['--enable-unsafe-webgpu','--ignore-gpu-blocklist','--disable-dawn-features=use_dxc']});
 const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];
 await page.addInitScript(()=>{
  localStorage.setItem('ose.ftue','off');
  localStorage.setItem('ose.flight.shared.1','1000');
 });
 page.on('pageerror',e=>errors.push(e.message));
 await page.goto(base);
 await page.locator('[data-ose="player-name"]').fill('Reentry pilot');
 await page.locator('[data-ose="play"]').click();
 await page.locator('[data-ose="session-join"]').first().click();
 await page.locator('[data-ose="ksc-tracking"]').click();
 await page.locator('[data-ose="ts-vessel"][data-ose-id="1"]').click();
 await page.locator('[data-ose="ts-fly"]').click();
 const hud=page.locator('[data-ose="flight"].open');await hud.waitFor();
 await page.mouse.move(720,450);await page.mouse.wheel(0,-1600);
 await page.waitForTimeout(6000);
 if(process.env.OSE_HEAT_TEST)await page.waitForFunction(()=>document.querySelector('#fh-thermal')?.dataset.level==='critical',null,{timeout:60000});
 assert.equal(await page.locator('[data-ose="gpu"]').getAttribute('data-ose-renderer'),'webgpu');
 const state=await hud.evaluate(e=>({altitude:e.getAttribute('data-ose-altitude'),layers:Number(e.getAttribute('data-ose-engine-fx-layers')),thermal:document.querySelector('#fh-thermal')?.dataset.level}));
 await page.screenshot({path:path.join(out,`${tag}.png`)});
 console.log(state);
 if(!process.env.OSE_BEFORE){
  assert.equal(state.layers>0,expectPlasma,'Plasma must follow atmospheric speed/density');
  if(expectPlasma)assert.notEqual(state.thermal,'off','Authoritative heat flux must show atmospheric heating');
  else assert.equal(state.thermal,'off','Cold slow flight or vacuum must not show a heat warning');
  assert.ok(state.thermal,'Thermal feedback must exist');
  await page.keyboard.press('m');
  assert.equal(await page.locator('[data-ose="flight-thermal"]').isVisible(),expectPlasma,'Map heat feedback follows flight state');
 }
 assert.deepEqual(errors,[]);
 await fs.writeFile(path.join(out,`${tag}.json`),JSON.stringify({altitude,speed,vertical,state,errors},null,2));
} finally {
 await browser?.close();server.kill();
 await fs.writeFile(path.join(out,'server.log'),log);
}
