// Repeatable GPU-pass isolation at fixed viewpoints, or a pad/ascent CPU profile with OSE_FLIGHT=1.
// Skips are diagnostic only. Use a private test server and full assets, since flight mode launches a craft.
// OSE_BASE_URL=http://127.0.0.1:PORT node scripts/profile-ascent.mjs [output.json]
import { chromium } from 'playwright';
import fs from 'node:fs';

const browser = await chromium.launch({ args: ['--enable-unsafe-webgpu', '--ignore-gpu-blocklist',
  ...(process.platform === 'win32' ? ['--disable-dawn-features=use_dxc'] : [])], channel: 'chromium' });
const reports = [];
const output = process.argv[2] || '../target/ascent-profile.json';
try {
  const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Performance.enable');
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  page.on('console', m => { if (/Error while parsing WGSL|Invalid ShaderModule|Invalid RenderPipeline/.test(m.text())) errors.push(m.text()); });
  await page.addInitScript(() => {
    window.perfProbe = { frames: [], skip: '', draws: {}, enabled: false, gpu: [], gpuTiming: false };
    const encoders = new WeakMap(), commands = new WeakMap();
    const request = GPUAdapter.prototype.requestDevice;
    GPUAdapter.prototype.requestDevice = async function(desc = {}) {
      const timing = this.features.has('timestamp-query');
      const device = await request.call(this, timing ? { ...desc,
        requiredFeatures: [...new Set([...(desc.requiredFeatures || []), 'timestamp-query'])] } : desc);
      window.perfProbe.gpuTiming = timing;
      const create = device.createCommandEncoder.bind(device);
      device.createCommandEncoder = (...args) => {
        const encoder = create(...args);
        if (timing && window.perfProbe.enabled) encoders.set(encoder, { device, labels: [], query: device.createQuerySet({ type: 'timestamp', count: 64 }) });
        return encoder;
      };
      return device;
    };
    const begin = GPUCommandEncoder.prototype.beginRenderPass;
    GPUCommandEncoder.prototype.beginRenderPass = function(desc) {
      const info = encoders.get(this);
      if (info && info.labels.length < 32) {
        const i = info.labels.length * 2;
        info.labels.push(desc.label || 'pass');
        desc = { ...desc, timestampWrites: { querySet: info.query, beginningOfPassWriteIndex: i, endOfPassWriteIndex: i+1 } };
      }
      return begin.call(this, desc);
    };
    const finish = GPUCommandEncoder.prototype.finish;
    GPUCommandEncoder.prototype.finish = function(...args) {
      const info = encoders.get(this);
      if (info?.labels.length) {
        const size = info.labels.length * 16;
        info.resolve = info.device.createBuffer({ size, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC });
        info.read = info.device.createBuffer({ size, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
        this.resolveQuerySet(info.query, 0, info.labels.length*2, info.resolve, 0);
        this.copyBufferToBuffer(info.resolve, 0, info.read, 0, size);
      }
      const command = finish.apply(this, args);
      if (info?.labels.length) commands.set(command, info);
      return command;
    };
    const submit = GPUQueue.prototype.submit;
    GPUQueue.prototype.submit = function(buffers) {
      const result = submit.call(this, buffers);
      for (const b of buffers) {
        const info = commands.get(b);
        if (!info) continue;
        const sink = window.perfProbe.gpu;
        info.read.mapAsync(GPUMapMode.READ).then(() => {
          const ts = new BigUint64Array(info.read.getMappedRange());
          sink.push(info.labels.map((label,i) => [label, Number(ts[i*2+1]-ts[i*2])/1e6]));
          info.read.unmap(); info.read.destroy(); info.resolve.destroy(); info.query.destroy();
        });
      }
      return result;
    };
    const raf = window.requestAnimationFrame.bind(window);
    window.requestAnimationFrame = cb => raf(t => {
      const start = performance.now(); cb(t);
      if (window.perfProbe.enabled) window.perfProbe.frames.push([t, performance.now() - start]);
    });
    const proto = GPURenderPassEncoder.prototype;
    const pipelines = new WeakMap();
    const set = proto.setPipeline;
    proto.setPipeline = function(p) { pipelines.set(this, p.label); return set.call(this, p); };
    for (const name of ['draw', 'drawIndexed']) {
      const draw = proto[name];
      proto[name] = function(...args) {
        const label = pipelines.get(this) || '(unlabelled)';
        const probe = window.perfProbe;
        if (probe.enabled) probe.draws[label] = (probe.draws[label] || 0) + 1;
        if (probe.skip && (probe.skip === 'all' || label.includes(probe.skip))) return;
        return draw.apply(this, args);
      };
    }
  });
  const baseURL = process.env.OSE_BASE_URL || 'http://127.0.0.1:7788';
  const flight = !!process.env.OSE_FLIGHT;
  if (flight) {
    await page.goto(baseURL);
    await page.locator('[data-ose="player-name"]').fill('ascent profiler');
    await page.locator('[data-ose="play"]').click();
    await page.locator('[data-ose="session-join"]').first().click();
    await page.locator('[data-ose="ksc-vab"]').first().click();
    await page.locator('[data-ose="vab-clear"]').click();
    for (const part of ['Tin Can', 'FL-T200', 'LV-T45']) {
      await page.locator('[data-ose="vab-search"]').fill(part);
      await page.locator('[data-ose="vab-part"]').first().click();
    }
    await page.locator('[data-ose="vab-launch"]').click();
    await page.locator('[data-ose="flight"].open').waitFor();
  }
  let launched = false;
  for (const alt of (process.env.OSE_ALTS || (flight ? '0,1' : '200,10000,70000,200000')).split(',').map(Number)) {
    if (!flight) await page.goto(`${baseURL}/?planet=Kerbin&alt=${alt}&pitch=-20`);
    else if (alt > 0 && !launched) {
      await page.keyboard.press('z'); await page.keyboard.press('Space');
      launched = true;
    }
    await page.locator('body.webgpu').waitFor();
    await page.waitForTimeout(10000);
    for (const skip of (process.env.OSE_SKIPS ?? (flight ? ',all' : ',planet volume clouds,planet terrain,planet ocean,planet sky,all')).split(',')) {
      await page.evaluate(skip => { window.perfProbe.skip = skip; window.perfProbe.enabled = false; }, skip);
      await page.waitForTimeout(500);
      const before = Object.fromEntries((await cdp.send('Performance.getMetrics')).metrics.map(m => [m.name,m.value]));
      if (flight && !skip) { await cdp.send('Profiler.enable'); await cdp.send('Profiler.start'); }
      await page.evaluate(() => { const p = window.perfProbe; p.frames = []; p.draws = {}; p.gpu = []; p.enabled = true; });
      await page.waitForTimeout(3000);
      const result = await page.evaluate(() => {
        const p = window.perfProbe; p.enabled = false;
        const times = p.frames.map(f => f[0]);
        const cpu = p.frames.map(f => f[1]).sort((a,b) => a-b);
        const gaps = times.slice(1).map((t,i) => t-times[i]).sort((a,b) => a-b);
        return { frameCount: times.length, fps: (times.length-1)*1000/(times.at(-1)-times[0]),
          cpuMedianMs: cpu[Math.floor(cpu.length*.5)], cpuP95Ms: cpu[Math.floor(cpu.length*.95)],
          frameP95Ms: gaps[Math.floor(gaps.length*.95)],
          gpuTiming: p.gpuTiming, gpuMedianMs: p.gpu.map(passes => passes.reduce((a,p) => a+p[1],0)).sort((a,b)=>a-b)[Math.floor(p.gpu.length*.5)],
          drawsPerFrame: Object.fromEntries(Object.entries(p.draws).map(([k,v]) => [k,Math.round(v/times.length)])) };
      });
      const after = Object.fromEntries((await cdp.send('Performance.getMetrics')).metrics.map(m => [m.name,m.value]));
      if (flight && !skip) {
        const {profile} = await cdp.send('Profiler.stop');
        fs.writeFileSync(`${output}.${alt}.cpuprofile`,JSON.stringify(profile));
      }
      const mainThreadMsPerFrame = Object.fromEntries(['TaskDuration','ScriptDuration','LayoutDuration','RecalcStyleDuration']
        .map(k => [k, (after[k]-before[k])*1000 / result.frameCount]));
      const altitudeM = flight ? Number(await page.locator('[data-ose="flight"]').getAttribute('data-ose-altitude')) : alt;
      reports.push({ phase: flight ? (alt === 0 ? 'pad' : 'ascent') : 'planet', altitudeM, skip: skip || 'none', ...result, mainThreadMsPerFrame });
      console.log(JSON.stringify(reports.at(-1)));
      if (!skip) await page.screenshot({path: `${output}.${alt}.png`});
    }
  }
  if (errors.length) throw new Error(errors.join('\n'));
  fs.writeFileSync(output, JSON.stringify(reports, null, 2));
} finally { await browser.close(); }
