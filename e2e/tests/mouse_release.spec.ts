import { expect, test } from "@playwright/test";

// A camera drag let go outside the window (or across Alt+Tab) never gets its mouseup: the camera kept
// following the bare mouse until the next click. The next move with no button held, or the window losing
// focus, ends the drag.
test("a camera drag released outside the window lets go of the mouse", async ({ page, baseURL }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(baseURL!);
  await page.locator('[data-ose="player-name"]').fill("mouse pilot");
  await page.locator('[data-ose="login"]').click();
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().waitFor();
  if (!(await page.locator("#create-name").isVisible())) {
    await page.locator("#create-details > summary").click();
  }
  await page.selectOption("#create-mode", "Sandbox");
  await page.fill("#create-name", "mouse release");
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-tracking"]').click();
  await expect(page.locator('[data-ose="menu"]').first()).toBeHidden();
  const canvas = page.locator("canvas:visible").first();
  const box = (await canvas.boundingBox())!;
  const [x, y] = [box.x + box.width / 2, box.y + box.height / 2];
  const dragging = () => canvas.evaluate(c => c.classList.contains("dragging"));

  // Pressed and moved with the button held: a drag.
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 20, y + 5);
  expect(await dragging()).toBe(true);
  // The button comes up outside the window: no mouseup, then a move with nothing held.
  await page.evaluate(([cx, cy]) => {
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: cx, clientY: cy, buttons: 0, bubbles: true }));
  }, [x + 40, y + 10]);
  expect(await dragging()).toBe(false);
  await page.mouse.up();

  // Pressed, then the window loses focus (Alt+Tab) before the release.
  await page.mouse.down();
  await page.mouse.move(x + 10, y);
  expect(await dragging()).toBe(true);
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  expect(await dragging()).toBe(false);
  await page.mouse.up();
  expect(errors).toEqual([]);
});
