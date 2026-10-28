import { expect, test } from "@playwright/test";

test("warp requests pass 4x and VAB icons accept clicks", async ({ page, baseURL }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(baseURL!);
  await page.locator('[data-ose="player-name"]').fill("session regression");
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().click();
  await expect(page.locator('[data-ose="ksc-vab"]').first()).toBeVisible();
  const warp = page.locator('[data-ose="warp"]');
  // Wait for each server echo before advancing; an effective cap must not reset the ladder.
  for (const factor of [2, 3, 4, 5, 10]) {
    await warp.click();
    await expect(warp).toHaveAttribute("data-ose-requested-warp", String(factor));
    await page.waitForTimeout(150);
  }
  await page.keyboard.press("/");
  await page.locator('[data-ose="ksc-vab"]').first().click();
  await page.locator('[data-ose="vab-clear"]').click();
  await page.locator('[data-ose="vab-search"]').fill("Tin Can");
  await page.locator('[data-ose="vab-part"]').first().click();
  await expect(page.locator('[data-ose="vab-canvas"]')).toHaveAttribute("data-ose-vab-parts", "1");
  // Real pointer clicks at the icon centre must activate the containing button.
  const clickIcon = async (selector: string) => {
    const icon = page.locator(selector);
    const rect = await icon.boundingBox();
    expect(rect).not.toBeNull();
    await page.mouse.click(rect!.x + rect!.width / 2, rect!.y + rect!.height / 2);
  };
  await expect(page.locator("#vab-undo")).toBeEnabled();
  await clickIcon("#vab-undo i");
  await expect(page.locator("#vab-stats b").first()).toHaveText("0");
  await clickIcon("#vab-redo i");
  await expect(page.locator("#vab-stats b").first()).toHaveText("1");
  await clickIcon("#vab-report-btn i");
  await expect(page.locator("#vab-report-pop")).toHaveClass(/open/);
  await clickIcon("#vab-back i");
  await expect(page.locator('[data-ose="ksc-vab"]').first()).toBeVisible();
  await page.locator('[data-ose="ksc-sessions"]').first().click();
  await expect(page.locator('[data-ose="session-list"]')).toBeVisible();
  // Own and delete a dedicated campaign so later browser projects keep their shared world.
  if (!(await page.locator("#create-name").isVisible())) await page.locator("#create-details > summary").click();
  const cleanupName = `Session cleanup ${Date.now()}`;
  await page.selectOption("#create-mode", "Sandbox");
  await page.fill("#create-name", cleanupName);
  await page.locator("#create").click();
  await expect(page.locator('[data-ose="ksc-sessions"]').first()).toBeVisible();
  await page.locator('[data-ose="ksc-sessions"]').first().click();
  const session = page.locator('[data-ose="session"]').filter({ hasText: cleanupName });
  await expect(session).toContainText("Owned by you");
  const id = await session.getAttribute("data-ose-id");
  page.once("dialog", dialog => dialog.accept());
  await session.locator('[data-ose="session-delete"]').click();
  await expect(page.locator(`[data-ose="session"][data-ose-id="${id}"]`)).toHaveCount(0);
  expect(errors).toEqual([]);
});
