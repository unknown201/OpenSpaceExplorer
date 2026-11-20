import { expect, test } from '@playwright/test';

test('planning modifier arrows preserve the burn, warp can be stopped, and the orbit click offers a node or warp', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => localStorage.setItem('ose.ftue', 'off'));
  await page.goto('/');
  await page.locator('[data-ose="player-name"]').fill('Waypoint controls');
  await page.locator('[data-ose="login"]').click();
  await page.locator('[data-ose="play"]').click();
  await page.locator('#create-details > summary').click();
  const campaignName = `Waypoint controls ${Date.now()}`;
  await page.locator('#create-name').fill(campaignName);
  await page.locator('#create-mode').selectOption('Sandbox');
  await page.locator('#create').click();
  await page.locator('[data-ose="ksc-launchpad"]').click();
  await page.locator('[data-ose="craft"]').filter({ hasText: 'Stock12_StartRocket' }).click();
  await page.locator('[data-ose="launch-altitude"]').fill('300');
  await page.locator('[data-ose="lp-orbit"]').click();
  await expect(page.locator('#flight-hud.open')).toBeVisible();
  await expect.poll(() => page.locator('#fh-throttle-fill').evaluate(e => (e as HTMLElement).style.height)).toBe('0%');
  await page.keyboard.press('m');
  await page.locator('[data-ose="add-node"]').click();
  const node = page.locator('[data-ose="maneuver-node"]');
  const range = page.locator('#node-prograde');
  await range.evaluate(el => {
    (el as HTMLInputElement).value = '40.7';
    el.dispatchEvent(new Event('input', { bubbles: true }));
  });
  const time = page.locator('#node-dt');
  const originalUT = Number(await node.getAttribute('data-ose-ut'));
  await time.focus();
  await page.keyboard.press('Shift+ArrowRight');
  expect(Number(await node.getAttribute('data-ose-ut')) - originalUT).toBeCloseTo(10, 1);
  await page.keyboard.press('Control+ArrowLeft');
  expect(Number(await node.getAttribute('data-ose-ut')) - originalUT).toBeCloseTo(9.9, 1);
  await expect(node).toHaveAttribute('data-ose-prograde', '40.7');
  await range.focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Shift+ArrowRight');
  await page.keyboard.press('Control+ArrowLeft');
  await expect(range).toHaveValue('51.6');
  // A held modifier on the map must not leak into the flight throttle.
  await page.locator('[data-ose="node-warp"]').focus();
  await page.keyboard.down('Shift');
  await page.waitForTimeout(400);
  await page.keyboard.up('Shift');
  await expect.poll(() => page.locator('#fh-throttle-fill').evaluate(e => (e as HTMLElement).style.height)).toBe('0%');
  await page.locator('[data-ose="flight-throttle-full"]').click();
  await expect.poll(() => page.locator('#fh-throttle-fill').evaluate(e => (e as HTMLElement).style.height)).toBe('100%');
  await page.locator('[data-ose="node-warp"]').click();
  await expect(page.locator('#node-warp')).toHaveText('Stop warp');
  await expect.poll(() => page.locator('#fh-throttle-fill').evaluate(e => (e as HTMLElement).style.height)).toBe('0%');
  await time.focus();
  await page.keyboard.press('/');
  await expect(page.locator('#node-warp')).toHaveText('Warp to');
  await expect(page.locator('#warp')).toHaveText('×1');
  await page.locator('[data-ose="node-warp"]').click();
  await page.locator('[data-ose="warp-stop"]').click();
  await expect(page.locator('#warp')).toHaveText('×1');
  await expect(page.locator('#node-warp')).toHaveText('Warp to');
  // A click on the orbit offers the original's popup there: time to the point, Add maneuver, Warp to.
  await page.locator('[data-ose="clear-node"]').click();
  await expect(node).toHaveCount(0);
  // the point of the orbit farthest from the vessel that nothing covers (its marker and label, the HUD and its
  // navball and speed gauge canvases), read
  // afresh each time since the view follows the vessel
  const canvas = page.locator('#gpu');
  const orbitPoint = async () => {
    await expect(canvas).toHaveAttribute('data-ose-orbit-points', /\d+ \d+;/);
    const [vesX, vesY] = (await canvas.getAttribute('data-ose-orbit-vessel'))!.split(' ').map(Number);
    // the 2D fallback map draws on #map (the WebGPU canvas is hidden then)
    const rect = (await canvas.boundingBox()) ?? (await page.locator('#map').boundingBox())!;
    const vessel = { x: rect.x + vesX, y: rect.y + vesY };
    const points = (await canvas.getAttribute('data-ose-orbit-points'))!
      .split(';')
      .map(p => p.split(' ').map(Number))
      .map(([x, y]) => ({ x: rect.x + x, y: rect.y + y }));
    const clear = await page.evaluate(ps => ps.map(p => ['gpu', 'map'].includes(document.elementFromPoint(p.x, p.y)?.id ?? '')), points);
    const dist = (p: { x: number; y: number }) => Math.hypot(p.x - vessel.x, p.y - vessel.y);
    const at = points.filter((_, i) => clear[i]).sort((a, b) => dist(b) - dist(a))[0];
    expect(at).toBeDefined();
    return { at, vessel };
  };
  let { at, vessel } = await orbitPoint();
  const popup = page.locator('[data-ose="orbit-popup"]');
  await page.mouse.click(at.x, at.y);
  await expect(popup).toBeVisible();
  // well round the 47:35 orbit from the vessel
  await expect(page.locator('[data-ose="orbit-popup-time"]')).toHaveText(/^T-[1-3]\d:\d\d$/);
  const pointUT = Number(await popup.getAttribute('data-ose-ut'));
  await page.locator('[data-ose="orbit-add-node"]').click();
  await expect(popup).toBeHidden();
  await expect(node).toHaveCount(1);
  expect(Number(await node.getAttribute('data-ose-ut'))).toBeCloseTo(pointUT, 0);
  await page.locator('[data-ose="clear-node"]').click();
  // ...and Warp to coasts there; a click off the orbit closes the popup.
  ({ at, vessel } = await orbitPoint());
  await page.mouse.click(at.x, at.y);
  await expect(popup).toBeVisible();
  // a point part of the way across from it toward the vessel: off the orbit, and not under the popup or a label
  const offs = [0.25, 0.35, 0.5, 0.65].map(f => ({ x: at.x + (vessel.x - at.x) * f, y: at.y + (vessel.y - at.y) * f }));
  const free = await page.evaluate(ps => ps.map(p => ['gpu', 'map'].includes(document.elementFromPoint(p.x, p.y)?.id ?? '')), offs);
  const off = offs.find((_, i) => free[i])!;
  expect(off).toBeDefined();
  await page.mouse.click(off.x, off.y);
  await expect(popup).toBeHidden();
  ({ at, vessel } = await orbitPoint());
  await page.mouse.click(at.x, at.y);
  await page.locator('[data-ose="orbit-warp"]').click();
  await expect(page.locator('#warp')).not.toHaveText('×1');
  await page.locator('[data-ose="warp-stop"]').click();
  await expect(page.locator('#warp')).toHaveText('×1');
  await page.locator('#fh-ksc').click();
  await page.locator('[data-ose="ksc-sessions"]').click();
  const campaign = page.locator('[data-ose="session"]').filter({ hasText: campaignName });
  await expect(campaign).toHaveCount(1);
  page.on('dialog', dialog => dialog.accept());
  // A campaign update queued before LeaveSession can reopen KSC after the first exit.
  // Retry only the cleanup handshake; all waypoint assertions above have already passed.
  await expect(async () => {
    if (await campaign.count() === 0) return;
    const exit = page.locator('[data-ose="ksc-sessions"]');
    if (await exit.isVisible()) await exit.click();
    await campaign.locator('[data-ose="session-delete"]').click({ timeout: 1500 });
    await expect(campaign).toHaveCount(0, { timeout: 3000 });
  }).toPass({ timeout: 20_000 });
  expect(errors).toEqual([]);
});
