import { expect, test } from "@playwright/test";

// Delivery routes in the tracking station (docs/delivery-routes-design.md, step 3): a vessel in orbit is marked
// as a depot from its card and shows up in the Routes tab; a craft on the pad starts a proving flight, which
// the Routes tab lists and whose card stops it (dropped, since it never docked) and forgets it.
test("the Routes tab marks depots and records proving flights", async ({ page, baseURL }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(baseURL!);
  await page.locator('[data-ose="player-name"]').fill("route planner");
  await page.locator('[data-ose="login"]').click();
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().waitFor();
  if (!(await page.locator("#create-name").isVisible())) {
    await page.locator("#create-details > summary").click();
  }
  await page.selectOption("#create-mode", "Sandbox");
  await page.fill("#create-name", `routes ${Date.now()}`);
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-launchpad"]').waitFor();
  const hints = page.getByRole("button", { name: "No more hints", exact: true });
  if (await hints.isVisible()) await hints.click();

  async function launch(craft: string, button: string) {
    await page.locator('[data-ose="ksc-launchpad"]').click();
    await page.locator('[data-ose="craft"]').filter({ hasText: craft }).first().click();
    await page.locator(`[data-ose="${button}"]`).click();
    await expect(page.locator("#flight-hud")).toHaveClass(/open/);
    const id = (await page.locator("#flight-hud").getAttribute("data-ose-vessel"))!;
    await page.locator("#fh-ksc").click();
    await page.locator('[data-ose="ksc-tracking"]').waitFor();
    return id;
  }
  const station = await launch("Stock12_StartRocket", "lp-orbit");
  const pad = await launch("Kerbal K1", "lp-launch");

  await page.locator('[data-ose="ksc-tracking"]').click();
  await page.locator('[data-ose="ts-tab-routes"]').click();
  await expect(page.locator('[data-ose="route-none"]')).toBeVisible();

  // the station in orbit becomes a depot from its vessel card
  await page.locator('[data-ose="ts-tab-all"]').click();
  await page.locator(`li[data-ose="ts-vessel"][data-ose-id="${station}"]`).click();
  // in orbit, away from any depot, it cannot start a route
  await page.locator(`[data-ose-rec-start="${station}"]`).click();
  await expect(page.locator('[data-ose="toast"]').filter({ hasText: "next to the depot" })).toBeVisible();
  await page.locator(`[data-ose-depot="${station}"]`).click();
  await expect(page.locator(`[data-ose-depot="${station}"]`)).toHaveText("Unmark depot", { timeout: 10_000 });

  // the craft on the pad records a route
  await page.locator(`li[data-ose="ts-vessel"][data-ose-id="${pad}"]`).click();
  await page.locator(`[data-ose-rec-start="${pad}"]`).click();
  await expect(page.locator(`[data-ose-rec-start="${pad}"]`)).toHaveCount(0, { timeout: 10_000 });

  await page.locator('[data-ose="ts-tab-routes"]').click();
  await expect(page.locator('[data-ose="ts-tab-routes"]')).toHaveClass(/on/);
  await expect(page.locator("#ts-list .ts-vessel.depot")).toHaveCount(1);
  await expect(page.locator("#ts-list .ts-vessel.depot")).toHaveAttribute("data-ose-ts-focus", station);
  const proving = page.locator('[data-ose="ts-recording"]');
  await expect(proving).toHaveCount(1);
  await expect(proving).toContainText("recording");
  await proving.click();
  await expect(page.locator("#ts-info")).toContainText("Your recording");
  await page.locator('[data-ose="rec-stop"]').click();
  await expect(proving).toContainText("dropped", { timeout: 10_000 });
  await page.locator('[data-ose="rec-forget"]').click();
  await expect(proving).toHaveCount(0, { timeout: 10_000 });
  await expect(page.locator("#ts-info")).toContainText("Select a route or a proving flight");
  expect(errors).toEqual([]);
});
