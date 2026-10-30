import { expect, test } from "@playwright/test";

// Netcode smoothness benchmark (docs/multiplayer-performance.md, "Smoothness under latency and jitter").
//
// Launches a starter rocket through the real client with each WebSocket direction delayed by a base latency
// plus random jitter (order preserving: a frame never overtakes the one before it), flies it for a while, and
// collects the client's own once-a-second smoothness summary (`<body data-ose-net>`, crates/ose-client/src/
// netstats.rs). The report is printed and attached; nothing is asserted about the numbers, so the same test
// measures a build before and after a change. Knobs:
//   OSE_NET_DELAY_MS=60    one-way base delay added to each direction
//   OSE_NET_JITTER_MS=25   each message is additionally delayed by a uniform random 0..2*jitter ms
//   OSE_NET_SECONDS=10     how long to sample the flight
//   OSE_NET_LABEL=after    a tag copied into the report
//   OSE_NET_MODE=legacy    open the page with `?netsmooth=0`: the client draws as before protocol 58 (clock
//                          snaps to every TimeState, orbit-element extrapolation, no error blending)
const delayMs = Number(process.env.OSE_NET_DELAY_MS ?? 60);
const jitterMs = Number(process.env.OSE_NET_JITTER_MS ?? 25);
const seconds = Number(process.env.OSE_NET_SECONDS ?? 10);
const label = process.env.OSE_NET_LABEL ?? "";
const legacy = process.env.OSE_NET_MODE === "legacy";

type Q = { n: number; p50: number; p95: number; max: number };
type Window = {
  seconds: number; streamKbps: number; streamMessages: number; arrivalGapMs: Q; clockStepMs: Q;
  clockPendingMs: Q; utRate: number; residualM: Q; jumps: number; frameGapMs: Q; stalls: number; handlerMs: Q;
};

// Deterministic jitter so two runs see the same delays (mulberry32).
function rng(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

test(`flight smoothness with ${delayMs} ms one-way delay and ${jitterMs} ms jitter`, async ({ page, baseURL }, testInfo) => {
  test.setTimeout(180_000);
  const timers = new Set<ReturnType<typeof setTimeout>>();
  let active = true;
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  const random = rng(20261029);
  // One queue per direction: a message leaves no earlier than the one before it, so delays vary but
  // ordering holds, as over a TCP WebSocket.
  const direction = () => {
    let due = 0;
    return (send: () => void) => {
      if (!active) return;
      const now = performance.now();
      due = Math.max(due, now + delayMs + random() * 2 * jitterMs);
      const timer = setTimeout(() => { timers.delete(timer); if (active) send(); }, due - now);
      timers.add(timer);
    };
  };
  const up = direction();
  const down = direction();
  await page.routeWebSocket("**/ws", socket => {
    const server = socket.connectToServer();
    socket.onMessage(message => up(() => server.send(message)));
    server.onMessage(message => down(() => socket.send(message)));
  });

  try {
    await page.goto(legacy ? `${baseURL}/?netsmooth=0` : baseURL!);
    await page.locator('[data-ose="player-name"]').fill(`smoothness ${label}`.trim());
    await page.locator('[data-ose="play"]').click();
    // A campaign of its own, as controls_direction does: the shared one's pads fill with other tests'
    // vessels, and then Launch has nowhere to roll out to.
    await page.locator('[data-ose="session-join"]').first().waitFor();
    if (!(await page.locator("#create-name").isVisible())) {
      await page.locator("#create-details > summary").click();
    }
    await page.selectOption("#create-mode", "Sandbox");
    await page.fill("#create-name", `smoothness ${testInfo.project.name}`);
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
    await page.locator('[data-ose="vab-name"]').fill("Smoothness rocket");
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

    // Sample the client's summaries for `seconds`, steering for the middle third so turning is in the mix.
    const windows: Window[] = [];
    let last = "";
    const started = performance.now();
    let steering = false;
    while (performance.now() - started < seconds * 1000) {
      const elapsed = performance.now() - started;
      if (!steering && elapsed > seconds * 1000 / 3) { steering = true; await page.keyboard.down("w"); }
      if (steering && elapsed > seconds * 2000 / 3) { steering = false; await page.keyboard.up("w"); }
      const json = (await page.locator("body").getAttribute("data-ose-net")) ?? "";
      if (json && json !== last) { last = json; windows.push(JSON.parse(json)); }
      await page.waitForTimeout(200);
    }
    if (steering) await page.keyboard.up("w");
    // The first summary still covers the launch and any loading stall before it.
    windows.shift();

    const all = (pick: (w: Window) => number) => windows.map(pick);
    const max = (xs: number[]) => xs.reduce((a, b) => Math.max(a, b), 0);
    const sum = (xs: number[]) => xs.reduce((a, b) => a + b, 0);
    const median = (xs: number[]) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.floor((s.length - 1) / 2)] : 0; };
    const report = {
      label, mode: legacy ? "legacy" : "smoothed", browser: testInfo.project.name, delayMs, jitterMs, seconds,
      windows: windows.length,
      webgpu: await page.locator("body").evaluate(body => body.classList.contains("webgpu")),
      // frame-to-frame motion residual of the flown vessel, metres: the jank the player sees
      residualM: { p50: median(all(w => w.residualM.p50)), p95: median(all(w => w.residualM.p95)), max: max(all(w => w.residualM.max)) },
      jumpsOver50cm: sum(all(w => w.jumps)),
      frames: sum(all(w => w.frameGapMs.n)),
      frameGapMs: { p50: median(all(w => w.frameGapMs.p50)), p95: median(all(w => w.frameGapMs.p95)), max: max(all(w => w.frameGapMs.max)) },
      stallsOver50ms: sum(all(w => w.stalls)),
      // universal time the server advanced per wall second (1 when its simulation keeps up with the clock)
      utRate: median(all(w => w.utRate)),
      clockStepMs: { p95: median(all(w => w.clockStepMs.p95)), max: max(all(w => w.clockStepMs.max)) },
      clockPendingMs: { p95: median(all(w => w.clockPendingMs.p95)), max: max(all(w => w.clockPendingMs.max)) },
      arrivalGapMs: { p50: median(all(w => w.arrivalGapMs.p50)), p95: median(all(w => w.arrivalGapMs.p95)), max: max(all(w => w.arrivalGapMs.max)) },
      handlerMs: { p95: median(all(w => w.handlerMs.p95)), max: max(all(w => w.handlerMs.max)) },
      streamKbps: median(all(w => w.streamKbps)),
    };
    console.log("netcode smoothness", JSON.stringify(report));
    await testInfo.attach("netcode-smoothness.json", { body: JSON.stringify({ report, windows }, null, 2), contentType: "application/json" });
    expect(errors).toEqual([]);
    expect(windows.length).toBeGreaterThanOrEqual(Math.min(5, seconds - 1));
  } finally {
    active = false;
    for (const timer of timers) clearTimeout(timer);
  }
});
