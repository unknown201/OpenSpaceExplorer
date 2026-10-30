import { expect, test } from "@playwright/test";

// Promoted from the client worktree's ladder capture probe. Screenshots belong to
// the test run, so this also works outside the original developer's machine.
test("ground EVA and boarding render without animation errors", async ({ page, baseURL }, testInfo) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(baseURL!);
  await page.locator('[data-ose="player-name"]').fill("ladder pilot");
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().waitFor();
  if (!(await page.locator("#create-name").isVisible())) {
    await page.locator("#create-details > summary").click();
  }
  await page.selectOption("#create-mode", "Sandbox");
  await page.fill("#create-name", "ladder controls");
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-vab"]').click();
  await page.locator('[data-ose="vab-clear"]').click();
  for (const part of ["Tin Can", "Mk16-XL"]) {
    await page.locator('[data-ose="vab-search"]').fill(part);
    await page.locator('[data-ose="vab-part"]').first().dblclick();
  }
  await page.locator('[data-ose="vab-name"]').fill("ladder pod");
  await page.locator('[data-ose="vab-launch"]').click();
  await expect(page.locator('[data-ose="flight"].open')).toBeVisible();
  const hints = page.getByRole("button", { name: "No more hints" });
  if (await hints.isVisible()) await hints.click();
  await page.locator('[data-ose="flight-eva"]').click();
  await expect(page.locator("#fh-board")).toBeVisible();
  // Artwork is optional in CI. When installed, check that the exported model
  // retains the normal idle as well as all three ladder clips.
  const model = await page.request.get(`${baseURL}/assets/kerbal/kerbal_m.glb`);
  const glb = model.ok() ? await model.body() : Buffer.alloc(0);
  // Without exported artwork the server answers with the app page, not a binary glTF ("glTF" magic).
  if (glb.length > 20 && glb.subarray(0, 4).toString() === "glTF") {
    const size = glb.readUInt32LE(12);
    const names = JSON.parse(glb.subarray(20, 20 + size).toString()).animations.map((a: { name: string }) => a.name);
    expect(names).toEqual(expect.arrayContaining(["idle", "ladder_up", "ladder_down", "ladder_idle"]));
  }
  await testInfo.attach("ground-eva", {
    body: await page.screenshot({ path: testInfo.outputPath("ground-eva.png") }),
    contentType: "image/png",
  });
  await page.locator('[data-ose="flight-board"]').click();
  await expect(page.locator('[data-ose="flight-eva"]')).toBeVisible();
  await expect(page.locator('[data-ose="flight-board"]')).toBeHidden();
  expect(errors).toEqual([]);
});
