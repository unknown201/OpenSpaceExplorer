// The gameplay session as an ordered chain of steps, run in a real browser against the served client.
//
// Each step is what a player does next in the browser. A step whose client feature does not exist yet
// returns BLOCKED naming the feature (and the roadmap phase); the chain keeps going. SKIPPED means this
// browser or machine cannot do it (no WebGPU adapter), which is not a game problem. FAILED means the feature
// is there and misbehaves, or the page itself is broken. When a feature lands, replace the BLOCKED body with
// the real interaction and move the frontier in tests/gameplay.spec.ts.
//
// Client hooks the steps look for (the contract the client is asked to honour, see ADR-0004):
//   #status                     text set by the WASM start function once the module runs
//   canvas                      the render surface
//   [data-ose="ut"]             universal time readout, updated from server TimeState
//   [data-ose="body"]           one element per celestial body shown in the map view (data-ose-id = body id)
//   [data-ose="warp"]           the time-warp control (clicking it raises the warp factor)
//   [data-ose="vessel"]         the player's active vessel in the map view
//   [data-ose="menu"]           the main menu, shown at start; [data-ose="player-name"] and [data-ose="play"],
//                               then [data-ose="session-list"] with a [data-ose="session-join"] per session
//   [data-ose="breadcrumbs"]    star > body > vessel for what the camera looks at
//   [data-ose="ksc"]            the KSC hub screen; [data-ose="ksc-tracking"] opens the map, [data-ose="ksc-launchpad"]
//                               the launch pad ([data-ose="craft"] entries with [data-ose="craft-launch"]);
//                               [data-ose="ksc-button"] in the HUD reopens the hub
//   [data-ose="vab"]            the editor: [data-ose="vab-categories"] buttons, [data-ose="vab-part"] rows,
//                               [data-ose="vab-canvas"] (data-ose-vab-parts = parts drawn), vab-name, vab-save,
//                               vab-launch, vab-remove, vab-clear, vab-report (the server's engineer report)
//   [data-ose="add-node"]       adds a maneuver node on the player's vessel
//   [data-ose="maneuver-node"]  a maneuver node on the player's trajectory (data-ose-ut = burn time)
import type { ConsoleMessage, Page, WebSocket } from "@playwright/test";
import { blocked, failed, passed, skipped, type Outcome, type StepResult } from "./report.js";

export const STEP_NAMES = [
  "page_loads",
  "wasm_client_starts",
  "main_menu",
  "ksc_hub",
  "clean_console",
  "webgpu_adapter",
  "render_canvas",
  "websocket_session",
  "time_display",
  "map_view_bodies",
  "time_warp_control",
  "player_vessel",
  "breadcrumbs",
  "maneuver_node",
  "vab",
  "launchpad",
] as const;
export type StepName = (typeof STEP_NAMES)[number];

/** Everything the page emitted while loading; listeners are attached before navigation. */
class Probe {
  consoleErrors: string[] = [];
  pageErrors: string[] = [];
  sockets: WebSocket[] = [];
  wasmBytes = 0;

  constructor(page: Page) {
    page.on("console", (m: ConsoleMessage) => {
      if (m.type() === "error") this.consoleErrors.push(m.text());
    });
    page.on("pageerror", (e) => this.pageErrors.push(e.message));
    page.on("websocket", (ws) => this.sockets.push(ws));
    page.on("response", async (r) => {
      if (r.url().endsWith(".wasm")) {
        const len = Number(r.headers()["content-length"] ?? 0);
        this.wasmBytes = len || (await r.body().catch(() => Buffer.alloc(0))).length;
      }
    });
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function timed(name: StepName, f: () => Promise<Outcome>): Promise<StepResult> {
  const start = Date.now();
  let outcome: Outcome;
  try {
    outcome = await f();
  } catch (e) {
    outcome = failed(e);
  }
  return { name, outcome, elapsedMs: Date.now() - start };
}

type AdapterProbe =
  | { ok: true; vendor: string; arch: string; desc: string; features: number }
  | { ok: false; why: string };

/** Run the whole chain on `page` and return one result per step, in STEP_NAMES order. */
export async function runGameplaySession(page: Page, baseURL: string): Promise<StepResult[]> {
  const probe = new Probe(page);
  const results: StepResult[] = [];
  let pageUp = false;
  const notLoaded = skipped("page did not load");

  // 1. The server serves the client page.
  results.push(
    await timed("page_loads", async () => {
      const t0 = Date.now();
      const resp = await page.goto(baseURL + "/", { waitUntil: "domcontentloaded", timeout: 30_000 });
      if (!resp) return failed("no response for /");
      if (!resp.ok()) return failed(`GET / answered HTTP ${resp.status()}`);
      const title = await page.title();
      if (title !== "OpenSpaceExplorer") return failed(`title is ${JSON.stringify(title)}`);
      if ((await page.locator("#status").count()) === 0) return failed("no #status element");
      pageUp = true;
      return passed(`title ${JSON.stringify(title)} in ${Date.now() - t0} ms`);
    }),
  );

  // 2. The WASM module downloads, instantiates and runs its start function.
  results.push(
    await timed("wasm_client_starts", async () => {
      if (!pageUp) return notLoaded;
      const status = page.locator("#status");
      try {
        await status.filter({ hasText: "client loaded" }).waitFor({ timeout: 20_000 });
      } catch {
        const text = await status.textContent();
        const err = probe.pageErrors[0] ?? probe.consoleErrors[0];
        return failed(`#status stayed ${JSON.stringify(text)}${err ? `; ${err}` : ""}`);
      }
      return passed(`wasm ${probe.wasmBytes} bytes, status ${JSON.stringify(await status.textContent())}`);
    }),
  );

  // 2b. The main menu: enter a name and press Play, which starts the session.
  results.push(
    await timed("main_menu", async () => {
      if (!pageUp) return notLoaded;
      const menu = page.locator('[data-ose="menu"]');
      if ((await menu.count()) === 0) return blocked('main menu with name and Play ([data-ose="menu"]); Phase 3');
      if (!(await menu.first().isVisible())) return failed("main menu exists but is not shown at start");
      const name = page.locator('[data-ose="player-name"]');
      if ((await name.count()) > 0) await name.first().fill("e2e pilot");
      await page.locator('[data-ose="play"]').first().click();
      // The sessions screen lists what can be joined; join the first entry.
      const join = page.locator('[data-ose="session-join"]');
      let joined = "";
      try {
        await join.first().waitFor({ timeout: 10_000 });
        const entry = page.locator('[data-ose="session"]').first();
        joined = ((await entry.locator(".name").textContent().catch(() => "")) ?? "").trim();
        await join.first().click();
      } catch {
        const note = ((await page.locator("#menu-note").textContent().catch(() => "")) ?? "").trim();
        return failed(`no session to join after Play${note ? `: ${note}` : ""}`);
      }
      try {
        await Promise.race([
          page.locator('[data-ose="ksc"]').first().waitFor({ state: "visible", timeout: 10_000 }),
          menu.first().waitFor({ state: "hidden", timeout: 10_000 }),
        ]);
      } catch {
        const note = ((await page.locator("#menu-note").textContent().catch(() => "")) ?? "").trim();
        return failed(`nothing happened after Join${note ? `: ${note}` : ""}`);
      }
      const player = ((await page.locator('[data-ose="player"]').first().textContent().catch(() => "")) ?? "").trim();
      return passed(`played as ${JSON.stringify(player || "(no player readout)")}, joined ${JSON.stringify(joined)}`);
    }),
  );

  // 2c. The Kerbal Space Center hub: building buttons; the Tracking Station opens the map.
  results.push(
    await timed("ksc_hub", async () => {
      if (!pageUp) return notLoaded;
      const ksc = page.locator('[data-ose="ksc"]');
      if ((await ksc.count()) === 0) return blocked('KSC hub with building buttons ([data-ose="ksc"]); Phase 5');
      if (!(await ksc.first().isVisible())) return failed("KSC hub exists but is not shown after joining");
      const buttons = await ksc.locator("button[data-ose^='ksc-']").evaluateAll((els) =>
        els.map((e) => `${e.getAttribute("data-ose")}${(e as HTMLButtonElement).disabled ? "(off)" : ""}`),
      );
      await page.locator('[data-ose="ksc-tracking"]').first().click();
      try {
        await page.locator('[data-ose="menu"]').first().waitFor({ state: "hidden", timeout: 5000 });
      } catch {
        return failed("Tracking Station did not open the map");
      }
      return passed(`buildings: ${buttons.join(", ")}`);
    }),
  );

  // 3. Loading produced no console errors or uncaught exceptions.
  results.push(
    await timed("clean_console", async () => {
      if (!pageUp) return notLoaded;
      await sleep(500);
      const errs = [...probe.pageErrors, ...probe.consoleErrors];
      return errs.length === 0 ? passed("no console errors, no page errors") : failed(errs.join(" | "));
    }),
  );

  // 4. This browser offers a WebGPU adapter (an environment fact the renderer will depend on).
  results.push(
    await timed("webgpu_adapter", async () => {
      if (!pageUp) return notLoaded;
      const info: AdapterProbe = await page.evaluate(async () => {
        const gpu = (navigator as unknown as { gpu?: { requestAdapter(): Promise<unknown> } }).gpu;
        if (!gpu) return { ok: false as const, why: "navigator.gpu is undefined" };
        try {
          const adapter = (await gpu.requestAdapter()) as {
            info?: Record<string, string>;
            requestAdapterInfo?: () => Promise<Record<string, string>>;
            features?: Iterable<string>;
          } | null;
          if (!adapter) return { ok: false as const, why: "requestAdapter() returned null" };
          const i = adapter.info ?? (adapter.requestAdapterInfo ? await adapter.requestAdapterInfo() : {});
          const features = adapter.features ? [...adapter.features].length : 0;
          return {
            ok: true as const,
            vendor: i.vendor ?? "",
            arch: i.architecture ?? "",
            desc: i.description ?? "",
            features,
          };
        } catch (e) {
          return { ok: false as const, why: String(e) };
        }
      });
      if (!info.ok) return skipped(`no WebGPU adapter: ${info.why}`);
      const parts = [info.vendor, info.arch, info.desc].filter(Boolean).join(" ");
      return passed(`adapter ${parts || "(unnamed)"}, ${info.features} features`);
    }),
  );

  // 5. The client has a render surface.
  results.push(
    await timed("render_canvas", async () => {
      if (!pageUp) return notLoaded;
      const canvas = page.locator("canvas:visible").first();
      if ((await canvas.count()) === 0) {
        return blocked("client renderer (a <canvas> with a WebGPU surface); Phase 3");
      }
      const box = await canvas.boundingBox();
      if (!box || box.width < 1 || box.height < 1) return failed("canvas has no size");
      await sleep(1500);
      const gpu = page.locator('[data-ose="gpu"]').first();
      const renderer = (await gpu.getAttribute("data-ose-renderer")) ?? "canvas2d";
      const backdrop = (await gpu.getAttribute("data-ose-menu-vessel")) ?? "none (no menu assets on this server)";
      return passed(`canvas ${Math.round(box.width)}x${Math.round(box.height)}, renderer ${renderer}, menu backdrop ${backdrop}`);
    }),
  );

  // 6. The client opens a protocol session to the server.
  results.push(
    await timed("websocket_session", async () => {
      if (!pageUp) return notLoaded;
      const deadline = Date.now() + 3000;
      while (probe.sockets.length === 0 && Date.now() < deadline) await sleep(100);
      const ws = probe.sockets[0];
      if (!ws) return blocked("client WebSocket session to /ws (Hello/Welcome); Phase 3");
      const url = ws.url();
      if (!url.endsWith("/ws")) return failed(`websocket to unexpected url ${url}`);
      try {
        await Promise.race([
          ws.waitForEvent("framereceived", { timeout: 5000 }),
          ws.waitForEvent("close", { timeout: 5000 }).then(() => {
            throw new Error("server closed the session before answering");
          }),
        ]);
      } catch (e) {
        return failed(e);
      }
      return passed(`session on ${url}, first frame received`);
    }),
  );

  // 7. The HUD shows server time and it advances.
  results.push(
    await timed("time_display", async () => {
      if (!pageUp) return notLoaded;
      const ut = page.locator('[data-ose="ut"]');
      if ((await ut.count()) === 0) {
        return blocked('universal-time readout in the HUD ([data-ose="ut"]); Phase 3');
      }
      const a = (await ut.first().textContent()) ?? "";
      await sleep(1500);
      const b = (await ut.first().textContent()) ?? "";
      if (a === b) return failed(`time readout did not advance (${JSON.stringify(a)})`);
      return passed(`UT ${a.trim()} -> ${b.trim()}`);
    }),
  );

  // 8. The map view shows the celestial bodies.
  results.push(
    await timed("map_view_bodies", async () => {
      if (!pageUp) return notLoaded;
      const bodies = page.locator('[data-ose="body"]');
      const n = await bodies.count();
      if (n === 0) return blocked('map view with celestial bodies ([data-ose="body"]); Phase 3');
      const names: string[] = await bodies.evaluateAll((els) =>
        els.map((e) => e.getAttribute("data-ose-id") ?? e.textContent?.trim() ?? "?"),
      );
      if (!names.some((x) => /kerbin/i.test(x))) {
        return failed(`bodies shown but no Kerbin: ${names.join(", ")}`);
      }
      return passed(`${n} bodies: ${names.slice(0, 6).join(", ")}${n > 6 ? ", ..." : ""}`);
    }),
  );

  // 9. Time warp can be raised from the UI.
  results.push(
    await timed("time_warp_control", async () => {
      if (!pageUp) return notLoaded;
      const warp = page.locator('[data-ose="warp"]');
      if ((await warp.count()) === 0) {
        return blocked('time-warp control ([data-ose="warp"]) sending RequestWarp; Phase 3');
      }
      const before = (await warp.first().textContent()) ?? "";
      await warp.first().click();
      await sleep(500);
      const after = (await warp.first().textContent()) ?? "";
      if (before === after) {
        // Lowest-wins warp arbitration: another connected player at a lower warp holds it. The client
        // names the holder; that is the server working, not a client fault.
        const holder = ((await page.locator('[data-ose="warp-holder"]').first().textContent().catch(() => "")) ?? "").trim();
        if (holder) return skipped(`${holder} (another player is connected)`);
        return failed(`warp control did not change (${JSON.stringify(before)})`);
      }
      return passed(`warp ${before.trim()} -> ${after.trim()}`);
    }),
  );

  // 10. The player's vessel appears in the map view.
  results.push(
    await timed("player_vessel", async () => {
      if (!pageUp) return notLoaded;
      const v = page.locator('[data-ose="vessel"]');
      if ((await v.count()) === 0) {
        return blocked('player vessel streamed and drawn in the map view ([data-ose="vessel"]); Phase 4');
      }
      return passed(`vessel shown: ${((await v.first().textContent()) ?? "").trim()}`);
    }),
  );

  // 10b. Breadcrumbs name what the camera looks at: star > body > vessel once the probe is followed.
  results.push(
    await timed("breadcrumbs", async () => {
      if (!pageUp) return notLoaded;
      const crumbs = page.locator('[data-ose="breadcrumbs"]');
      if ((await crumbs.count()) === 0) return blocked('breadcrumbs star > body > vessel ([data-ose="breadcrumbs"]); Phase 3');
      await sleep(300);
      const text = ((await crumbs.first().textContent()) ?? "").replace(/\s+/g, " ").trim();
      if (!/kerbol/i.test(text) || !/kerbin/i.test(text)) return failed(`breadcrumbs read ${JSON.stringify(text)}`);
      return passed(text);
    }),
  );

  // 11. A maneuver node can be placed on the vessel's trajectory: click "add node", then the node marker
  // exists and carries the burn time it was scheduled for.
  results.push(
    await timed("maneuver_node", async () => {
      if (!pageUp) return notLoaded;
      const add = page.locator('[data-ose="add-node"]');
      if ((await add.count()) === 0) {
        return blocked('maneuver-node editing in the map view ([data-ose="add-node"], [data-ose="maneuver-node"]); Phase 5');
      }
      await add.first().click();
      const node = page.locator('[data-ose="maneuver-node"]');
      try {
        await node.first().waitFor({ timeout: 3000 });
      } catch {
        return failed("clicking add-node produced no maneuver node");
      }
      const ut = await node.first().getAttribute("data-ose-ut");
      if (!ut || !Number.isFinite(Number(ut))) return failed(`node has no burn time (data-ose-ut=${ut})`);
      await sleep(700);
      const reason = ((await page.locator("#node-reason").textContent().catch(() => "")) ?? "").trim();
      if (reason) return failed(`server refused the burn: ${reason}`);
      return passed(`node scheduled at UT ${Number(ut).toFixed(1)}`);
    }),
  );

  // 11b. The Vehicle Assembly Building: pick a pod, a tank and an engine from the picker, name the craft, save
  // it, and read the server's engineer report back.
  results.push(
    await timed("vab", async () => {
      if (!pageUp) return notLoaded;
      const open = page.locator('[data-ose="ksc-button"]');
      if ((await open.count()) === 0) return blocked('VAB editor ([data-ose="vab"]); Phase 5');
      await open.first().click();
      const vabButton = page.locator('[data-ose="ksc-vab"]');
      if ((await vabButton.count()) === 0 || (await vabButton.first().isDisabled())) {
        return blocked('VAB editor ([data-ose="vab"]); Phase 5');
      }
      await vabButton.first().click();
      const vab = page.locator('[data-ose="vab"]');
      await vab.first().waitFor({ state: "visible", timeout: 5000 });
      await page.locator('[data-ose="vab-clear"]').first().click();
      const picked: string[] = [];
      for (const category of ["Pods", "FuelTank", "Engine"]) {
        const tab = page.locator(`[data-ose="vab-categories"] button[data-ose-category="${category}"]`);
        if ((await tab.count()) === 0) return failed(`no ${category} category in the picker (catalog loaded?)`);
        await tab.first().click();
        const part = page.locator('[data-ose="vab-part"]').first();
        try {
          await part.waitFor({ timeout: 5000 });
        } catch {
          return failed(`no parts listed under ${category}`);
        }
        picked.push(((await part.getAttribute("data-ose-part")) ?? "?").trim());
        await part.click();
      }
      const drawn = await page.locator('[data-ose="vab-canvas"]').first().getAttribute("data-ose-vab-parts");
      if (drawn !== "3") return failed(`expected 3 parts on the canvas, saw ${drawn}`);
      await page.locator('[data-ose="vab-name"]').first().fill("e2e stack");
      await page.locator('[data-ose="vab-save"]').first().click();
      const report = page.locator('[data-ose="vab-report"]');
      try {
        await report.filter({ hasText: /Saved as|Not saved/ }).waitFor({ timeout: 10_000 });
      } catch {
        return failed(`no save report (${JSON.stringify((await report.textContent()) ?? "")})`);
      }
      const text = ((await report.textContent()) ?? "").trim();
      if (!/^Saved as/.test(text)) return failed(text);
      await page.locator('[data-ose="vab-back"]').first().click();
      await page.locator('[data-ose="ksc-tracking"]').first().click();
      return passed(`${picked.join(" + ")}; ${text}`);
    }),
  );

  // 12. The launch pad lists the craft library and launches a stock craft into orbit.
  results.push(
    await timed("launchpad", async () => {
      if (!pageUp) return notLoaded;
      const open = page.locator('[data-ose="ksc-button"]');
      if ((await open.count()) === 0) return blocked('launchpad dialog over the craft library ([data-ose="launchpad"]); Phase 5');
      await open.first().click();
      await page.locator('[data-ose="ksc-launchpad"]').first().click();
      const launch = page.locator('[data-ose="craft-launch"]');
      try {
        await launch.first().waitFor({ timeout: 10_000 });
      } catch {
        return failed("launch pad showed no crafts");
      }
      const n = await launch.count();
      const first = page.locator('[data-ose="craft"]').first();
      const craft = ((await first.locator(".name").textContent()) ?? "").trim();
      const vesselsBefore = await page.locator('[data-ose="vessel"]').count();
      await launch.first().click();
      try {
        await page.locator('[data-ose="menu"]').first().waitFor({ state: "hidden", timeout: 10_000 });
      } catch {
        const note = ((await page.locator("#menu-note").textContent().catch(() => "")) ?? "").trim();
        return failed(`launch did not complete${note ? `: ${note}` : ""}`);
      }
      await sleep(700);
      const vesselsAfter = await page.locator('[data-ose="vessel"]').count();
      if (vesselsAfter <= vesselsBefore) return failed(`launched but no new vessel (${vesselsBefore} -> ${vesselsAfter})`);
      return passed(`${n} crafts listed; launched ${JSON.stringify(craft)}, vessels ${vesselsBefore} -> ${vesselsAfter}`);
    }),
  );

  return results;
}
