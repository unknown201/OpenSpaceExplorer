import { expect, test, type Page } from "@playwright/test";

// Locks the flight keys to KSP2's directions (Josh has seen them come back reversed more than once):
// W pitches the nose down (toward the craft's belly), S up, A yaws left, D right, Q rolls left, E right;
// and a rocket stands on its pad as KSP2's does, so D starts the gravity turn to the east.

type Axes = { f: number[]; t: number[]; r: number[] };
const dot = (a: number[], b: number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const sub = (a: number[], b: number[]) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];

async function axes(page: Page): Promise<Axes> {
  return page.evaluate(() => {
    const c = document.getElementById("fh-navball")!;
    const v = (a: string) => (c.getAttribute(a) ?? "0,0,0").split(",").map(Number);
    return { f: v("data-ose-forward"), t: v("data-ose-top"), r: v("data-ose-right") };
  });
}

/** How far a key moves the nose toward the craft's top and right, and its top toward its right, over and above
 *  the drift without it. */
async function effect(page: Page, key: string): Promise<[number, number, number]> {
  const span = async (k: string | null) => {
    const a = await axes(page);
    if (k) await page.keyboard.down(k);
    await page.waitForTimeout(700);
    if (k) await page.keyboard.up(k);
    const b = await axes(page);
    const df = sub(b.f, a.f);
    const dt = sub(b.t, a.t);
    return [dot(df, a.t), dot(df, a.r), dot(dt, a.r)];
  };
  await page.waitForTimeout(1500);
  const base = await span(null);
  const k = await span(key);
  return [k[0] - base[0], k[1] - base[1], k[2] - base[2]];
}

async function start(page: Page, baseURL: string, name: string) {
  page.setDefaultTimeout(15_000);
  // Instruments without the locally exported game artwork.
  await page.route("**/assets/ui/Flight-Navball-Texture.png", route => route.fulfill({
    contentType: "image/svg+xml",
    body: '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024"><rect width="1024" height="512" fill="#267cbd"/><rect y="512" width="1024" height="512" fill="#9d663b"/></svg>',
  }));
  await page.goto(baseURL);
  await page.locator('[data-ose="player-name"]').fill(name);
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().click();
  await page.locator('[data-ose="ksc-launchpad"]').click();
}

test("each flight key turns the craft the original's way", async ({ page, baseURL }) => {
  await start(page, baseURL!, "direction pilot");
  await page.locator('[data-ose="craft"]').filter({ hasText: "Stock12_StartRocket" }).click();
  await page.locator('[data-ose="lp-orbit"]').click();
  await expect(page.locator("#status")).toContainText("launched Stock12_StartRocket");
  await page.locator('[data-ose="ksc-button"]').click();
  const hints = page.getByRole("button", { name: "No more hints" });
  if (await hints.isVisible()) await hints.click();
  await page.locator('[data-ose="ksc-tracking"]').click();
  await page.locator('[data-ose="ts-vessel"].mine').filter({ hasText: "Stock12_StartRocket" }).last().click();
  await page.locator('[data-ose="ts-fly"]').click();
  await expect(page.locator('[data-ose="flight-navball"]')).toHaveAttribute("data-ose-navball", "drawn");
  // [nose toward top, nose toward right, top toward right]
  const [wTop] = await effect(page, "w");
  expect(wTop, "W pitches the nose down").toBeLessThan(-0.01);
  const [sTop] = await effect(page, "s");
  expect(sTop, "S pitches the nose up").toBeGreaterThan(0.01);
  const [, aRight] = await effect(page, "a");
  expect(aRight, "A yaws left").toBeLessThan(-0.01);
  const [, dRight] = await effect(page, "d");
  expect(dRight, "D yaws right").toBeGreaterThan(0.01);
  const [, , qRoll] = await effect(page, "q");
  expect(qRoll, "Q rolls left").toBeLessThan(-0.01);
  const [, , eRoll] = await effect(page, "e");
  expect(eRoll, "E rolls right").toBeGreaterThan(0.01);
});

test("a rocket leaving the pad turns east with D", async ({ page, baseURL }) => {
  page.setDefaultTimeout(15_000);
  await page.route("**/assets/ui/Flight-Navball-Texture.png", route => route.fulfill({
    contentType: "image/svg+xml",
    body: '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024"><rect width="1024" height="512" fill="#267cbd"/><rect y="512" width="1024" height="512" fill="#9d663b"/></svg>',
  }));
  // A campaign of its own: the shared one's pads hold other tests' vessels, and with collisions (dd6b22f) a
  // rocket launched beside one is knocked about.
  await page.goto(baseURL!);
  await page.locator('[data-ose="player-name"]').fill("gravity turn pilot");
  await page.locator('[data-ose="play"]').click();
  await page.locator('[data-ose="session-join"]').first().waitFor();
  if (!(await page.locator("#create-name").isVisible())) {
    await page.locator("#create-details > summary").click();
  }
  await page.selectOption("#create-mode", "Sandbox");
  await page.fill("#create-name", `gravity turn ${test.info().project.name}`);
  await page.locator('[data-ose="create"]').click();
  await page.locator('[data-ose="ksc-launchpad"]').click();
  await page.locator('[data-ose="craft"]').filter({ hasText: "Stock12_StartRocket" })
    .locator('[data-ose="craft-launch"]').click();
  await expect(page.locator('[data-ose="flight"].open')).toBeVisible();
  const hints = page.getByRole("button", { name: "No more hints" });
  if (await hints.isVisible()) await hints.click();
  await page.keyboard.press("z");
  await page.keyboard.press("Space");
  await page.waitForTimeout(3000);
  // Read while D is held, once the nose has left the vertical: held longer, a small rocket's tip-over
  // swings the heading round as it passes level.
  await page.keyboard.down("d");
  await page.waitForTimeout(1200);
  const heading = parseFloat((await page.locator("#fh-heading").textContent()) ?? "");
  await page.keyboard.up("d");
  expect(heading, `D tips a rising rocket east (heading ${heading})`).toBeGreaterThan(45);
  expect(heading).toBeLessThan(135);
});
