// Measure cold/warm critical-scene transfers with one browser-wide bandwidth budget.
// OSE_BASE_URL=http://127.0.0.1:PORT OSE_DOWNLOAD_MBPS=7.5 node scripts/profile-loading.mjs output.json
import { chromium } from 'playwright';
import fs from 'node:fs';
import {networkProxy} from './network-proxy.mjs';
const upstream = process.env.OSE_BASE_URL || 'http://127.0.0.1:7788';
const mbps = Number(process.env.OSE_DOWNLOAD_MBPS || 30);
const output = process.argv[2] || '../target/loading-profile.json';
const game = !!process.env.OSE_GAME;
// "ready" waits for all menu prefetches; a number simulates time spent choosing a name/campaign.
const menuWait = process.env.OSE_MENU_WAIT_MS || '0';
const proxy = game ? await networkProxy(upstream,mbps,30) : null;
const base = proxy?.base || upstream;
const browser = await chromium.launch({channel: 'chromium', args: ['--enable-unsafe-webgpu', '--ignore-gpu-blocklist',
  ...(process.platform === 'win32' ? ['--disable-dawn-features=use_dxc'] : [])]});
try {
  const page = await browser.newPage({serviceWorkers:process.env.OSE_PACKAGES === '0' ? 'block' : 'allow'});
  // Keep the menu's randomly selected stock craft identical between transport comparisons.
  await page.addInitScript(() => {Math.random = () => 0.5;});
  page.setDefaultTimeout(180000);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Network.enable');
  if (!proxy) await cdp.send('Network.emulateNetworkConditions', {
    offline: false, latency: 30, downloadThroughput: mbps * 1e6 / 8, uploadThroughput: mbps * 1e6 / 8,
  });
  const requests = new Map();
  const records = [];
  cdp.on('Network.responseReceived', ({requestId, response}) => {
    requests.set(requestId, {requestId, url: response.url, status: response.status, headerBytes: response.encodedDataLength || 0, encoding: response.headers['content-encoding'],
      cache: response.fromDiskCache || response.fromPrefetchCache || false});
    if (game && /\/(ksc\/ksc|vab\/vab)\.glb$/.test(response.url)) console.log('scene response', response.status, response.url);
  });
  cdp.on('Network.loadingFinished', ({requestId, encodedDataLength}) => {
    const response = requests.get(requestId);
    if (response) records.push({...response, wireBytes: encodedDataLength});
  });
  await page.goto(`${base}/health`);
  const phases = [];
  for (const phase of ['cold', 'warm']) {
    const startBytes=proxy?.transferredBytes || 0;
    for (const worker of page.context().serviceWorkers()) await worker.evaluate(() => performance.clearResourceTimings()).catch(() => {});
    records.length = 0;
    requests.clear();
    let result;
    if (game) {
      const start = performance.now();
      await page.goto(base);
      await page.locator('body.webgpu').waitFor();
      await page.locator('[data-ose="player-name"]').fill('loading profiler');
      await page.locator('[data-ose="play"]').click();
      if (menuWait === 'ready') await page.locator('[data-ose="gpu"][data-ose-preload="ready"]').waitFor();
      else if (Number(menuWait) > 0) await page.waitForTimeout(Number(menuWait));
      const joinedAt = performance.now();
      await page.locator('[data-ose="session-join"]').first().click();
      await page.locator('[data-ose="gpu"][data-ose-textured-scene="ksc"]').waitFor();
      const kscAt = performance.now();
      await page.locator('[data-ose="ksc-vab"]').first().click();
      await page.locator('[data-ose="gpu"][data-ose-textured-scene="vab"]').waitFor();
      // Includes persistent-cache writes and model parsing/upload after the final transfer.
      await page.waitForFunction(async () => {
        for (const name of ['ose-large-assets-v1','ose-usage-assets-v1']) {
          if (await (await caches.open(name)).match('assets/vab/vab.glb')) return true;
        }
        return false;
      }, undefined, {timeout:30000});
      result = {elapsedMs: performance.now()-start, menuMs: joinedAt-start,
        joinToKscMs: kscAt-joinedAt, kscToVabMs: performance.now()-kscAt};
    } else result = await page.evaluate(async () => {
      const start = performance.now();
      const models = [];
      for (const path of ['/assets/ksc/ksc.glb', '/assets/vab/vab.glb']) {
        const at = performance.now();
        const response = await fetch(path);
        if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
        const bytes = new Uint8Array(await response.arrayBuffer());
        if (String.fromCharCode(...bytes.slice(0,4)) !== 'glTF') throw new Error(`${path}: not a decoded GLB`);
        models.push({path, decodedBytes: bytes.length, elapsedMs: performance.now() - at});
      }
      return {elapsedMs: performance.now() - start, models};
    });
    await page.waitForTimeout(100);
    const sceneRequests = [...requests.values()].filter(r => r.url.endsWith('/ksc/ksc.glb') || r.url.endsWith('/vab/vab.glb'))
      .map(r => ({...r,wireBytes:records.find(done => done.requestId === r.requestId)?.wireBytes ?? r.headerBytes}));
    const packages = (await Promise.all(page.context().serviceWorkers().map(worker => worker.evaluate(() =>
      performance.getEntriesByType('resource').filter(e => /\/assets\/packages\//.test(e.name)).map(e => {
        const r=e;
        return {url:r.name,wireBytes:r.transferSize,encodedBodyBytes:r.encodedBodySize,elapsedMs:r.duration};
      })).catch(() => [])))).flat();
    const report = {phase, ...result, wireBytes: sceneRequests.reduce((sum, r) => sum + r.wireBytes, 0),
      httpBodyBytes:proxy ? proxy.transferredBytes-startBytes : undefined, requests: sceneRequests, packages};
    phases.push(report);
    console.log(JSON.stringify(report));
    // Stop speculative work before the next sample; persistent member caches survive worker restart.
    if (proxy) {await cdp.send('ServiceWorker.enable'); await cdp.send('ServiceWorker.stopAllWorkers');}
  }
  fs.writeFileSync(output, JSON.stringify({mbps, addedLatencyMs: 30, base:upstream, game, menuWait,
    sharedHttpBudget:!!proxy, packagesEnabled:process.env.OSE_PACKAGES !== '0', phases}, null, 2));
} finally { await browser.close(); await proxy?.close(); }
