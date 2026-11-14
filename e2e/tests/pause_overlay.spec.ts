import { expect, test } from "@playwright/test";

// The Game Menu (Esc) is an overlay on the flight, not a trip anywhere: no loading screen opening or closing it,
// and a revert from it closes it and starts the flight view over.
test("the game menu overlays the flight and a revert closes it", async ({ page, baseURL }) => {
  page.setDefaultTimeout(20_000);
  await page.goto(baseURL!);
  await page.locator('[data-ose="player-name"]').fill("overlay pilot");
  await page.locator('[data-ose="login"]').click();
  await page.locator('[data-ose="play"]').click();
  // The live campaign, as the tests after this one join it: creating one here would change theirs.
  await page.locator('[data-ose="session-join"]').first().click();
  await page.locator('[data-ose="ksc-vab"]').click();
  await page.locator('[data-ose="vab-clear"]').click();
  for (const part of ["Tin Can", "Mk16-XL"]) {
    await page.locator('[data-ose="vab-search"]').fill(part);
    await page.locator('[data-ose="vab-part"]').first().dblclick();
  }
  await page.locator('[data-ose="vab-name"]').fill("overlay pod");
  await page.locator('[data-ose="vab-launch"]').click();
  await expect(page.locator('[data-ose="flight"].open')).toBeVisible();
  const hints = page.getByRole("button", { name: "No more hints" });
  if (await hints.isVisible()) await hints.click();
  const loading = page.locator("#loading");
  await expect(loading).not.toHaveClass(/show/, { timeout: 10_000 });
  const neverLoads = async () => {
    for (let i = 0; i < 8; i++) {
      expect(await loading.evaluate(e => e.classList.contains("show")), "no loading screen").toBe(false);
      await page.waitForTimeout(100);
    }
  };
  await page.keyboard.press("Escape");
  await expect(page.locator("#screen-paused")).toHaveClass(/open/);
  await neverLoads();
  await page.keyboard.press("Escape");
  await expect(page.locator("#menu")).toHaveClass(/hidden/);
  await neverLoads();
  // From the map: the revert closes the menu and the map.
  await page.keyboard.press("m");
  await expect(page.locator("#flight-hud")).toHaveClass(/map/);
  await page.keyboard.press("Escape");
  await page.locator('#screen-paused [data-save-action="RevertToLaunch"]').click();
  await expect(page.locator("#menu")).toHaveClass(/hidden/);
  await expect(page.locator("#flight-hud")).not.toHaveClass(/map/);
  // Leave the pad clear for the tests that join this campaign after us.
  await page.keyboard.press("Escape");
  await page.locator('[data-ose="to-recover"]').click();
  await expect(page.locator('[data-ose="flight"].open')).toBeHidden();
});
