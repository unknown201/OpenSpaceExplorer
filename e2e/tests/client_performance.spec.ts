import { expect, test } from '@playwright/test';

test('graphics frame cap persists and idle views batch uploads without refraction copies', async ({ page, browserName }) => {
  test.skip(browserName !== 'chromium', 'The WebGPU probe uses Chromium GPUQueue instrumentation.');
  await page.addInitScript(() => {
    const { GPUQueue, GPUCommandEncoder } = window as any;
    const probe = { frames: 0, writes: 0, copies: 0 };
    (window as any).renderProbe = probe;
    const submit = GPUQueue.prototype.submit;
    GPUQueue.prototype.submit = function (...args: any[]) {
      probe.frames++;
      return submit.apply(this, args);
    };
    const write = GPUQueue.prototype.writeBuffer;
    GPUQueue.prototype.writeBuffer = function (buffer: any, ...args: any[]) {
      if (buffer.label === 'frame objects') probe.writes++;
      return write.call(this, buffer, ...args);
    };
    const copy = GPUCommandEncoder.prototype.copyTextureToTexture;
    GPUCommandEncoder.prototype.copyTextureToTexture = function (...args: any[]) {
      probe.copies++;
      return copy.apply(this, args);
    };
  });
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => {
    if (/Invalid (RenderPipeline|BindGroup)|Validation Error|Error while parsing WGSL/.test(message.text()))
      errors.push(message.text());
  });
  await page.goto('/');
  await expect(page.locator('body')).toHaveClass(/webgpu/);
  await expect(page.locator('[data-ose="gpu"]')).toHaveAttribute('data-ose-menu-vessel', /.+/);
  await expect(page.locator('#settings-frame-limit')).toHaveValue('60');
  // Open Graphics through the real menu and change the live render cadence.
  await page.locator('#main-settings').click();
  await page.locator('#settings-tabs [data-tab="graphics"]').click();
  await page.locator('#settings-fps').check();
  await expect(page.locator('#fps')).toContainText('frame max');
  await page.locator('#settings-frame-limit').selectOption('30');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('ose.frameLimit'))).toBe('30');
  await page.waitForTimeout(1500); // let texture arrivals settle before counting frame work
  const sample = await page.evaluate(async () => {
    const probe = (window as any).renderProbe;
    const before = { ...probe }, start = performance.now();
    await new Promise(resolve => setTimeout(resolve, 1500));
    return { fps: (probe.frames - before.frames) * 1000 / (performance.now() - start),
      frames: probe.frames - before.frames, writes: probe.writes - before.writes,
      copies: probe.copies - before.copies };
  });
  expect(sample.frames).toBeGreaterThan(5);
  expect(sample.fps).toBeLessThanOrEqual(32);
  expect(sample.writes).toBe(sample.frames);
  expect(sample.copies).toBe(0);
  await page.reload();
  await expect(page.locator('#settings-frame-limit')).toHaveValue('30');
  await page.locator('#main-settings').click();
  await page.locator('#settings-tabs [data-tab="graphics"]').click();
  await page.locator('#settings-frame-limit').selectOption('120');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('ose.frameLimit'))).toBe('120');
  await page.locator('#settings-frame-limit').selectOption('0');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('ose.frameLimit'))).toBe('0');
  expect(errors).toEqual([]);
  console.log('client render work', JSON.stringify(sample));
});

test('held flight controls still flush on press and release with the 30 FPS render cap', async ({ page, browserName }) => {
  test.skip(browserName !== 'chromium', 'Requires the production WebGPU renderer.');
  const rotations: Buffer[] = [];
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => localStorage.setItem('ose.frameLimit', '30'));
  await page.routeWebSocket('**/ws', socket => {
    const server = socket.connectToServer();
    socket.onMessage(message => {
      // SetRotation's postcard enum discriminant in protocol 72.
      if (typeof message !== 'string' && message[0] === 55) rotations.push(Buffer.from(message));
      server.send(message);
    });
    server.onMessage(message => socket.send(message));
  });
  await page.goto('/');
  await expect(page.locator('body')).toHaveClass(/webgpu/);
  await page.locator('[data-ose="player-name"]').fill('Capped render controls');
  const login = page.locator('[data-ose="login"]');
  if (await login.isVisible()) await login.click();
  await page.locator('[data-ose="play"]').click();
  await page.locator('#screen-sessions.open').waitFor();
  await page.locator('#create-details > summary').click();
  await page.locator('#create-name').fill('Render cap controls');
  await page.locator('#create-mode').selectOption('Sandbox');
  await page.locator('#create').click();
  await page.locator('[data-ose="ksc-vab"]').click();
  await page.locator('[data-ose="vab-clear"]').click();
  for (const part of ['Tin Can', 'FL-T200', 'LV-T45']) {
    await page.locator('[data-ose="vab-search"]').fill(part);
    await page.locator('[data-ose="vab-part"]').first().click();
  }
  await page.locator('[data-ose="vab-launch"]').click();
  await page.locator('[data-ose="flight"].open').waitFor();
  await page.waitForTimeout(1000);
  const before = rotations.length;
  try {
    await page.keyboard.down('w');
    await expect.poll(() => rotations.length).toBe(before + 1);
    await page.waitForTimeout(400);
    expect(rotations.length).toBe(before + 1);
  } finally {
    await page.keyboard.up('w');
  }
  await expect.poll(() => rotations.length).toBe(before + 2);
  expect(errors).toEqual([]);
});
