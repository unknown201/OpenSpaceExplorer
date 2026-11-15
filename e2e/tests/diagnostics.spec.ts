import { expect, test } from '@playwright/test';

test('manual and automatic stall reports reach the authenticated server', async ({ page }) => {
  await page.goto('/');
  await page.locator('[data-ose="player-name"]').fill('Diagnostics pilot');
  await page.locator('[data-ose="login"]').click();
  await page.locator('[data-ose="play"]').click();
  await page.locator('#screen-sessions.open').waitFor();
  await page.locator('#create-details > summary').click();
  await page.locator('#create-name').fill('Diagnostics check');
  await page.locator('#create-mode').selectOption('Sandbox');
  await page.locator('#create').click();
  await page.locator('#screen-ksc.open').waitFor();
  await page.keyboard.press('Escape');
  await page.locator('#to-settings').click();
  await page.locator('#settings-tabs [data-tab="graphics"]').click();
  await expect(page.locator('#performance-report-status')).not.toHaveText('Sending report…');
  const manualResponse = page.waitForResponse(response => response.url().endsWith('/api/diagnostics')
    && response.request().postDataJSON()?.reason === 'manual');
  await page.locator('[data-ose="report-performance"]').click();
  const manual = await manualResponse;
  expect(manual.status()).toBe(200);
  const ack = await manual.json();
  await expect(page.locator('#performance-report-status')).toContainText(ack.report_id);
  const report = manual.request().postDataJSON();
  expect(report.build).toMatch(/^[a-f0-9]{40}$/);
  expect(report.metrics.frames).toBeGreaterThan(0);
  const token = await page.evaluate(() => localStorage.getItem('ose.token'));
  expect(manual.request().postData()).not.toContain(token!);
  expect(report).not.toHaveProperty('email');
  expect(report).not.toHaveProperty('player_name');
  // Exercise the actual RAF-to-HTTP path after the intentional stall cooldown.
  await page.waitForTimeout(10_100);
  const stallResponse = page.waitForResponse(response => response.url().endsWith('/api/diagnostics')
    && response.request().postDataJSON()?.reason === 'stall'
    && response.request().postDataJSON()?.metrics.frame_gap_max_ms >= 300);
  await page.evaluate(() => {
    const until = performance.now() + 330;
    while (performance.now() < until) { /* deliberate main-thread stall */ }
  });
  const stall = await stallResponse;
  expect(stall.status()).toBe(200);
  expect(stall.request().postDataJSON().session).toBe(report.session);
  expect(stall.request().postDataJSON().sequence).toBeGreaterThan(report.sequence);
  await page.locator('#settings-close').click();
  await page.keyboard.press('Escape');
  await page.locator('[data-ose="ksc-sessions"]').click();
  const session = page.locator('[data-ose="session"]').filter({ hasText: 'Diagnostics check' });
  page.once('dialog', dialog => dialog.accept());
  await session.locator('[data-ose="session-delete"]').click();
  await expect(session).toHaveCount(0);
});
