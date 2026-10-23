// Browser end-to-end tests for the OpenSpaceExplorer WASM client (docs/ADR-0004-browser-e2e.md).
//
// By default this starts `ose-server --static-dir target/web` from the workspace root (build the client first:
// `npm run build-client`) and drives the real page in Chromium and Firefox. Set OSE_BASE_URL to test an
// already running server instead, e.g. OSE_BASE_URL=https://ose.jfly.dev npm test (that site is reachable
// from the house network and Tailscale only).
import { defineConfig, devices } from "@playwright/test";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const port = Number(process.env.OSE_E2E_PORT ?? 7788);
const external = process.env.OSE_BASE_URL;
const baseURL = external ?? `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : [["list"]],
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        // The full Chromium build, not the headless shell: WebGPU needs the real browser.
        channel: "chromium",
        launchOptions: {
          args: [
            "--enable-unsafe-webgpu",
            "--enable-features=Vulkan,UseSkiaRenderer",
            "--ignore-gpu-blocklist",
            "--use-angle=swiftshader", // software fallback so CI runners without a GPU still get an adapter
          ],
        },
      },
    },
    {
      name: "firefox",
      use: {
        ...devices["Desktop Firefox"],
        launchOptions: {
          firefoxUserPrefs: {
            "dom.webgpu.enabled": true,
            "gfx.webgpu.ignore-blocklist": true,
            "gfx.webrender.all": true,
          },
        },
      },
    },
  ],
  webServer: external
    ? undefined
    : {
        command: `cargo run -q -p ose-server -- --listen 127.0.0.1:${port} --static-dir target/web`,
        cwd: root,
        url: `${baseURL}/health`,
        reuseExistingServer: !process.env.CI,
        timeout: 600_000, // a cold `cargo run` compiles the server first
        stdout: "ignore",
        stderr: "pipe",
      },
});
