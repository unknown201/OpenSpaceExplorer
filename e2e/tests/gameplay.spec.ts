// The gameplay session as an automated test in each browser project (Chromium, Firefox).
//
// FRONTIER records how far the client lets a player get today. The test fails when any step FAILS (the
// page is broken, or a feature exists and misbehaves) or when a step expected to pass is now blocked or
// skipped (a regression). A step expected to be blocked that now PASSES is progress: printed loudly, not
// a failure; move the frontier forward in the same change that lands the feature.
import { expect, test } from "@playwright/test";
import { detail, formatReport } from "../src/report.js";
import { runGameplaySession, STEP_NAMES, type StepName } from "../src/steps.js";

type Expect = "Pass" | "PassOrSkip" | "Blocked";

const FRONTIER: [StepName, Expect][] = [
  ["page_loads", "Pass"],
  ["wasm_client_starts", "Pass"],
  ["clean_console", "Pass"],
  ["webgpu_adapter", "PassOrSkip"], // an environment fact: headless runners may have no adapter
  ["render_canvas", "Pass"],
  ["websocket_session", "Pass"],
  ["time_display", "Pass"],
  ["map_view_bodies", "Pass"],
  ["time_warp_control", "Pass"],
  ["player_vessel", "Blocked"],
  ["maneuver_node", "Blocked"],
];

test("frontier names every step in order", () => {
  expect(FRONTIER.map(([n]) => n)).toEqual([...STEP_NAMES]);
});

test("a player gets as far as the client allows", async ({ page, baseURL }, testInfo) => {
  const results = await runGameplaySession(page, baseURL!.replace(/\/$/, ""));
  const report = formatReport(testInfo.project.name, results);
  console.log(report);
  await testInfo.attach("gameplay-report", { body: report, contentType: "text/plain" });
  await testInfo.attach("gameplay-results.json", {
    body: JSON.stringify({ browser: testInfo.project.name, results }, null, 2),
    contentType: "application/json",
  });

  const problems: string[] = [];
  const progress: string[] = [];
  results.forEach((r, i) => {
    const [name, want] = FRONTIER[i];
    expect(r.name).toBe(name);
    const o = r.outcome;
    if (o.kind === "FAILED") problems.push(`${name} FAILED: ${o.error}`);
    else if (want === "Pass" && o.kind !== "PASSED") problems.push(`${name} regressed: ${o.kind} ${detail(o)}`);
    else if (want === "PassOrSkip" && o.kind === "BLOCKED") problems.push(`${name} regressed: BLOCKED ${detail(o)}`);
    else if (want === "Blocked" && o.kind === "PASSED") progress.push(`${name} now passes: ${detail(o)}`);
    else if (want === "Blocked" && o.kind === "SKIPPED") problems.push(`${name} skipped instead of blocked: ${detail(o)}`);
  });
  for (const p of progress) console.log(`PROGRESS: ${p} (move the frontier in e2e/tests/gameplay.spec.ts)`);
  expect(problems, `gameplay problems in ${testInfo.project.name}:\n${problems.join("\n")}`).toEqual([]);
});
