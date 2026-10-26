// Usage packages run off the rendering thread and serve the same individual asset URLs to every loader.
const CACHE = 'ose-usage-assets-v1';
const DISK_BYTES = 256 * 1024 * 1024, MEMORY_BYTES = 128 * 1024 * 1024;
const MAX_ENTRY = 64 * 1024 * 1024;
let catalog, catalogAt = 0, failedAt = 0, catalogTask, diskTask, diskBytes = 0, memoryBytes = 0;
const diskSizes = new Map(), memory = new Map(), assets = new Map(), jobs = new Map(), listeners = new Map();
let writing = Promise.resolve(), active = 0;

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', event => event.waitUntil(self.clients.claim()));

const absolute = path => new URL(path, self.location.origin + '/').href;
async function manifest(force = false) {
  if (force) {catalogAt = 0; failedAt = 0;}
  if (catalog && Date.now() - catalogAt < 30000) return catalog;
  if (Date.now() - failedAt < 30000) throw new Error('Package catalog unavailable');
  if (catalogTask) return catalogTask;
  catalogTask = (async () => {
    const response = await fetch('/assets/packages/index.json', {cache: 'no-cache'});
    if (!response.ok) throw new Error('No package catalog');
    const index = await response.json();
    if (index.version !== 1 || !index.chunks || !index.groups) throw new Error('Unsupported package catalog');
    const next = new Map();
    for (const [id, chunk] of Object.entries(index.chunks)) {
      if (!Array.isArray(chunk.assets) || !/^[a-f0-9]{16}$/.test(chunk.revision)) throw new Error('Invalid chunk');
      const bundle = new URL(chunk.url, self.location.origin + '/');
      if (bundle.origin !== self.location.origin || !bundle.pathname.startsWith('/assets/packages/')) throw new Error('Invalid bundle URL');
      chunk.id = id;
      for (const path of chunk.assets) {
        const url = new URL(path, self.location.origin + '/');
        if (url.origin !== self.location.origin || !url.pathname.startsWith('/assets/') || url.pathname.startsWith('/assets/packages/')) throw new Error('Invalid asset URL');
        if (next.has(url.href)) throw new Error('Duplicate package asset');
        next.set(url.href, chunk);
      }
    }
    assets.clear(); for (const [url, chunk] of next) assets.set(url, chunk);
    catalog = index; catalogAt = Date.now();
    return index;
  })().catch(error => {failedAt = Date.now(); throw error;}).finally(() => {catalogTask = undefined;});
  return catalogTask;
}

async function disk() {
  if (!diskTask) diskTask = (async () => {
    try {
      const cache = await caches.open(CACHE);
      await Promise.all((await cache.keys()).map(async key => {
        const response = await cache.match(key);
        const size = Number(response?.headers.get('x-ose-decoded-bytes'));
        if (Number.isSafeInteger(size) && size >= 0 && size <= MAX_ENTRY) {
          diskSizes.set(key.url, size); diskBytes += size;
        } else await cache.delete(key);
      }));
      return cache;
    } catch { return null; }
  })();
  return diskTask;
}
async function cached(url, revision) {
  const held = memory.get(url);
  if (held?.revision === revision) return held.response.clone();
  try {
    const cache = await disk();
    const response = await cache?.match(url);
    if (response?.headers.get('x-ose-package') === revision) return response;
  } catch { /* Quota/storage restrictions leave normal loading available. */ }
  return null;
}
function hold(url, response, revision, size) {
  const old = memory.get(url);
  if (old) {memoryBytes -= old.size; memory.delete(url);}
  while (memoryBytes + size > MEMORY_BYTES && memory.size) {
    const [key, value] = memory.entries().next().value;
    memory.delete(key); memoryBytes -= value.size;
  }
  memory.set(url, {response, revision, size}); memoryBytes += size;
}
async function save(url, bytes, meta, revision) {
  const headers = {'content-type':meta.type, 'cache-control':'no-cache',
    'last-modified':meta.modified, 'etag':`"${revision}"`, 'x-ose-package':revision, 'x-ose-decoded-bytes':String(bytes.length)};
  if (meta.preview) headers['x-ose-preview'] = '1';
  const response = new Response(bytes, {headers});
  const write = writing.then(async () => {
    if (assets.get(url)?.revision !== revision) return;
    const cache = await disk();
    let stored = false;
    if (cache) try {
      if (diskSizes.has(url)) {diskBytes -= diskSizes.get(url); diskSizes.delete(url);}
      while (diskBytes + bytes.length > DISK_BYTES && diskSizes.size) {
        const [key, size] = diskSizes.entries().next().value;
        await cache.delete(key); diskSizes.delete(key); diskBytes -= size;
      }
      await cache.put(url, response.clone());
      diskSizes.set(url, bytes.length); diskBytes += bytes.length; stored = true;
    } catch { /* Use bounded memory if persistent storage fails. */ }
    if (!stored) hold(url, response, revision, bytes.length);
    for (const listener of listeners.get(url) || []) {
      if (listener.revision === revision) listener.resolve(true);
    }
  });
  writing = write.catch(() => {});
  await write;
}
function watch(url, revision) {
  let resolve;
  const promise = new Promise(r => {resolve = r;});
  const listener = {revision, resolve};
  if (!listeners.has(url)) listeners.set(url, new Set());
  listeners.get(url).add(listener);
  return {promise, cancel() {
    const group = listeners.get(url); group?.delete(listener);
    if (!group?.size) listeners.delete(url);
  }};
}

// Decode one file at a time, without concatenating/decompressing a whole scene on the main thread.
class Reader {
  constructor(stream) {this.reader = stream.getReader(); this.bytes = new Uint8Array(); this.offset = 0;}
  async take(size) {
    const output = new Uint8Array(size); let at = 0;
    while (at < size) {
      if (this.offset === this.bytes.length) {
        const next = await this.reader.read();
        if (next.done) throw new Error('Truncated package');
        this.bytes = next.value; this.offset = 0;
      }
      const n = Math.min(size - at, this.bytes.length - this.offset);
      output.set(this.bytes.subarray(this.offset, this.offset+n), at);
      this.offset += n; at += n;
    }
    return output;
  }
  async word() {const b = await this.take(4); return new DataView(b.buffer).getUint32(0,true);}
}
async function transfer(job) {
  const chunk = job.chunk;
  if ((await Promise.all(chunk.assets.map(url => cached(absolute(url), chunk.revision)))).every(Boolean)) return true;
  const response = await fetch(absolute(chunk.url), {priority:job.urgent ? 'high' : 'low'});
  if (!response.ok || !response.body) throw new Error('Package unavailable');
  const reader = new Reader(response.body), decoder = new TextDecoder();
  try {
    if (decoder.decode(await reader.take(8)) !== 'OSEPACK1') throw new Error('Invalid package');
    const expected = new Set(chunk.assets.map(absolute));
    for (;;) {
      const length = await reader.word();
      if (!length) {if (expected.size) throw new Error('Missing package entries'); break;}
      if (length > 16384) throw new Error('Oversized package header');
      const meta = JSON.parse(decoder.decode(await reader.take(length)));
      const url = absolute(meta.url);
      if (!expected.delete(url) || !Number.isSafeInteger(meta.size) || meta.size < 0 || meta.size > MAX_ENTRY) throw new Error('Invalid package entry');
      const bytes = await reader.take(meta.size);
      await save(url, bytes, meta, chunk.revision);
    }
    return true;
  } finally {await reader.reader.cancel().catch(() => {});}
}
function pump() {
  for (;;) {
    const next = [...jobs.values()].filter(j => !j.running).sort((a,b) => b.priority-a.priority)[0];
    // One speculative transfer. A demanded package may run alongside it with high fetch priority.
    if (!next || active >= 2 || !next.urgent && active) return;
    next.running = true; active++;
    transfer(next).catch(() => false).then(next.resolve).finally(() => {
      jobs.delete(next.chunk.url); active--; pump();
    });
  }
}
function ensure(chunk, priority, urgent = false) {
  let job = jobs.get(chunk.url);
  if (!job) {
    let resolve; const promise = new Promise(r => {resolve=r;});
    job = {chunk, priority, urgent, promise, resolve, running:false}; jobs.set(chunk.url, job);
  } else {job.priority = Math.max(job.priority, priority); job.urgent ||= urgent;}
  // Let an entire usage plan enter the queue before choosing its first request.
  queueMicrotask(pump);
  return job.promise;
}
async function warm(usage) {
  const index = await manifest();
  const order = usage === 'menu' ? [['core',90],['menu',80],['ksc',30],['vab',10]]
    : usage === 'mission-control' ? [['mission-control',90],['vab',20],['ksc',10]]
    : usage === 'vab' ? [['vab',90],['planet-Kerbin',10]]
    : usage === 'ksc' ? [['ksc',90],['vab',10]] : [[usage,90]];
  const wanted = new Map();
  function group(name, priority, visited = new Set()) {
    if (visited.has(name)) return;
    visited.add(name);
    const g = index.groups[name]; if (!g) return;
    for (const dependency of g.depends || []) group(dependency, priority, visited);
    for (const id of g.prefetch || []) wanted.set(id, Math.max(wanted.get(id) || 0, priority));
  }
  for (const [name, priority] of order) group(name, priority);
  for (const [key, job] of jobs) {
    if (!job.urgent && !job.running && !wanted.has(job.chunk.id)) {
      jobs.delete(key); job.resolve(false);
    } else if (!job.urgent && wanted.has(job.chunk.id)) job.priority = wanted.get(job.chunk.id);
  }
  return (await Promise.all([...wanted].map(([id,p]) => ensure(index.chunks[id],p)))).every(Boolean);
}
async function asset(request) {
  try {await manifest();} catch {return fetch(request);}
  const chunk = assets.get(request.url);
  if (!chunk) return fetch(request);
  let response = await cached(request.url, chunk.revision);
  if (!response) {
    const waiting = watch(request.url, chunk.revision);
    try {await Promise.race([waiting.promise, ensure(chunk,100,true)]);}
    finally {waiting.cancel();}
    response = await cached(request.url, chunk.revision);
  }
  if (!response) return fetch(request); // A broken/old package never prevents ordinary loading.
  // Revisions also distinguish exports made within the same Last-Modified second.
  if (request.headers.get('if-none-match') === `"${chunk.revision}"`) {
    return new Response(null, {status:304,headers:response.headers});
  }
  return response;
}
self.addEventListener('fetch', event => {
  const request = event.request, url = new URL(request.url);
  if (request.method === 'GET' && url.origin === self.location.origin && url.pathname.startsWith('/assets/')
      && assets.has(request.url) && !request.headers.has('range')) event.respondWith(asset(request));
});
self.addEventListener('message', event => {
  const data = event.data, port = event.ports[0];
  if (!port || !['init','usage'].includes(data?.type)) return;
  const task = (async () => {
    if (data.type === 'init') await self.clients.claim();
    const index = await manifest(data.type === 'init');
    const supported = Object.keys(index.chunks).length > 0;
    const ready = data.type === 'usage' && supported ? await warm(data.usage) : supported;
    port.postMessage({supported,ready});
  })().catch(() => port.postMessage({supported:false,ready:false}));
  event.waitUntil(task);
});
