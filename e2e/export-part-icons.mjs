// Part icons for the VAB picker (docs/ui-parity-ksp2.md V10): the client's own VAB renderer in icon mode
// (`?particons`: no hall, plain backdrop, the part framed at the centre) photographs every catalog part.
// Writes 600 px squares to <outDir>/<part id>.png; tools/shrink_part_icons.py makes the 128 px icons in
// assets/web/ui/parts, pushed with `tools/push_assets.py ui`.
//
//   node export-part-icons.mjs http://127.0.0.1:7811 <outDir> [part id ...]
import { chromium } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";

const base = process.argv[2] ?? "http://127.0.0.1:7811";
const out = process.argv[3] ?? "part-icons";
const only = process.argv.slice(4);
fs.mkdirSync(out, { recursive: true });
const catalog = JSON.parse(
  fs.readFileSync(path.join(import.meta.dirname, "..", "assets", "own", "parts-catalog.json"), "utf8"),
);
const ids = only.length ? only : catalog.map((p) => p.id);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const browser = await chromium.launch({
  channel: "chromium",
  headless: true,
  args: ["--enable-unsafe-webgpu", "--ignore-gpu-blocklist", "--disable-dawn-features=use_dxc"],
});
const page = await browser.newPage({ viewport: { width: 1600, height: 900 } });
await page.addInitScript(() => localStorage.setItem("ose.ftue", "off"));
await page.goto(base + "/?particons");
await page.locator("#status", { hasText: "client loaded" }).waitFor({ timeout: 30_000 });
await sleep(2000);
await page.locator('[data-ose="player-name"]').fill("icon camera");
await page.locator('[data-ose="play"]').click();
await sleep(2500);
await page.locator("#create-details > summary").click();
await page.selectOption('[data-ose="create-mode"]', "Sandbox");
await page.fill('[data-ose="create-name"]', "part icons");
await page.locator('[data-ose="create"]').click();
await page.locator('[data-ose="ksc"]').waitFor({ state: "visible", timeout: 15_000 });
await sleep(2000);
await page.locator('[data-ose="ksc-vab"]').click();
await sleep(5000);
let done = 0;
const missing = [];
for (const id of ids) {
  await page.locator('[data-ose="vab-clear"]').click();
  await page.fill('[data-ose="vab-search"]', id);
  await sleep(150);
  const row = page.locator(`[data-ose="vab-part"][data-ose-part="${id}"]`);
  if ((await row.count()) === 0) {
    missing.push(id);
    continue;
  }
  await row.first().click();
  // The part's model streams in; give the first of a kind longer.
  await sleep(done === 0 ? 4000 : 1200);
  await page.mouse.move(5, 895);
  await page.screenshot({ path: path.join(out, `${id}.png`), clip: { x: 500, y: 150, width: 600, height: 600 } });
  done++;
  if (done % 25 === 0) console.log(`${done} / ${ids.length}`);
}
console.log(`icons: ${done}, not in the picker: ${missing.length}${missing.length ? " (" + missing.join(", ") + ")" : ""}`);
await browser.close();
process.exit(0);
