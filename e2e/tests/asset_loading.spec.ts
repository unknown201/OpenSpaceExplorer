import { expect, test, type Page } from '@playwright/test';

// These individual-loader regressions deliberately exercise the fallback without package interception.
test.use({serviceWorkers:'block'});

test.beforeEach(async ({page,baseURL}) => {
  await page.goto(`${baseURL}/health`);
  const gpu = await page.evaluate(async () => {
    try { return !!(await (navigator as any).gpu?.requestAdapter()); } catch { return false; }
  });
  test.skip(!gpu, 'Scene-loading tests require WebGPU; this browser uses Canvas fallback.');
  await page.route('**/assets/menu/**', route => route.fulfill({status:404}));
  await page.route('**/assets/vab/scene.json', route => route.fulfill({status:404}));
  // CI has no exported game install. Keep the loader tests independent of those local assets.
  await page.route('**/assets/ksc/scene.json', route => route.fulfill({json:{file:'ksc/ksc.glb',
    camera:{origin:[0,2,10],look_at:[0,0,0],fov_deg:60},buildings:[]}}));
  await page.route('**/assets/terrain/Kerbin.json', route => route.fulfill({json:{version:1,id:'Kerbin',
    radius_m:600000,has_ocean:true,sites:[],global:{offset:0,width:1,height:1,scale_m:0,offset_m:0},
    mask:null,contrast:1,large:[],medium:[],decal_heights:null,decal_alpha:null,decals:[]}}));
});

function model(revision: number, textured = false, extraImages: string[] = []): Buffer {
  const bin = Buffer.alloc(4 * 1024 * 1024);
  [-1,0,0, 1,0,0, 0,1,0].forEach((v,i) => bin.writeFloatLE(v,i*4));
  [0,1,2].forEach((v,i) => bin.writeUInt32LE(v,36+i*4));
  bin[bin.length-1] = revision;
  const source = JSON.stringify({asset:{version:'2.0'},buffers:[{byteLength:bin.length}],
    bufferViews:[{buffer:0,byteOffset:0,byteLength:36},{buffer:0,byteOffset:36,byteLength:12}],
    accessors:[{bufferView:0,componentType:5126,count:3,type:'VEC3'},
      {bufferView:1,componentType:5125,count:3,type:'SCALAR'}],
    meshes:[{primitives:[{attributes:{POSITION:0},indices:1,...(textured ? {material:0} : {})}]}],
    ...(textured ? {images:[...extraImages.map(uri => ({uri})),{uri:'../textures/loading-test.png'}],textures:[{source:extraImages.length}],
      materials:[{pbrMetallicRoughness:{baseColorTexture:{index:0}}}]} : {}),
    nodes:[{mesh:0}],scenes:[{nodes:[0]}],scene:0});
  const json = Buffer.from(source.padEnd(Math.ceil(source.length/4)*4,' '));
  const header = Buffer.alloc(20); header.write('glTF'); header.writeUInt32LE(2,4);
  header.writeUInt32LE(28+json.length+bin.length,8); header.writeUInt32LE(json.length,12); header.writeUInt32LE(0x4e4f534a,16);
  const binHeader = Buffer.alloc(8); binHeader.writeUInt32LE(bin.length); binHeader.writeUInt32LE(0x004e4942,4);
  return Buffer.concat([header,json,binHeader,bin]);
}

async function join(page: Page, base: string) {
  await page.goto(base);
  await enterCampaign(page);
}

async function enterCampaign(page: Page) {
  await expect(page.locator('#status')).toContainText('client loaded');
  await page.locator('[data-ose="player-name"]').fill('loading test');
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().click();
  await expect(page.locator('[data-ose="ksc"]')).toBeVisible();
}

test('main menu warms both models and previews without blocking setup, even when storage is blocked', async ({page,baseURL}) => {
  await page.addInitScript(() => Object.defineProperty(window,'caches',{get(){throw new Error('storage blocked');}}));
  await page.addInitScript(() => {
    const prototype = (window as any).GPUAdapterInfo?.prototype;
    if (prototype) Object.defineProperty(prototype,'isFallbackAdapter',{get:() => {
      (window as any).__softwareInfoReads = ((window as any).__softwareInfoReads || 0) + 1;
      return true;
    }});
  });
  await page.route('**/assets/vab/scene.json', route => route.fulfill({json:{file:'vab/vab.glb',
    camera:{origin:[0,2,10],look_at:[0,0,0],fov_deg:60}}}));
  const models: string[] = [], images: boolean[] = [], errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.route('**/assets/{ksc,vab}/*.glb', route => {
    models.push(route.request().url());
    return route.fulfill({body:model(1,true,['../textures/loading-small.png']),contentType:'model/gltf-binary'});
  });
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAFUlEQVR4nGMMqDjxn4GBgYEJRIAwACawApOpXK/OAAAAAElFTkSuQmCC','base64');
  // Already-small source images have no preview marker and must not stop the rest of preloading.
  let small = 0;
  await page.route('**/assets/textures/loading-small.png*', route => {
    small++;
    return route.fulfill({body:png,contentType:'image/png'});
  });
  await page.route('**/assets/textures/loading-test.png*', route => {
    const preview = route.request().url().endsWith('?preview=1');
    images.push(preview);
    return route.fulfill({body:png,contentType:'image/png',headers:preview ? {'x-ose-preview':'1'} : {}});
  });
  await page.goto(baseURL!);
  await page.locator('[data-ose="player-name"]').fill('ready while typing');
  await expect(page.locator('[data-ose="play"]')).toBeEnabled();
  await page.locator('[data-ose="play"]').click();
  await expect(page.locator('[data-ose="session-join"]').first()).toBeVisible();
  await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-preload','ready');
  expect(models.length).toBe(2);
  expect(images).toEqual([true]);
  expect(small).toBe(1);
  await page.locator('[data-ose="session-join"]').first().click();
  await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-textured-scene','ksc');
  expect(models.length).toBe(2);
  // Foreground upgrades may now begin; the prefetched preview is reused without a second request.
  expect(images.filter(Boolean)).toHaveLength(1);
  await page.locator('[data-ose="ksc-vab"]').first().click();
  await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-vab-scene','ready');
  expect(models.length).toBe(2);
  expect(errors).toEqual([]);
  expect(await page.evaluate(() => (window as any).__softwareInfoReads || 0)).toBeGreaterThan(0);
});

test('joining during a menu transfer reuses it and pauses remaining background scenes', async ({page,baseURL}) => {
  let release!: () => void;
  const held = new Promise<void>(resolve => {release=resolve;});
  let downloads=0, vab=0;
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.route('**/assets/ksc/ksc.glb', async route => {
    downloads++;
    await held;
    await route.fulfill({body:model(1),contentType:'model/gltf-binary'});
  });
  await page.route('**/assets/vab/scene.json', route => {vab++; return route.fulfill({status:404});});
  try {
    await page.goto(baseURL!);
    await expect.poll(() => downloads).toBe(1);
    await enterCampaign(page);
    await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-preload','paused');
    release();
    await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-ksc-scene','ready');
    expect(downloads).toBe(1);
    expect(vab).toBe(0);
    expect(errors).toEqual([]);
  } finally {release();}
});

test('large assets persist, revalidate and update; cache admission evicts old data', async ({ page, baseURL }) => {
  let revision = 1;
  const sent: {status:number; conditional:string|undefined}[] = [];
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.route('**/assets/menu/**', route => route.fulfill({status:404}));
  await page.route('**/assets/ksc/ksc.glb', async route => {
    const date = new Date(Date.UTC(2026,5,revision)).toUTCString();
    const conditional = route.request().headers()['if-modified-since'];
    const status = conditional === date ? 304 : 200;
    sent.push({status,conditional});
    await route.fulfill({status, headers:{'last-modified':date,'cache-control':'no-cache','content-type':'model/gltf-binary'},
      ...(status === 200 ? {body:model(revision)} : {})});
  });
  // Seed a full cache so the first admission also exercises eviction without allocating 256 MiB.
  await page.goto(`${baseURL}/health`);
  await page.evaluate(async () => {
    const cache = await caches.open('ose-large-assets-v1');
    for (let i=0;i<4;i++) await cache.put(`/old-${i}.glb`, new Response('old', {headers:{'x-ose-decoded-bytes':String(64*1024*1024)}}));
  });
  const cachedRevision = async () => page.evaluate(async () => {
    const cache = await caches.open('ose-large-assets-v1');
    const saved = await cache.match('assets/ksc/ksc.glb');
    if (!saved) return 0;
    const bytes = new Uint8Array(await saved.arrayBuffer());
    return bytes[bytes.length-1];
  });
  await join(page, baseURL!);
  await expect.poll(cachedRevision).toBe(1);
  expect(sent.map(r=>r.status)).toEqual([200]);
  expect(await page.evaluate(async () => (await (await caches.open('ose-large-assets-v1')).keys()).length)).toBe(4);

  await join(page, baseURL!);
  await expect.poll(() => sent.length).toBe(2);
  expect(sent[1].status).toBe(304);
  expect(sent[1].conditional).toBeTruthy();
  await expect.poll(cachedRevision).toBe(1);

  revision = 2;
  await join(page, baseURL!);
  await expect.poll(cachedRevision).toBe(2);
  expect(sent[2].status).toBe(200);
  expect(errors).toEqual([]);
});

test('blocked browser storage falls back to a normal asset download', async ({ page, baseURL }) => {
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.addInitScript(() => Object.defineProperty(window,'caches',{get(){throw new Error('storage blocked');}}));
  await page.route('**/assets/menu/**', route => route.fulfill({status:404}));
  let downloads = 0;
  await page.route('**/assets/ksc/ksc.glb', route => {
    downloads++;
    return route.fulfill({body:model(1),contentType:'model/gltf-binary'});
  });
  await join(page, baseURL!);
  await expect.poll(() => downloads).toBe(1);
  await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-ksc-scene','ready');
  expect(errors).toEqual([]);
});

test('planet metadata starts while its terrain pack is still downloading', async ({ page, baseURL }) => {
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  let terrainStarted = false, metadataStarted = false;
  await page.route('**/assets/terrain/Kerbin.bin', async route => {
    terrainStarted = true;
    await held;
    await route.fulfill({status:404});
  });
  await page.route('**/assets/planets/Kerbin/planet.json', async route => {
    metadataStarted = true;
    await route.continue();
  });
  try {
    await page.goto(`${baseURL}/?planet=Kerbin&alt=10000`);
    await expect.poll(() => terrainStarted).toBe(true);
    await expect.poll(() => metadataStarted).toBe(true);
  } finally { release(); }
});

test('a scene draws its preview and then fetches the full original texture', async ({ page, baseURL }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/assets/menu/**', route => route.fulfill({status:404}));
  await page.route('**/assets/ksc/ksc.glb', route => route.fulfill({body:model(1,true),contentType:'model/gltf-binary'}));
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAFUlEQVR4nGMMqDjxn4GBgYEJRIAwACawApOpXK/OAAAAAElFTkSuQmCC','base64');
  const requests: boolean[] = [];
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/assets/textures/loading-test.png*', async route => {
    const preview = route.request().url().endsWith('?preview=1');
    requests.push(preview);
    if (!preview) await held;
    await route.fulfill({body:png,contentType:'image/png',headers:preview ? {'x-ose-preview':'1'} : {}});
  });
  try {
    await join(page,baseURL!);
    await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-textured-scene','ksc');
    await expect.poll(() => requests.length).toBe(2);
    expect(requests).toEqual([true,false]);
  } finally { release(); }
  expect(errors).toEqual([]);
});
