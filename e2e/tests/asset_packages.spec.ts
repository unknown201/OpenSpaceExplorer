import {expect, test, type Page} from '@playwright/test';
import {createServer, type Server} from 'node:http';
import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {createGzip, constants} from 'node:zlib';

// Serve real worker/bootstrap code with small, deterministic usage packages. No exported game install,
// GPU, request interception, or production campaign is required to exercise the package transport.
test.use({serviceWorkers:'allow'});
type Entry = {url:string; body:Buffer; type?:string};
const model = Buffer.alloc(4*1024*1024,7);
let server: Server, base: string, entries: Record<string,Entry[]>, requested: string[], individual: string[];
let broken: boolean, blocked: boolean, holdVab: boolean, release: () => void, held: Promise<void>;
const date = 'Mon, 26 Oct 2026 10:00:00 GMT';
const entry = (url:string, body:string|Buffer):Entry => ({url:`assets/${url}`,body:typeof body === 'string' ? Buffer.from(body) : body});
const revision = (rows:Entry[]) => createHash('sha256').update(Buffer.concat(rows.map(e=>e.body))).digest('hex').slice(0,16);
function catalog() {
  const chunks = Object.fromEntries(Object.entries(entries).map(([id, rows]) => {
    const rev = revision(rows);
    return [id,{revision:rev,url:`assets/packages/${id}/${rev}.bin`,assets:rows.map(e=>e.url)}];
  }));
  const group = (id:string, optional:string[] = []) => ({depends:id === 'core' ? [] : ['core'],
    chunks:[`${id}-0`,...optional],prefetch:[`${id}-0`]});
  return {version:1,chunks,groups:{core:group('core'),'mission-control':group('mission-control'),
    menu:group('menu'),ksc:group('ksc'),vab:group('vab'),'planet-Mun':group('planet-Mun',['planet-Mun-1'])}};
}
test.beforeEach(async () => {
  entries = {
    'core-0':[entry('ui/core.png','core')],
    'menu-0':[entry('menu/scene.json','menu backdrop')],
    'mission-control-0':[entry('ui/mission.json','mission control')],
    'ksc-0':[entry('ksc/scene.json','KSC metadata'),entry('ksc/ksc.glb','KSC geometry')],
    'vab-0':[entry('vab/scene.json','VAB metadata'),entry('vab/vab.glb',model)],
    'planet-Mun-0':[entry('planets/Mun/planet.json','Mun metadata'),entry('planets/Mun/albedo.jpg','Mun colour')],
    'planet-Mun-1':[entry('planets/Mun/detail_a0.png','Mun detail')],
  };
  requested=[]; individual=[]; broken=false; blocked=false; holdVab=false;
  held=new Promise<void>(r=>{release=r;});
  const worker = await readFile(new URL('../../crates/ose-client/asset-worker.js',import.meta.url),'utf8');
  const bootstrap = await readFile(new URL('../../crates/ose-client/asset-packages.js',import.meta.url),'utf8');
  server = createServer(async (req,res) => {
    const path = new URL(req.url!,base).pathname;
    res.setHeader('cache-control','no-cache');
    if(path === '/favicon.ico') {res.statusCode=204;res.end();return;}
    if(path === '/health') {res.end('health'); return;}
    if(path === '/') {
      res.setHeader('content-type','text/html');
      res.end('<!doctype html><title>Package test</title><canvas data-ose="gpu"></canvas><input data-ose="player-name"><script src="/asset-packages.js"></script>'); return;
    }
    if(path === '/asset-worker.js' || path === '/asset-packages.js') {
      res.setHeader('content-type','application/javascript');
      const prelude = blocked ? "Object.defineProperty(self,'caches',{get(){throw new Error('storage blocked');}});\n" : '';
      res.end(path === '/asset-worker.js' ? prelude+worker : bootstrap); return;
    }
    if(path === '/assets/packages/index.json') {
      res.setHeader('content-type','application/json'); res.end(JSON.stringify(catalog())); return;
    }
    const match = path.match(/^\/assets\/packages\/([^/]+)\/([a-f0-9]+)\.bin$/);
    if(match) {
      const [id,rev] = match.slice(1); requested.push(id);
      const rows=entries[id];
      if(!rows || revision(rows)!==rev) {res.statusCode=410;res.end();return;}
      res.setHeader('content-type','application/octet-stream'); res.setHeader('content-encoding','gzip');
      const gzip = createGzip(); gzip.pipe(res); gzip.write('OSEPACK1');
      for(let i=0;i<rows.length;i++) {
        if(id==='vab-0' && i===1 && holdVab) await held;
        if(res.destroyed) {gzip.destroy();return;}
        const e=rows[i], meta=Buffer.from(JSON.stringify({url:e.url,size:broken && id==='vab-0' && i===1 ? 1024**3 : e.body.length,
          type:e.type || 'application/octet-stream',preview:false,modified:date}));
        const n=Buffer.alloc(4);n.writeUInt32LE(meta.length);gzip.write(n);gzip.write(meta);gzip.write(e.body);
        await new Promise<void>(resolve=>gzip.flush(constants.Z_SYNC_FLUSH,resolve));
      }
      gzip.end(Buffer.alloc(4)); return;
    }
    const relative=path.slice(1);
    const found=Object.values(entries).flat().find(e=>e.url===relative);
    individual.push(relative);
    if(found) {
      res.setHeader('content-type',found.type || 'application/octet-stream');
      if(req.headers.range==='bytes=0-3') {res.statusCode=206;res.setHeader('content-range',`bytes 0-3/${found.body.length}`);res.end(found.body.subarray(0,4));}
      else res.end(found.body);
    } else {res.statusCode=404;res.end('missing');}
  });
  await new Promise<void>(resolve=>server.listen(0,'127.0.0.1',resolve));
  base=`http://127.0.0.1:${(server.address() as any).port}`;
});
test.afterEach(async () => {release();server.closeAllConnections();await new Promise<void>(resolve=>server.close(()=>resolve()));});
async function open(page:Page) {
  await page.goto(base);
  await ready(page);
}
async function ready(page:Page) {
  await expect.poll(()=>page.evaluate(()=>(window as any).oseAssetPackages.status)).toBe('ready');
  expect(await page.evaluate(()=>(window as any).oseAssetPackages.ready)).toBe(true);
}
async function warm(page:Page,usage:string) {
  await page.evaluate(usage=>(window as any).oseAssetPackages.prioritize(usage),usage);
  await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-preload','ready');
}
async function asset(page:Page,path:string,headers:Record<string,string> = {}) {
  return page.evaluate(async ({path,headers})=>{
    const response=await fetch(`/assets/${path}`,{headers});const b=new Uint8Array(await response.arrayBuffer());
    return {status:response.status,version:response.headers.get('x-ose-package'),size:b.length,first:b[0],text:b.length<100 ? new TextDecoder().decode(b) : ''};
  },{path,headers});
}

test('mission control warms before VAB; both survive reload without redownloading, and Mun stays on demand', async ({page}) => {
  await open(page);await warm(page,'mission-control');
  expect(requested).toEqual(['core-0','mission-control-0','vab-0','ksc-0']);
  const b=await asset(page,'vab/vab.glb');expect(b.size).toBe(model.length);expect(b.version).toBeTruthy();
  expect(individual).toEqual([]);
  await page.reload();await ready(page);
  await warm(page,'mission-control');
  expect(requested).toHaveLength(4);
  expect((await asset(page,'vab/vab.glb')).size).toBe(model.length);
});

test('main menu scenery takes priority over speculative campus and VAB packages', async ({page}) => {
  await open(page);await warm(page,'menu');
  expect(requested).toEqual(['core-0','menu-0','ksc-0','vab-0']);
});

test('selecting the Mun loads its essential package; surface detail is requested only when used', async ({page}) => {
  await open(page);await warm(page,'planet-Mun');
  expect(requested).toEqual(['core-0','planet-Mun-0']);
  expect((await asset(page,'planets/Mun/planet.json')).text).toBe('Mun metadata');
  expect((await asset(page,'planets/Mun/detail_a0.png')).text).toBe('Mun detail');
  expect(requested).toEqual(['core-0','planet-Mun-0','planet-Mun-1']);
  expect(individual).toEqual([]);
});

test('metadata streams before the rest of a package; foreground requests share preloading and can overtake it', async ({page}) => {
  holdVab=true;await open(page);
  const warming=page.evaluate(()=>(window as any).oseAssetPackages.prioritize('vab'));
  await expect.poll(()=>requested.includes('vab-0')).toBe(true);
  expect((await asset(page,'vab/scene.json')).text).toBe('VAB metadata');
  await page.locator('[data-ose="player-name"]').fill('still responsive');
  const first=asset(page,'vab/vab.glb'),second=asset(page,'vab/vab.glb');
  expect((await asset(page,'planets/Mun/planet.json')).text).toBe('Mun metadata');
  expect(requested.filter(p=>p==='vab-0')).toHaveLength(1);
  release();await warming;
  expect((await first).size).toBe(model.length);expect((await second).size).toBe(model.length);
  expect(individual).toEqual([]);
});

test('changing usage drops queued speculative scenes and prioritizes the new destination', async ({page}) => {
  holdVab=true;await open(page);
  const old=page.evaluate(()=>(window as any).oseAssetPackages.prioritize('mission-control'));
  await expect.poll(()=>requested.includes('vab-0')).toBe(true);
  const next=page.evaluate(()=>(window as any).oseAssetPackages.prioritize('planet-Mun'));
  release();await Promise.all([old,next]);
  expect(requested).toEqual(['core-0','mission-control-0','vab-0','planet-Mun-0']);
  expect(requested).not.toContain('ksc-0');
});

test('invalid packages fall back to the individual file without large allocations or a hung request', async ({page}) => {
  broken=true;await open(page);
  const b=await asset(page,'vab/vab.glb');
  expect(b.size).toBe(model.length);expect(b.version).toBeNull();
  expect(requested.filter(p=>p==='vab-0')).toHaveLength(1);
  expect(individual).toEqual(['assets/vab/vab.glb']);
});

test('blocked persistent storage retains bounded in-memory package reuse', async ({page}) => {
  blocked=true;await open(page);await warm(page,'mission-control');
  const a=await asset(page,'vab/vab.glb'), b=await asset(page,'vab/vab.glb');
  expect(a.size).toBe(model.length);expect(b.version).toBeTruthy();
  expect(requested.filter(p=>p==='vab-0')).toHaveLength(1);expect(individual).toEqual([]);
});

test('a new export invalidates cached members even with the same Last-Modified second', async ({page}) => {
  await open(page);
  const a=await asset(page,'vab/vab.glb');
  entries['vab-0'][1].body=Buffer.alloc(model.length,9);
  await page.reload();await ready(page);
  const b=await asset(page,'vab/vab.glb',{'if-modified-since':date});
  expect(b.status).toBe(200);expect(b.first).toBe(9);expect(b.version).not.toBe(a.version);
  expect(requested.filter(p=>p==='vab-0')).toHaveLength(2);expect(individual).toEqual([]);
});

test('persistent admission evicts old members and range requests retain the ordinary HTTP path', async ({page}) => {
  await page.goto(`${base}/health`);
  await page.evaluate(async ()=>{
    const cache=await caches.open('ose-usage-assets-v1');
    for(let i=0;i<4;i++) await cache.put(`/old-${i}`,new Response('old',{headers:{'x-ose-decoded-bytes':String(64*1024*1024)}}));
  });
  await open(page);await asset(page,'vab/vab.glb');
  expect(await page.evaluate(async ()=>!!await(await caches.open('ose-usage-assets-v1')).match('/old-0'))).toBe(false);
  const response=await asset(page,'vab/vab.glb',{'range':'bytes=0-3'});
  expect(response.status).toBe(206);expect(response.size).toBe(4);expect(response.version).toBeNull();
});
