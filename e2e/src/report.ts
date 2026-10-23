// Step outcomes and the progress table, the same vocabulary as crates/ose-bot/src/report.rs.

export type Outcome =
  | { kind: "PASSED"; detail: string }
  | { kind: "BLOCKED"; feature: string }
  | { kind: "SKIPPED"; reason: string }
  | { kind: "FAILED"; error: string };

export const passed = (detail: string): Outcome => ({ kind: "PASSED", detail });
export const blocked = (feature: string): Outcome => ({ kind: "BLOCKED", feature });
export const skipped = (reason: string): Outcome => ({ kind: "SKIPPED", reason });
export const failed = (error: unknown): Outcome => ({
  kind: "FAILED",
  error: error instanceof Error ? error.message.split("\n")[0] : String(error),
});

export function detail(o: Outcome): string {
  switch (o.kind) {
    case "PASSED":
      return o.detail;
    case "BLOCKED":
      return o.feature;
    case "SKIPPED":
      return o.reason;
    case "FAILED":
      return o.error;
  }
}

export interface StepResult {
  name: string;
  outcome: Outcome;
  elapsedMs: number;
}

export function formatReport(browser: string, results: StepResult[]): string {
  const lines = [`${"step".padEnd(22)} ${"outcome".padEnd(8)} detail   [${browser}]`];
  for (const r of results) {
    lines.push(`${r.name.padEnd(22)} ${r.outcome.kind.padEnd(8)} ${detail(r.outcome)}`);
  }
  const count = (k: Outcome["kind"]) => results.filter((r) => r.outcome.kind === k).length;
  const furthest = [...results].reverse().find((r) => r.outcome.kind === "PASSED")?.name ?? "nothing";
  lines.push(
    "",
    `${count("PASSED")} passed, ${count("BLOCKED")} blocked, ${count("SKIPPED")} skipped, ${count("FAILED")} failed ` +
      `out of ${results.length}; furthest step passed: ${furthest}`,
  );
  return lines.join("\n");
}
