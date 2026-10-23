// Build the WASM client with trunk into target/web, where playwright.config.ts tells ose-server to serve it
// from. Run from anywhere; paths are resolved from this file.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..", "..");
const clientDir = path.join(root, "crates", "ose-client");
const dist = path.join(root, "target", "web");

const r = spawnSync("trunk", ["build", "--release", "--dist", dist], {
  cwd: clientDir,
  stdio: "inherit",
  shell: process.platform === "win32",
});
if (r.error) {
  console.error(`could not run trunk: ${r.error.message} (cargo install trunk, or see docs/STATUS.md)`);
  process.exit(1);
}
process.exit(r.status ?? 1);
