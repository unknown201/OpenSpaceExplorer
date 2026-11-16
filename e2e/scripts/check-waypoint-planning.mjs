// Isolated real-server/browser regression for planning, coasting warp and cancellation.
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const root = fileURLToPath(new URL('../..', import.meta.url));
const before = Boolean(process.env.OSE_BEFORE);
const tag = before ? 'before' : 'after';
const sharedRoot = process.env.OSE_SHARED_ROOT ?? root;
const out = path.join(sharedRoot, '.cache/waypoint-release-validation');
await fs.mkdir(out, { recursive: true });
const port = Number(process.env.OSE_WAYPOINT_PORT ?? 7798);
const base = `http://127.0.0.1:${port}`;
const server = spawn(process.env.OSE_SERVER ?? path.join(sharedRoot, 'target/debug/ose-server.exe'), [
  '--listen', `127.0.0.1:${port}`, '--static-dir', process.env.OSE_STATIC_DIR ?? path.join(sharedRoot, 'target/waypoint-release-web'),
  '--assets-dir', path.join(sharedRoot, 'assets/web'), '--save', 'none', '--db', ':memory:', '--allow-guest-accounts',
], { cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
let log = '', browser, page;
const result = { tag, errors: [] };
server.stdout.on('data', b => log += b);
server.stderr.on('data', b => log += b);
try {
  for (let i = 0; ; i++) {
    if (server.exitCode !== null) throw new Error(log);
    if (await fetch(`${base}/health`).then(r => r.ok).catch(() => false)) break;
    if (i > 1200) throw new Error('Server did not start');
    await new Promise(r => setTimeout(r, 100));
  }
  browser = await chromium.launch({ channel: 'chromium', headless: true, args: [
    '--enable-unsafe-webgpu', '--ignore-gpu-blocklist', '--disable-dawn-features=use_dxc',
  ] });
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  const predictions = new Map();
  const schema = (await fs.readFile(path.join(root, 'crates/ose-net/src/lib.rs'), 'utf8')).split('pub enum ServerMessage {')[1].split(/^}/m)[0];
  const trajectoryTag = [...schema.matchAll(/^    ([A-Z]\w*)(?: \{|,)/gm)].map(m => m[1]).indexOf('Trajectory');
  // Read-only postcard telemetry; derive the enum tag from the current protocol schema.
  // This verifies that the browser actually receives the multi-revolution patch under test.
  page.on('websocket', socket => socket.on('framereceived', ({ payload }) => {
    if (typeof payload === 'string' || payload[0] !== trajectoryTag) return;
    let offset = 1;
    const uint = () => { let n = 0, scale = 1, byte; do {
      byte = payload[offset++]; n += (byte & 127) * scale; scale *= 128;
    } while (byte & 128); return n; };
    const float = () => { const n = payload.readDoubleLE(offset); offset += 8; return n; };
    const tick = uint(), vessel = uint(), count = uint(), patches = [];
    for (let i = 0; i < count; i++) {
      const body = uint(), elements = Array.from({ length: 7 }, float);
      patches.push({ body, elements, start: float(), end: float(), transition: uint() });
    }
    predictions.set(vessel, { tick, patches });
  }));
  page.setDefaultTimeout(15000);
  await page.addInitScript(() => localStorage.setItem('ose.ftue', 'off'));
  page.on('pageerror', e => result.errors.push(e.message));
  await page.goto(base);
  await page.locator('[data-ose="player-name"]').fill('Waypoint regression');
  const login = page.locator('[data-ose="login"]');
  if (await login.isVisible()) await login.click();
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="create-session"] summary').first().click();
  await page.locator('[data-ose="create-mode"]').selectOption('Sandbox');
  await page.locator('[data-ose="create-name"]').fill('Waypoint regression');
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-launchpad"]').click();
  await page.locator('[data-ose="craft"]').filter({ hasText: 'Stock12_StartRocket' }).click();
  await page.locator('[data-ose="launch-altitude"]').fill('300');
  await page.locator('[data-ose="lp-orbit"]').click();
  await page.waitForFunction(() => document.querySelector('#status')?.textContent.includes('launched Stock12_StartRocket'));
  if (!await page.locator('#flight-hud.open').count()) {
    await page.locator('[data-ose="ksc-button"]').click();
    await page.locator('[data-ose="ksc-tracking"]').click();
    await page.locator('[data-ose="ts-vessel"].mine').filter({ hasText: 'Stock12_StartRocket' }).last().click();
    await page.locator('[data-ose="ts-fly"]').click();
  }
  await page.waitForFunction(() => document.querySelector('#fh-throttle-fill')?.style.height === '0%');
  assert.equal(await page.locator('[data-ose="gpu"]').getAttribute('data-ose-renderer'), 'webgpu');
  await page.keyboard.press('m');
  await page.locator('[data-ose="add-node"]').click();
  const node = page.locator('[data-ose="maneuver-node"]');
  const setRange = async (id, value) => page.locator(`#${id}`).evaluate((el, v) => {
    const maxTime = 10 * 426 * 21600;
    el.value = String(el.id === 'node-dt' ? 10000 * Math.log1p(v / 60) / Math.log1p(maxTime / 60) : v);
    el.dispatchEvent(new Event('input', { bubbles: true }));
  }, value);
  await setRange('node-dt', 400);
  await setRange('node-prograde', 40);
  // Make a fractional burn through the actual fine-drag gizmo. A timing edit must preserve it.
  await page.waitForTimeout(300);
  const nb = await node.boundingBox();
  const hb = await page.locator('#mnv-prograde').boundingBox();
  const hx = hb.x + hb.width / 2, hy = hb.y + hb.height / 2;
  const dx = hx - nb.x - nb.width / 2, dy = hy - nb.y - nb.height / 2;
  const length = Math.hypot(dx, dy);
  await page.keyboard.down('Shift');
  await page.mouse.move(hx, hy);
  await page.mouse.down();
  await page.mouse.move(hx + 19 * dx / length, hy + 19 * dy / length, { steps: 5 });
  await page.waitForTimeout(180);
  await page.mouse.up();
  await page.keyboard.up('Shift');
  result.fineDragThrottle = await page.locator('#fh-throttle-fill').evaluate(e => e.style.height);
  if (!before) assert.equal(result.fineDragThrottle, '0%', 'Fine-drag modifiers must not open the engines');
  const initialPrograde = await node.getAttribute('data-ose-prograde');
  assert.notEqual(Number(initialPrograde) % 1, 0, 'Fixture must contain a fine-drag fractional burn');
  const rangeValue = async (slider, id) => {
    const v = Number(await slider.inputValue());
    return id === 'node-dt' ? 60 * Math.expm1(v / 10000 * Math.log1p(10 * 426 * 21600 / 60)) : v;
  };
  const initialUT = Number(await node.getAttribute('data-ose-ut'));
  const time = page.locator('#node-dt');
  const rect = await time.boundingBox();
  await page.mouse.move(rect.x + rect.width * 0.3, rect.y + rect.height / 2);
  await page.mouse.down();
  await page.mouse.move(rect.x + rect.width * 0.6, rect.y + rect.height / 2, { steps: 30 });
  await page.mouse.up();
  result.timing = { initialUT, editedUT: Number(await node.getAttribute('data-ose-ut')), initialPrograde, prograde: await node.getAttribute('data-ose-prograde') };
  assert.notEqual(result.timing.editedUT, initialUT);
  if (!before) assert.equal(result.timing.prograde, initialPrograde, 'Timing edits must preserve the exact burn');
  await setRange('node-dt', 25);
  result.shortTiming = await rangeValue(time, 'node-dt');
  if (!before) assert.ok(Math.abs(result.shortTiming - 25) < 1e-6, 'Planning must support the next sixty seconds');
  if (!before) {
    result.modifiers = {};
    for (const id of ['node-dt', 'node-prograde', 'node-normal', 'node-radial']) {
      const slider = page.locator(`#${id}`);
      await setRange(id, 100);
      await slider.focus();
      await page.keyboard.press('ArrowRight');
      assert.ok(Math.abs(await rangeValue(slider, id) - 101) < 1e-6);
      await page.keyboard.press('Shift+ArrowRight');
      assert.ok(Math.abs(await rangeValue(slider, id) - 111) < 1e-6);
      await page.keyboard.press('Control+ArrowLeft');
      assert.ok(Math.abs(await rangeValue(slider, id) - 110.9) < 1e-6);
      await page.keyboard.press('Control+Shift+ArrowDown');
      assert.ok(Math.abs(await rangeValue(slider, id) - 109.9) < 1e-6);
      // Modifier presses must not leak into the ship's throttle on later frames.
      await page.waitForTimeout(150);
      assert.equal(await page.locator('#fh-throttle-fill').evaluate(e => e.style.height), '0%');
      result.modifiers[id] = await rangeValue(slider, id);
    }
  }
  await page.locator('[data-ose="flight-throttle-cut"]').click();
  // Give the authoritative coast-to-rails handoff time to settle before comparing paths.
  for (let i = 0; i < 3; i++) await page.locator('[data-ose="warp"]').click();
  await page.waitForTimeout(3000);
  await page.keyboard.press('/');
  // A maneuver days away used to sample hundreds of turns into one 240-segment strip.
  // Widen the slider to exercise this valid server-side planning horizon directly.
  await setRange('node-dt', 21 * 21600 + 5 * 3600 + 475);
  await setRange('node-prograde', 477);
  await setRange('node-normal', 0);
  await setRange('node-radial', 0);
  await page.waitForTimeout(1500);
  result.longHorizon = [...predictions.entries()].flatMap(([vessel, prediction]) =>
    prediction.patches.filter(p => p.transition === 3).map(p => ({
      vessel, span: p.end - p.start,
      revolutions: (p.end - p.start) / (2 * Math.PI * Math.sqrt(p.elements[0] ** 3 / 3.5316e12)),
    })));
  assert.ok(result.longHorizon.some(p => p.revolutions > 100), 'The renderer must receive the long-horizon maneuver');
  await page.screenshot({ path: path.join(out, `${tag}-long-horizon.png`) });
  await page.locator('[data-ose="node-warp"]').click();
  await page.waitForTimeout(650);
  await page.screenshot({ path: path.join(out, `${tag}-long-horizon-warp.png`) });
  await page.locator('[data-ose="clear-node"]').click();
  await page.keyboard.press('/');
  await page.waitForTimeout(400);
  await page.locator('[data-ose="add-node"]').click();
  await setRange('node-prograde', 40);
  await setRange('node-dt', 600);
  // Reproduce warping with a previously open engine throttle.
  await page.locator('[data-ose="flight-throttle-full"]').click();
  await page.waitForFunction(() => document.querySelector('#fh-throttle-fill')?.style.height === '100%');
  await page.locator('[data-ose="node-warp"]').click();
  await page.waitForTimeout(1200);
  result.warpStart = await page.evaluate(() => ({
    throttle: document.querySelector('#fh-throttle-fill').style.height,
    requested: document.querySelector('#warp').getAttribute('data-ose-requested-warp'),
    stopVisible: Boolean(document.querySelector('#warp-stop')),
    nodeButton: document.querySelector('#node-warp').textContent,
  }));
  await page.screenshot({ path: path.join(out, `${tag}.png`) });
  await page.locator('[data-ose="warp-down"]').click();
  const manual = await page.locator('#warp').getAttribute('data-ose-requested-warp');
  await page.waitForTimeout(1500);
  result.manualWarp = { requested: manual, later: await page.locator('#warp').getAttribute('data-ose-requested-warp') };
  if (!before) {
    assert.equal(result.warpStart.throttle, '0%', 'Warp-to must coast with engines off');
    assert.equal(result.warpStart.stopVisible, true, 'A visible stop control must exist');
    assert.equal(result.warpStart.nodeButton, 'Stop warp');
    assert.equal(result.manualWarp.later, manual, 'Manual warp changes must cancel auto-warp');
    await page.locator('[data-ose="warp-stop"]').click();
    await page.waitForFunction(() => document.querySelector('#warp').textContent === '×1');
    const coast = await page.evaluate(() => ({
      fuel: document.querySelector('#fh-fuel').textContent,
      apoapsis: document.querySelector('#fh-ap').textContent,
      periapsis: document.querySelector('#fh-pe').textContent,
    }));
    // A second warp on the stable, unpowered orbit must consume no fuel or change its apsides.
    await page.locator('[data-ose="node-warp"]').click();
    await page.waitForTimeout(900);
    await page.locator('[data-ose="warp-stop"]').click();
    await page.waitForTimeout(500);
    result.coast = await page.evaluate(() => ({
      fuel: document.querySelector('#fh-fuel').textContent,
      apoapsis: document.querySelector('#fh-ap').textContent,
      periapsis: document.querySelector('#fh-pe').textContent,
    }));
    assert.deepEqual(result.coast, coast, 'Coasting warp must preserve fuel and the actual orbit');
    await page.locator('[data-ose="node-warp"]').click();
    await page.waitForFunction(() => document.querySelector('#node-warp').textContent === 'Stop warp');
    await page.locator('[data-ose="node-warp"]').click();
    await page.waitForTimeout(1200);
    assert.equal(await page.locator('#warp').textContent(), '×1');
    assert.equal(await page.locator('#node-warp').textContent(), 'Warp to');
    await page.locator('[data-ose="node-warp"]').click();
    await page.locator('[data-ose="flight-throttle-full"]').click();
    await page.waitForTimeout(300);
    assert.equal(await page.locator('#warp').textContent(), '×1', 'Manual thrust cancels automatic warp');
    assert.equal(await page.locator('#node-warp').textContent(), 'Warp to');
    await page.locator('[data-ose="flight-throttle-cut"]').click();
    // Slash still stops warp after the player has focused a range input.
    await page.locator('[data-ose="node-warp"]').click();
    await time.focus();
    await page.keyboard.press('/');
    await page.waitForTimeout(1200);
    assert.equal(await page.locator('#warp').textContent(), '×1');
    assert.equal(await page.locator('#node-warp').textContent(), 'Warp to');
    // Editing/deleting an active warp target must stop using the obsolete target.
    await page.locator('[data-ose="node-warp"]').click();
    await setRange('node-dt', 500);
    await page.waitForTimeout(1000);
    assert.equal(await page.locator('#warp').textContent(), '×1');
    await page.locator('[data-ose="node-warp"]').click();
    await page.locator('[data-ose="clear-node"]').click();
    await page.waitForTimeout(1000);
    assert.equal(await page.locator('#warp').textContent(), '×1');
    assert.equal(await node.count(), 0);
    // Let a short warp complete on its own: it must return to 1x before the maneuver,
    // with engines still off and the plan still available to the player.
    await page.locator('[data-ose="add-node"]').click();
    await setRange('node-dt', 90);
    await setRange('node-prograde', 40);
    await page.locator('[data-ose="node-warp"]').click();
    await page.waitForFunction(() => document.querySelector('#node-warp').textContent === 'Stop warp');
    await page.waitForFunction(() => document.querySelector('#node-warp').textContent === 'Warp to', null, { timeout: 60000 });
    result.arrival = await page.evaluate(() => ({
      warp: document.querySelector('#warp').textContent,
      throttle: document.querySelector('#fh-throttle-fill').style.height,
      countdown: document.querySelector('#node-dt-text').textContent,
    }));
    assert.equal(result.arrival.warp, '×1');
    assert.equal(result.arrival.throttle, '0%');
    assert.match(result.arrival.countdown, /^T-0:(1[3-9]|2[0-5])$/, 'Arrival must leave time to prepare the maneuver');
    await page.locator('[data-ose="clear-node"]').click();
    assert.deepEqual(result.errors, []);
    result.passed = true;
  }
} catch (error) {
  if (page) {
    await page.screenshot({ path: path.join(out, 'failure.png') }).catch(() => {});
    console.log('Active screens', await page.locator('.screen.open').evaluateAll(es => es.map(e => e.id)));
  }
  throw error;
} finally {
  await browser?.close();
  server.kill();
  await fs.writeFile(path.join(out, `${tag}.json`), JSON.stringify(result, null, 2));
  await fs.writeFile(path.join(out, `${tag}-server.log`), log);
  console.log(JSON.stringify(result, null, 2));
}
