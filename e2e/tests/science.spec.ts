import { expect, test } from "@playwright/test";

test("science buttons glow for available experiments and stop after diminishing returns exhaust", async ({ page, baseURL }, testInfo) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(baseURL!);
  await page.locator('[data-ose="player-name"]').fill("science pilot");
  const login = page.locator('[data-ose="login"]');
  if (await login.isVisible()) await login.click();
  await page.locator('[data-ose="play"]').click();
  await page.locator("#create-details > summary").click();
  await page.locator("#create-mode").selectOption("Sandbox");
  await page.locator("#create-name").fill("Science availability regression");
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-launchpad"]').click();
  await page.locator('[data-ose="craft"]').filter({ hasText: "Kerbal K1" }).click();
  await page.locator('[data-ose="lp-launch"]').click();
  const hints = page.getByRole("button", { name: "No more hints" });
  if (await hints.isVisible()) await hints.click();
  const science = page.locator("#fh-act-science");
  const textButton = page.locator("#fh-science");
  await expect(science).toHaveClass(/science-ready/);
  await expect(textButton).toHaveClass(/science-ready/);
  expect(await science.evaluate(el => getComputedStyle(el).boxShadow)).not.toBe("none");
  await expect(page.locator("#loading")).not.toHaveClass(/show/, { timeout: 10_000 });
  await expect(page.locator("#loading")).toHaveCSS("opacity", "0");
  await testInfo.attach("science-ready", {
    body: await page.screenshot({ path: testInfo.outputPath("science-ready.png") }),
    contentType: "image/png",
  });

  let previous = Infinity;
  let decreased = false;
  let runs = 0;
  do {
    await expect(science).toBeEnabled();
    await science.click();
    await expect(science).toBeDisabled();
    await expect(science).not.toHaveClass(/science-ready/);
    await expect(textButton).not.toHaveClass(/science-ready/);
    await page.locator("#fh-transmit").click({ force: true });
    const note = page.locator("#fh-note");
    await expect(note).toContainText(/transmitted [1-9]\d* .* for [1-9]\d* science/, { timeout: 30_000 });
    const earned = Number((await note.textContent())!.match(/for (\d+) science/)![1]);
    expect(earned).toBeGreaterThan(0);
    expect(earned).toBeLessThanOrEqual(previous);
    if (Number.isFinite(previous) && earned < previous) decreased = true;
    previous = earned;
    runs++;
    expect(runs).toBeLessThan(20);
    // Allow the next authoritative snapshot to update the buttons after transmission.
    await page.waitForTimeout(300);
  } while (await science.isEnabled());
  expect(runs).toBeGreaterThan(1);
  expect(decreased).toBe(true);
  await expect(science).toBeDisabled();
  await expect(textButton).toBeDisabled();
  expect(await science.evaluate(el => getComputedStyle(el).boxShadow)).toBe("none");
  expect(errors).toEqual([]);
  await page.keyboard.press('Escape');
  await page.locator('[data-ose="to-recover"]').click();
  await page.locator('#fr-close').click();
  await page.locator('[data-ose="ksc-sessions"]').click();
  const session = page.locator('[data-ose="session"]').filter({ hasText: 'Science availability regression' });
  page.once('dialog', dialog => dialog.accept());
  await session.locator('[data-ose="session-delete"]').click();
  await expect(session).toHaveCount(0);
});
