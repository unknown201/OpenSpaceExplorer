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
  "flight",
  "rnd",
  "tracking_station",
  "missions",
  "campaign_cycle",
  "vab_rocket",
  "eva",
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
        // The server's own campaign when it is listed: later steps start campaigns of their own, which a rerun
        // against the same server would otherwise pick up first.
        const shared = page.locator('[data-ose="session"]', { hasText: "Shared campaign" });
        const entry = (await shared.count()) > 0 ? shared.first() : page.locator('[data-ose="session"]').first();
        joined = ((await entry.locator(".name").textContent().catch(() => "")) ?? "").trim();
        await entry.locator('[data-ose="session-join"]').click();
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
      // The readout shows whole seconds and a debug server with several structured vessels runs below real
      // time, so give it a few seconds.
      await sleep(3000);
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
        const why = ((await page.locator("#node-reason").textContent().catch(() => "")) ?? "").trim();
        const n = await node.count();
        return failed(`clicking add-node produced no maneuver node (${n} hidden markers${why ? `; ${why}` : ""})`);
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
      // Stock crafts that can fly (delta-v listed), the biggest first; an Exploration campaign refuses crafts
      // with parts it has not researched, so try the next one when the pad says so.
      const rows = page.locator('[data-ose="craft"][data-ose-stock="1"]');
      const candidates: { i: number; dv: number }[] = [];
      for (let i = 0; i < (await rows.count()); i++) {
        const dv = Number((await rows.nth(i).getAttribute("data-ose-dv")) ?? "0");
        if (dv > 300) candidates.push({ i, dv });
      }
      candidates.sort((a, b) => b.dv - a.dv);
      const vesselsBefore = await page.locator('[data-ose="vessel"]').count();
      let craft = "";
      let launched = false;
      const refusals: string[] = [];
      for (const c of candidates) {
        const row = rows.nth(c.i);
        craft = ((await row.locator(".name").textContent()) ?? "").trim();
        await row.locator('[data-ose="craft-launch"]').first().click();
        const t0 = Date.now();
        let note = "";
        while (Date.now() - t0 < 10_000) {
          if (!(await page.locator('[data-ose="menu"]').first().isVisible())) {
            launched = true;
            break;
          }
          note = ((await page.locator("#menu-note").textContent().catch(() => "")) ?? "").trim();
          if (note.includes("refused")) break;
          await sleep(200);
        }
        if (launched) break;
        refusals.push(`${craft}: ${note || "no answer"}`);
      }
      if (!launched) return failed(`no craft could launch: ${refusals.join(" | ").slice(0, 400)}`);
      await sleep(700);
      const vesselsAfter = await page.locator('[data-ose="vessel"]').count();
      if (vesselsAfter <= vesselsBefore) return failed(`launched but no new vessel (${vesselsBefore} -> ${vesselsAfter})`);
      return passed(`${n} crafts listed; launched ${JSON.stringify(craft)}, vessels ${vesselsBefore} -> ${vesselsAfter}`);
    }),
  );

  // 13. Flight: the launch put the craft on the pad and opened the flight view; full throttle and stage,
  // and the craft leaves the ground under power: more than 20 m up and Flying, not a bounce on the pad.
  results.push(
    await timed("flight", async () => {
      if (!pageUp) return notLoaded;
      const hud = page.locator('[data-ose="flight"]');
      if ((await hud.count()) === 0) return blocked('flight view with HUD after launching from the pad ([data-ose="flight"]); Phase 6');
      try {
        await page.locator('[data-ose="flight"].open').first().waitFor({ timeout: 5_000 });
      } catch {
        return failed("launching from the pad did not open the flight view");
      }
      const altOf = async () => Number((await hud.first().getAttribute("data-ose-altitude")) ?? "NaN");
      const t0 = Date.now();
      while (!Number.isFinite(await altOf()) && Date.now() - t0 < 5_000) await sleep(200);
      const before = await altOf();
      if (!Number.isFinite(before)) return failed("the flight HUD shows no altitude for the launched vessel");
      await page.locator('[data-ose="flight-throttle-full"]').first().click();
      await sleep(300);
      await page.locator('[data-ose="flight-stage"]').first().click();
      let after = before;
      let situation = "";
      let engineMax = 0;
      let thrustSeen = false;
      const audio = page.locator('[data-ose="audio"]');
      const t1 = Date.now();
      while (Date.now() - t1 < 20_000) {
        await sleep(500);
        if ((await audio.count()) > 0) {
          engineMax = Math.max(engineMax, Number((await audio.first().getAttribute("data-ose-engine")) ?? "0"));
          thrustSeen ||= !/ 0 kN/.test((await page.locator("#fh-twr").textContent()) ?? " 0 kN");
        }
        after = await altOf();
        situation = (await hud.first().getAttribute("data-ose-situation")) ?? "";
        if (after > before + 20 && situation === "Flying") break;
      }
      if (!(after > before + 20 && situation === "Flying")) {
        const note = ((await page.locator("#fh-note").textContent().catch(() => "")) ?? "").trim();
        return failed(`the craft did not climb after full throttle and stage (${before.toFixed(0)} m -> ${after.toFixed(0)} m, ${situation}${note ? `; ${note}` : ""}) [${await page.locator("#fh-stage").textContent()}, ${await page.locator("#fh-fuel").textContent()}, ${await page.locator("#fh-twr").textContent()}]`);
      }
      // Sound follows the flight: the engine loop ran under thrust and the music is the body's.
      if ((await audio.count()) > 0) {
        const music = (await audio.first().getAttribute("data-ose-music")) ?? "";
        if (thrustSeen && !(engineMax > 0)) return failed("the engine sound stayed silent under thrust (data-ose-engine 0)");
        if (!music.startsWith("music_kerbin")) return failed(`flight music is "${music}", not Kerbin's`);
      }
      return passed(`lifted off: ${before.toFixed(0)} m -> ${after.toFixed(0)} m, ${situation}`);
    }),
  );

  // 14. Research and Development: back to the KSC through the pause menu, open the R&D Center, pick a node
  // and read its info panel.
  results.push(
    await timed("rnd", async () => {
      if (!pageUp) return notLoaded;
      const open = page.locator('[data-ose="ksc-rnd"]');
      if ((await open.count()) === 0) return blocked('R&D Center with the tech tree ([data-ose="rnd"]); Phase 8');
      if (!(await page.locator('[data-ose="ksc"]').first().isVisible())) {
        await page.keyboard.press("Escape");
        await page.locator('[data-ose="to-ksc"]').first().click();
      }
      await open.first().click();
      const nodes = page.locator('[data-ose="rd-node"]');
      try {
        await nodes.first().waitFor({ timeout: 5_000 });
      } catch {
        return failed("the R&D Center showed no tech nodes");
      }
      const n = await nodes.count();
      const available = page.locator('[data-ose="rd-node"].available');
      if ((await available.count()) === 0) return failed(`${n} tech nodes but none available after the start node`);
      await available.first().click();
      const title = ((await page.locator('[data-ose="rd-info"] h3').first().textContent()) ?? "").trim();
      if (!title) return failed("clicking a tech node showed no info");
      return passed(`${n} tech nodes; selected ${JSON.stringify(title)}`);
    }),
  );

  // 15. Tracking Station: back to the KSC, open the station, pick a vessel and read its orbit card.
  results.push(
    await timed("tracking_station", async () => {
      if (!pageUp) return notLoaded;
      const open = page.locator('[data-ose="ksc-tracking"]');
      if ((await open.count()) === 0 || (await page.locator('[data-ose="tracking"]').count()) === 0) {
        return blocked('Tracking Station with the vessel list ([data-ose="tracking"]); Phase 8');
      }
      const back = page.locator('[data-ose="rnd-back"]');
      if (await back.first().isVisible()) await back.first().click();
      await open.first().click();
      const rows = page.locator('[data-ose="ts-vessel"]');
      try {
        await rows.first().waitFor({ timeout: 5_000 });
      } catch {
        return failed("the Tracking Station listed no vessels");
      }
      const n = await rows.count();
      await rows.first().click();
      const card = page.locator('[data-ose="ts-info"] h3');
      try {
        await card.first().waitFor({ timeout: 3_000 });
      } catch {
        return failed("selecting a vessel showed no orbit card");
      }
      const name = ((await card.first().textContent()) ?? "").trim();
      const focus = page.locator('[data-ose="ts-focus"]');
      if ((await focus.count()) === 0) return failed("the vessel card has no Focus button");
      await focus.first().click();
      return passed(`${n} vessels tracked; selected ${JSON.stringify(name)}`);
    }),
  );

  // 15b. Mission Control: open it from the KSC, pick an offered mission and accept it (protocol 18).
  results.push(
    await timed("missions", async () => {
      if (!pageUp) return notLoaded;
      if (await page.locator('[data-ose="ts-back"]').first().isVisible()) await page.locator('[data-ose="ts-back"]').first().click();
      const open = page.locator('[data-ose="ksc-mission"]');
      if ((await open.count()) === 0 || !(await open.first().isVisible())) return skipped("not on the KSC screen");
      await open.first().click();
      const list = page.locator('[data-ose="mc-mission"]');
      try {
        await list.first().waitFor({ timeout: 5_000 });
      } catch {
        return failed("Mission Control listed no missions");
      }
      let offered = page.locator('[data-ose="mc-mission"].offered');
      if ((await offered.count()) === 0) {
        await page.locator('[data-ose="mc-tab-secondary"]').first().click();
        offered = page.locator('[data-ose="mc-mission"].offered');
      }
      if ((await offered.count()) === 0) {
        await page.locator('[data-ose="mc-back"]').first().click();
        return passed(`${await list.count()} missions listed, none offered to accept`);
      }
      const name = ((await offered.first().locator(".name").textContent()) ?? "").trim();
      await offered.first().click();
      const accept = page.locator('[data-ose="mc-accept"]');
      try {
        await accept.first().waitFor({ timeout: 3_000 });
      } catch {
        return failed(`the offered mission ${JSON.stringify(name)} has no Accept button`);
      }
      await accept.first().click();
      const note = page.locator('[data-ose="mc-note"]');
      try {
        await note.filter({ hasText: /accepted|refused|not|already/i }).first().waitFor({ timeout: 5_000 });
      } catch {
        return failed(`no answer after accepting ${JSON.stringify(name)}`);
      }
      const text = ((await note.first().textContent()) ?? "").trim();
      await page.locator('[data-ose="mc-back"]').first().click();
      if (text !== "Mission accepted") return failed(`accepting ${JSON.stringify(name)}: ${text}`);
      return passed(`accepted ${JSON.stringify(name)}`);
    }),
  );

  // 16. Campaigns: exit to the main menu, find the campaign on the list and resume it, exit again and start a
  // new Sandbox campaign (protocol 22: several campaigns per server, LeaveSession).
  results.push(
    await timed("campaign_cycle", async () => {
      if (!pageUp) return notLoaded;
      const exit = page.locator('[data-ose="ksc-sessions"]');
      if ((await exit.count()) === 0) return blocked("Exit to Main Menu on the KSC screen; Phase 8");
      if (!(await page.locator('[data-ose="ksc"]').first().isVisible())) {
        if (!(await page.locator('[data-ose="menu"]').first().isVisible())) await page.keyboard.press("Escape");
        if (await page.locator('[data-ose="to-ksc"]').first().isVisible()) await page.locator('[data-ose="to-ksc"]').first().click();
        if (await page.locator('[data-ose="ts-back"]').first().isVisible()) await page.locator('[data-ose="ts-back"]').first().click();
      }
      const name = ((await page.locator('[data-ose="campaign"]').textContent()) ?? "").split(" (")[0].trim();
      await exit.first().click();
      const rows = page.locator('[data-ose="session"]').filter({ hasText: name || "campaign" });
      try {
        await rows.first().waitFor({ timeout: 5_000 });
      } catch {
        return failed(`after exiting, ${JSON.stringify(name)} is not on the campaign list`);
      }
      await rows.first().locator('[data-ose="session-join"]').click();
      try {
        await page.locator('[data-ose="ksc"]').first().waitFor({ state: "visible", timeout: 10_000 });
      } catch {
        return failed(`resuming ${JSON.stringify(name)} did not reach the KSC (${(await page.locator("#menu-note").textContent()) ?? ""})`);
      }
      await exit.first().click();
      if (!(await page.locator("#create-name").isVisible())) await page.locator("#create-details > summary").click();
      const presets = await page.locator("#create-preset option").count();
      if (presets === 0) return failed("the new campaign form has no difficulty presets");
      await page.selectOption("#create-mode", "Sandbox");
      await page.fill("#create-name", "e2e sandbox");
      await page.locator('[data-ose="create"]').click();
      try {
        await page.locator('[data-ose="ksc"]').first().waitFor({ state: "visible", timeout: 10_000 });
      } catch {
        return failed(`starting a second campaign did not reach the KSC (${(await page.locator("#menu-note").textContent()) ?? ""})`);
      }
      const now = ((await page.locator('[data-ose="campaign"]').textContent()) ?? "").trim();
      if (!now.startsWith("e2e sandbox")) return failed(`the KSC shows ${JSON.stringify(now)}, not the new campaign`);
      return passed(`exited ${JSON.stringify(name)}, resumed it, then started "e2e sandbox" (${presets} difficulty presets)`);
    }),
  );

  // 17. A rocket built in the VAB (Mk1 pod, FL-T400 tank, LV-T45 engine): Save and launch to the pad, then Z and
  // Space as a player presses them, and it lifts off.
  results.push(
    await timed("vab_rocket", async () => {
      if (!pageUp) return notLoaded;
      const vabButton = page.locator('[data-ose="ksc-vab"]');
      if (!(await vabButton.first().isVisible())) return skipped("not on the KSC screen");
      await vabButton.first().click();
      await page.locator('[data-ose="vab"]').first().waitFor({ state: "visible", timeout: 5000 });
      await page.locator('[data-ose="vab-clear"]').first().click();
      const picked: string[] = [];
      for (const q of ["Tin Can", "FL-T400", "LV-T45"]) {
        await page.fill('[data-ose="vab-search"]', q);
        const part = page.locator('[data-ose="vab-part"]').first();
        try {
          await part.waitFor({ timeout: 3000 });
        } catch {
          return failed(`no part found for ${JSON.stringify(q)}`);
        }
        picked.push(((await part.getAttribute("data-ose-part")) ?? "?").trim());
        await part.click();
      }
      await page.fill('[data-ose="vab-search"]', "");
      await page.fill('[data-ose="vab-name"]', "e2e rocket");
      await page.locator('[data-ose="vab-launch"]').first().click();
      const hud = page.locator('[data-ose="flight"]');
      try {
        await page.locator('[data-ose="flight"].open').first().waitFor({ timeout: 10_000 });
      } catch {
        const why = `${(await page.locator('[data-ose="vab-report"]').textContent()) ?? ""} ${(await page.locator("#menu-note").textContent()) ?? ""}`;
        return failed(`Save and launch did not open the flight view on the pad: ${why.trim()}`);
      }
      const altOf = async () => Number((await hud.first().getAttribute("data-ose-altitude")) ?? "NaN");
      const t0 = Date.now();
      while (!Number.isFinite(await altOf()) && Date.now() - t0 < 5_000) await sleep(200);
      const before = await altOf();
      await page.keyboard.press("z");
      await sleep(300);
      await page.keyboard.press("Space");
      let after = before;
      let situation = "";
      const t1 = Date.now();
      while (Date.now() - t1 < 20_000) {
        await sleep(500);
        after = await altOf();
        situation = (await hud.first().getAttribute("data-ose-situation")) ?? "";
        if (after > before + 20 && situation === "Flying") break;
      }
      if (!(after > before + 20 && situation === "Flying")) {
        return failed(`${picked.join(" + ")} did not lift off after Z and Space (${before.toFixed(0)} m -> ${after.toFixed(0)} m, ${situation}; ${(await page.locator("#fh-twr").textContent()) ?? ""})`);
      }
      return passed(`${picked.join(" + ")}: ${before.toFixed(0)} m -> ${after.toFixed(0)} m, ${situation}`);
    }),
  );

  // 18. A Kerbal on EVA (protocol 15): back at the KSC, a Mk1 pod with a parachute on the pad, EVA, plant the
  // flag, board again. Then the server's own campaign is made live again for the next run.
  results.push(
    await timed("eva", async () => {
      if (!pageUp) return notLoaded;
      const restore = async () => {
        if (!(await page.locator('[data-ose="menu"]').first().isVisible())) await page.keyboard.press("Escape");
        if (await page.locator('[data-ose="to-sessions"]').first().isVisible()) await page.locator('[data-ose="to-sessions"]').first().click();
        else if (await page.locator('[data-ose="ksc-sessions"]').first().isVisible()) await page.locator('[data-ose="ksc-sessions"]').first().click();
        const shared = page.locator('[data-ose="session"]', { hasText: "Shared campaign" });
        try {
          await shared.first().waitFor({ timeout: 5_000 });
          await shared.first().locator('[data-ose="session-join"]').click();
        } catch {
          /* no shared campaign on this server */
        }
        await sleep(1500);
      };
      if (!(await page.locator('[data-ose="menu"]').first().isVisible())) await page.keyboard.press("Escape");
      const toKsc = page.locator('[data-ose="to-ksc"]');
      if (await toKsc.first().isVisible()) await toKsc.first().click();
      if (!(await page.locator('[data-ose="ksc-vab"]').first().isVisible())) {
        await restore();
        return skipped("not on the KSC screen");
      }
      await page.locator('[data-ose="ksc-vab"]').first().click();
      await page.locator('[data-ose="vab-clear"]').first().click();
      for (const q of ["Tin Can", "Mk16-XL"]) {
        await page.fill('[data-ose="vab-search"]', q);
        await page.locator('[data-ose="vab-part"]').first().click();
      }
      await page.fill('[data-ose="vab-search"]', "");
      await page.fill('[data-ose="vab-name"]', "e2e capsule");
      await page.locator('[data-ose="vab-launch"]').first().click();
      const hud = page.locator('[data-ose="flight"]');
      try {
        await page.locator('[data-ose="flight"].open').first().waitFor({ timeout: 10_000 });
      } catch {
        await restore();
        return failed("the capsule did not roll out to the pad");
      }
      const craftVessel = await hud.first().getAttribute("data-ose-vessel");
      const eva = page.locator('[data-ose="flight-eva"]');
      try {
        await eva.first().waitFor({ state: "visible", timeout: 8_000 });
      } catch {
        await restore();
        return failed("no EVA button for a crewed pod on the pad");
      }
      await eva.first().click();
      const kerbalVessel = async () => (await hud.first().getAttribute("data-ose-vessel")) ?? "";
      const t0 = Date.now();
      while ((await kerbalVessel()) === craftVessel && Date.now() - t0 < 8_000) await sleep(200);
      if ((await kerbalVessel()) === craftVessel) {
        const why = (await page.locator("#fh-note").textContent()) ?? "";
        await restore();
        return failed(`EVA did not hand the view to a Kerbal (${why})`);
      }
      const flag = page.locator('[data-ose="flight-flag"]');
      try {
        await flag.first().waitFor({ state: "visible", timeout: 8_000 });
      } catch {
        await restore();
        return failed("no Plant flag button for a Kerbal standing on the pad");
      }
      await flag.first().click();
      try {
        await page.locator("#fh-note").filter({ hasText: "flag planted" }).waitFor({ state: "attached", timeout: 5_000 });
      } catch {
        const why = (await page.locator("#fh-note").textContent()) ?? "";
        await restore();
        return failed(`planting the flag: ${why}`);
      }
      await page.locator('[data-ose="flight-board"]').first().click();
      const t1 = Date.now();
      while ((await kerbalVessel()) !== craftVessel && Date.now() - t1 < 8_000) await sleep(200);
      const boarded = (await kerbalVessel()) === craftVessel;
      const why = (await page.locator("#fh-note").textContent()) ?? "";
      await restore();
      if (!boarded) return failed(`boarding the capsule again: ${why}`);
      return passed("EVA, flag planted, boarded the capsule again");
    }),
  );

  return results;
}
