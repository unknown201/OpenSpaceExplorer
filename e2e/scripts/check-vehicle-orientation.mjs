// Private local server only: creates disposable sandbox campaigns and launches stock vehicles.
// OSE_BASE_URL=http://127.0.0.1:7799 node e2e/scripts/check-vehicle-orientation.mjs
import { chromium } from 'playwright';
import fs from 'node:fs';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';

const out = new URL('../../.cache/vehicle-orientation/', import.meta.url);
fs.mkdirSync(out, { recursive: true });
const browser = await chromium.launch({
  channel: 'chromium',
  executablePath: process.env.OSE_CHROMIUM_EXECUTABLE,
  args: ['--enable-unsafe-webgpu', '--ignore-gpu-blocklist', '--disable-dawn-features=use_dxc'],
});
try {
  const vehicles = process.env.OSE_VEHICLES?.split(',') || ['Stock04_Jet', 'Stock03_Drone', 'Kerbal K2'];
  for (const name of vehicles) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    await page.addInitScript(() => localStorage.setItem('ose.ftue', 'off'));
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(process.env.OSE_BASE_URL || 'http://127.0.0.1:7799');
    await page.locator('[data-ose="player-name"]').fill('vehicle verification');
    await page.locator('[data-ose="play"]').click();
    await page.locator('[data-ose="create-session"] > summary').click();
    await page.locator('[data-ose="create-mode"]').selectOption('Sandbox');
    await page.locator('[data-ose="create-name"]').fill('Vehicle orientation audit');
    await page.locator('[data-ose="create"]').click();
    if (await page.locator('#ftue-off').isVisible()) await page.locator('#ftue-off').click();
    await page.locator('[data-ose="ksc-vab"]').first().click();
    await page.locator('[data-ose="vab-load"]').click();
    await page.locator('[data-ose="vab-load-craft"]').first().waitFor();
    await page.locator('[data-ose="vab-load-craft"]').filter({ hasText: name }).click();
    await page.waitForTimeout(5000);
    await page.screenshot({ path: fileURLToPath(new URL(`${name}-editor.png`, out)) });
    await page.locator('[data-ose="vab-site"]').selectOption(name === 'Kerbal K2' ? 'LaunchPad_1' : 'Runway_1');
    await page.locator('[data-ose="vab-launch"]').click();
    await page.locator('[data-ose="flight"].open').waitFor();
    await page.waitForTimeout(7000);
    if (name === 'Kerbal K2') {
      await page.mouse.move(720, 450);
      await page.mouse.wheel(0, 650);
      await page.waitForTimeout(500);
    }
    console.log(name, await page.locator('[data-ose="flight"]').getAttribute('data-ose-situation'));
    await page.screenshot({ path: fileURLToPath(new URL(`${name}-ground.png`, out)) });
    assert.ok(await page.locator('body').evaluate(el => el.classList.contains('webgpu')), 'WebGPU active');
    if (name === 'Stock04_Jet') {
      await page.keyboard.press('z');
      await page.keyboard.press('Space');
      await page.keyboard.down('s');
      await page.waitForTimeout(500);
      await page.keyboard.up('s');
      await page.waitForFunction(() => document.querySelector('[data-ose="flight"]').getAttribute('data-ose-situation') === 'Flying', null, { timeout: 90000 });
      await page.waitForTimeout(3000);
      await page.screenshot({ path: fileURLToPath(new URL(`${name}-airborne.png`, out)) });
      console.log('Jet takeoff verified:', await page.locator('#fh-attitude').innerText());
    }
    assert.deepEqual(errors, []);
    await page.close();
  }
} finally {
  await browser.close();
}
