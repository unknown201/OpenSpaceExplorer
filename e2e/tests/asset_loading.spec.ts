import { expect, test, type Page } from '@playwright/test';

test.beforeEach(async ({page,baseURL}) => {
  await page.goto(`${baseURL}/health`);
  const gpu = await page.evaluate(async () => !!(await (navigator as any).gpu?.requestAdapter()));
  test.skip(!gpu, 'Scene-loading tests require WebGPU; this browser uses Canvas fallback.');
});

function model(revision: number, textured = false): Buffer {
  const bin = Buffer.alloc(4 * 1024 * 1024);
  [-1,0,0, 1,0,0, 0,1,0].forEach((v,i) => bin.writeFloatLE(v,i*4));
  [0,1,2].forEach((v,i) => bin.writeUInt32LE(v,36+i*4));
  bin[bin.length-1] = revision;
  const source = JSON.stringify({asset:{version:'2.0'},buffers:[{byteLength:bin.length}],
    bufferViews:[{buffer:0,byteOffset:0,byteLength:36},{buffer:0,byteOffset:36,byteLength:12}],
    accessors:[{bufferView:0,componentType:5126,count:3,type:'VEC3'},
      {bufferView:1,componentType:5125,count:3,type:'SCALAR'}],
    meshes:[{primitives:[{attributes:{POSITION:0},indices:1,...(textured ? {material:0} : {})}]}],
    ...(textured ? {images:[{uri:'../textures/loading-test.png'}],textures:[{source:0}],
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
  await expect(page.locator('#status')).toContainText('client loaded');
  await page.locator('[data-ose="player-name"]').fill('loading test');
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().click();
  await expect(page.locator('[data-ose="ksc"]')).toBeVisible();
}

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
