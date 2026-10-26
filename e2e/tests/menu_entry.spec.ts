import {expect,test} from '@playwright/test';

test.use({serviceWorkers:'block'});

test('static first paint needs no JavaScript or exported assets',async({browser,baseURL})=>{
  const context=await browser.newContext({javaScriptEnabled:false,viewport:{width:1280,height:720},serviceWorkers:'block'});
  try {
    const page=await context.newPage();await page.goto(baseURL!);
    await expect(page.locator('[data-ose="menu-poster"]')).toBeVisible();
    await expect(page.locator('[data-ose="menu-poster"] svg')).toBeVisible();
    await expect(page.locator('.logo-title')).toBeVisible();
    await expect(page.locator('[data-ose="play"]')).toBeVisible();
    await page.screenshot({path:test.info().outputPath('first-paint-desktop.png')});
    await page.setViewportSize({width:390,height:844});
    await expect(page.locator('[data-ose="play"]')).toBeVisible();
    await page.screenshot({path:test.info().outputPath('first-paint-mobile.png')});
  } finally {await context.close();}
});

test('a slow bootstrap cannot block the first-paint artwork',async({page,baseURL})=>{
  let release!:()=>void;const held=new Promise<void>(resolve=>{release=resolve;});
  await page.route('**/asset-packages.js',async route=>{await held;await route.continue();});
  try {
    await page.goto(baseURL!,{waitUntil:'commit'});
    await expect(page.locator('[data-ose="menu-poster"] svg')).toBeVisible();
    await expect(page.locator('[data-ose="play"]')).toBeVisible();
  } finally {release();}
});

test('Canvas fallback keeps the static menu and removes it when joining a campaign',async({page,baseURL})=>{
  await page.addInitScript(()=>Object.defineProperty(navigator,'gpu',{value:undefined,configurable:true}));
  await page.goto(baseURL!);
  await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-menu-scene','static');
  await expect(page.locator('[data-ose="menu-poster"]')).toBeVisible();
  await expect(page.locator('.poster-status')).toBeHidden();
  await page.locator('[data-ose="player-name"]').fill('static menu pilot');
  await page.locator('[data-ose="play"]').click();
  await expect(page.locator('[data-ose="menu-poster"]')).toBeVisible();
  await page.locator('[data-ose="session-join"]').first().click();
  await expect(page.locator('[data-ose="ksc"]')).toBeVisible();
  await expect(page.locator('[data-ose="menu-poster"]')).toBeHidden();
});

function glb(textured:boolean) {
  const bin=Buffer.alloc(48);
  [-1,0,0,1,0,0,0,1,0].forEach((v,i)=>bin.writeFloatLE(v,i*4));
  [0,1,2].forEach((v,i)=>bin.writeUInt32LE(v,36+i*4));
  const source=JSON.stringify({asset:{version:'2.0'},buffers:[{byteLength:48}],
    bufferViews:[{buffer:0,byteOffset:0,byteLength:36},{buffer:0,byteOffset:36,byteLength:12}],
    accessors:[{bufferView:0,componentType:5126,count:3,type:'VEC3'},{bufferView:1,componentType:5125,count:3,type:'SCALAR'}],
    meshes:[{primitives:[{attributes:{POSITION:0},indices:1,...(textured?{material:0}:{})}]}],
    ...(textured?{images:[{uri:'../textures/menu-test.png'}],textures:[{source:0}],materials:[{pbrMetallicRoughness:{baseColorTexture:{index:0}}}]}:{}),
    nodes:[{mesh:0}],scenes:[{nodes:[0]}],scene:0});
  const json=Buffer.from(source.padEnd(Math.ceil(source.length/4)*4,' '));
  const h=Buffer.alloc(20);h.write('glTF');h.writeUInt32LE(2,4);h.writeUInt32LE(28+json.length+48,8);h.writeUInt32LE(json.length,12);h.writeUInt32LE(0x4e4f534a,16);
  const b=Buffer.alloc(8);b.writeUInt32LE(48);b.writeUInt32LE(0x004e4942,4);
  return Buffer.concat([h,json,b,bin]);
}

test('poster covers partial geometry until all visible menu models have previews, with setup still usable',async({page,baseURL})=>{
  await page.goto(`${baseURL}/health`);
  const gpu=await page.evaluate(async()=>{try{return !!await(navigator as any).gpu?.requestAdapter();}catch{return false;}});
  test.skip(!gpu,'The textured handoff requires WebGPU.');
  await page.emulateMedia({reducedMotion:'reduce'});
  const errors:string[]=[];page.on('pageerror',e=>errors.push(e.message));
  await page.route('**/assets/ksc/scene.json',r=>r.fulfill({status:404}));
  await page.route('**/assets/vab/scene.json',r=>r.fulfill({status:404}));
  await page.route('**/assets/menu/scene.json',r=>r.fulfill({json:{camera:{pos:[0,0,10],forward:[0,0,-1],up:[0,1,0],fov_deg:60},
    kerbin:{file:'menu/planet.glb',pos:[0,0,0],rot:[0,0,0,1],scale:[1,1,1],mesh_radius:1,spin_deg_s:0},
    vessels:[{name:'Fixture ship',pos:[0,0,0],rot:[0,0,0,1],scale:[1,1,1],spin_deg_s:0,parts:[
      {id:'menu-ship',pos:[0,0,0],rot:[0,0,0,1],scale:[1,1,1]},
      {id:'inactive',pos:[0,0,0],rot:[0,0,0,1],scale:[1,1,1],active:false}]}]}}));
  await page.route('**/assets/menu/planet.glb',r=>r.fulfill({body:glb(false),contentType:'model/gltf-binary'}));
  await page.route('**/assets/parts/menu-ship.glb',r=>r.fulfill({body:glb(true),contentType:'model/gltf-binary'}));
  let inactive=0;await page.route('**/assets/parts/inactive.glb',r=>{inactive++;return r.fulfill({status:404});});
  let release!:()=>void,fullRelease!:()=>void;
  const held=new Promise<void>(resolve=>{release=resolve;}),full=new Promise<void>(resolve=>{fullRelease=resolve;});
  const png=Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAFUlEQVR4nGMMqDjxn4GBgYEJRIAwACawApOpXK/OAAAAAElFTkSuQmCC','base64');
  let previewStarted=false;
  await page.route('**/assets/textures/menu-test.png*',async r=>{
    const preview=r.request().url().endsWith('?preview=1');if(preview){previewStarted=true;await held;}else await full;
    await r.fulfill({body:png,contentType:'image/png',headers:preview?{'x-ose-preview':'1'}:{}});
  });
  try {
    await page.goto(baseURL!);
    await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-menu-vessel','Fixture ship');
    await expect.poll(()=>previewStarted).toBe(true);
    await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-menu-scene','loading');
    await expect(page.locator('[data-ose="menu-poster"]')).toHaveCSS('opacity','1');
    await page.locator('[data-ose="player-name"]').fill('typing while scenery loads');
    await page.locator('[data-ose="play"]').click();
    await expect(page.locator('[data-ose="session-join"]').first()).toBeVisible();
    release();
    await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-menu-scene','ready');
    await expect(page.locator('[data-ose="menu-poster"]')).toHaveCSS('opacity','0');
    await expect(page.locator('[data-ose="menu-poster"]')).toHaveCSS('transition-duration','0s');
    expect(inactive).toBe(0);expect(errors).toEqual([]);
  } finally {release();fullRelease();}
});
