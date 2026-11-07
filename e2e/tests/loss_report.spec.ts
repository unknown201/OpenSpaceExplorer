import { expect, test } from "@playwright/test";

// A crash leaves the camera on the crash site and, a moment later, opens the flight report with the original's
// destroyed-vessel buttons (FlightReportUIManager.OnActiveVesselDestroyed): Revert to Launch, Revert to VAB,
// Tracking Station and KSC. Revert to Launch puts the craft back on the pad in the flight view.
test("a lost vessel's report offers the reverts and Revert to Launch flies again", async ({ page, baseURL }) => {
  page.setDefaultTimeout(30_000);
  await page.goto(baseURL!);
  await page.locator('[data-ose="player-name"]').fill("crash pilot");
  await page.locator('[data-ose="play"]').click();
  // A Sandbox campaign of its own: every part, and nobody else online to block the reverts.
  await page.locator('[data-ose="session-join"]').first().waitFor();
  if (!(await page.locator("#create-name").isVisible())) {
    await page.locator("#create-details > summary").click();
  }
  await page.selectOption("#create-mode", "Sandbox");
  await page.fill("#create-name", "loss report");
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-vab"]').click();
  await page.locator('[data-ose="vab-clear"]').click();
  for (const part of ["Tin Can", "FL-T400", "LV-T45"]) {
    await page.locator('[data-ose="vab-search"]').fill(part);
    await page.locator('[data-ose="vab-part"]').first().dblclick();
  }
  await page.locator('[data-ose="vab-name"]').fill("crash hop");
  await page.locator('[data-ose="vab-launch"]').click();
  const flight = page.locator('[data-ose="flight"]');
  await expect(page.locator('[data-ose="flight"].open')).toBeVisible();
  const hints = page.getByRole("button", { name: "No more hints" });
  if (await hints.isVisible()) await hints.click();
  await page.waitForTimeout(3000);
  const start = Number(await flight.getAttribute("data-ose-altitude"));
  await page.locator('[data-ose="flight-throttle-full"]').click();
  await page.locator('[data-ose="flight-stage"]').click();
  await page.waitForFunction(
    alt => Number(document.querySelector('[data-ose="flight"]')!.getAttribute("data-ose-altitude")) > alt + 35,
    start,
    { timeout: 30_000 },
  );
  await page.locator('[data-ose="flight-throttle-cut"]').click();
  await expect(flight).toHaveAttribute("data-ose-situation", "Crashed", { timeout: 45_000 });
  // Still in flight on the wreck, and then the report.
  await expect(page.locator('[data-ose="flight"].open')).toBeVisible();
  const report = page.locator("#flight-report.open");
  await expect(report).toBeVisible({ timeout: 10_000 });
  await expect(page.locator('[data-ose="flight"].open')).toBeVisible();
  await expect(report.locator("button")).toHaveText([
    "Revert to Launch",
    "Revert to VAB",
    "Tracking Station",
    "KSC",
    "Continue",
  ]);
  await expect(report.locator("#fr-revert-launch")).toBeEnabled();
  await report.locator("#fr-revert-launch").click();
  await expect(page.locator("#flight-report")).not.toHaveClass(/open/);
  await expect(flight).not.toHaveAttribute("data-ose-situation", "Crashed");
  await expect(page.locator('[data-ose="flight"].open')).toBeVisible();
  // Leave the pad clear.
  await page.keyboard.press("Escape");
  await page.locator('[data-ose="to-recover"]').click();
  await expect(page.locator('[data-ose="flight"].open')).toBeHidden();
});
