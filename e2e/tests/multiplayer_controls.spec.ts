import { expect, test } from "@playwright/test";
import { performance } from "node:perf_hooks";

test("held flight controls with 30 ms added round-trip delay", async ({ page, baseURL }, testInfo) => {
  const controls: { at: number; bytes: number }[] = [];
  const timers = new Set<ReturnType<typeof setTimeout>>();
  let active = true;
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  // Delay each direction without changing ordering. This models latency, not packet loss or bandwidth.
  // The delay starts with the measured flight: Firefox's routed WebSocket sometimes holds the join reply
  // when every frame is deferred from the first one.
  let delaying = false;
  await page.routeWebSocket("**/ws", socket => {
    const server = socket.connectToServer();
    const later = (send: () => void) => {
      if (!active) return;
      if (!delaying) { send(); return; }
      const timer = setTimeout(() => { timers.delete(timer); if (active) send(); }, 15);
      timers.add(timer);
    };
    socket.onMessage(message => {
      // Every binary client message: held steering keys go as a held rotation input (SetRotation, protocol
      // 39 on), sent when it changes. Not tied to discriminants, which move whenever the protocol grows.
      if (typeof message !== "string") {
        controls.push({ at: performance.now(), bytes: message.length });
      }
      later(() => server.send(message));
    });
    server.onMessage(message => later(() => socket.send(message)));
  });

  try {
    await page.goto(baseURL!);
    await page.locator('[data-ose="player-name"]').fill("latency pilot");
    await page.locator('[data-ose="play"]').click();
    // A campaign of its own, as controls_direction does: the shared one's pads fill with other tests'
    // vessels, and then Launch has nowhere to roll out to.
    await page.locator('[data-ose="session-join"]').first().waitFor();
    if (!(await page.locator("#create-name").isVisible())) {
      await page.locator("#create-details > summary").click();
    }
    await page.selectOption("#create-mode", "Sandbox");
    await page.fill("#create-name", `latency controls ${testInfo.project.name}`);
    await page.locator('[data-ose="create"]').click();
    await page.locator('[data-ose="ksc-vab"]').first().click();
    await page.locator('[data-ose="vab-clear"]').click();
    for (const part of ["Tin Can", "FL-T200", "LV-T45"]) {
      await page.locator('[data-ose="vab-search"]').fill(part);
      // The card list re-renders after the search; wait for it as the gameplay chain does.
      const card = page.locator('[data-ose="vab-part"]').first();
      await card.waitFor({ timeout: 5000 });
      await card.dblclick();
    }
    await page.locator('[data-ose="vab-search"]').fill("");
    await page.locator('[data-ose="vab-name"]').fill("Latency test rocket");
    // An earlier test can leave its craft on the pad the VAB has selected; launch from a free one.
    const site = page.locator('[data-ose="vab-site"]');
    if (await site.isVisible()) {
      const free = await site.locator("option:not([disabled])").first().getAttribute("value");
      if (free) await site.selectOption(free);
    }
    await page.locator('[data-ose="vab-launch"]').click();
    // Rolling out opens the flight view over the KSC scene, which takes a while on a CI runner's software GPU.
    await expect(page.locator('[data-ose="flight"].open')).toBeVisible({ timeout: 20_000 });
    await page.keyboard.press("z");
    await page.keyboard.press("Space");
    await expect(page.locator('[data-ose="flight"]')).toHaveAttribute("data-ose-situation", "Flying", { timeout: 20_000 });
    delaying = true;
    const start = performance.now();
    const frameTimes = page.evaluate(() => new Promise<number[]>(resolve => {
      const times: number[] = [];
      const begin = performance.now();
      const frame = (now: number) => {
        times.push(now);
        if (now - begin < 2000) requestAnimationFrame(frame);
        else resolve(times);
      };
      requestAnimationFrame(frame);
    }));
    await page.keyboard.down("w");
    const frames = await frameTimes;
    await page.keyboard.up("w");
    await page.waitForTimeout(150);
    const samples = controls.filter(sample => sample.at >= start);
    const fps = (frames.length - 1) * 1000 / (frames.at(-1)! - frames[0]);
    // Holding W sends the rotation input once (the server holds it until it changes), and letting go
    // sends it again; no steady stream of attitude targets is needed any more.
    expect(samples.length).toBeGreaterThan(0);
    expect(errors).toEqual([]);
    const report = { addedRttMs: 30, browser: testInfo.project.name, fps,
      controlMessages: samples.length, controlPayloadBytes: samples.reduce((n, s) => n + s.bytes, 0),
      webgpu: await page.locator("body").evaluate(body => body.classList.contains("webgpu")) };
    console.log("multiplayer controls", JSON.stringify(report));
    await testInfo.attach("multiplayer-controls.json", { body: JSON.stringify(report, null, 2), contentType: "application/json" });
    await testInfo.attach("flight.png", { body: await page.screenshot({ path: testInfo.outputPath("flight.png") }), contentType: "image/png" });
  } finally {
    active = false;
    for (const timer of timers) clearTimeout(timer);
  }
});
