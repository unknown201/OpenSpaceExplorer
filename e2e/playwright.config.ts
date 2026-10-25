// Browser end-to-end tests for the OpenSpaceExplorer WASM client (docs/ADR-0004-browser-e2e.md).
//
// By default this starts `ose-server --static-dir target/web` from the workspace root (build the client first:
// `npm run build-client`) and drives the real page in Chromium and Firefox. Environment knobs:
//   OSE_BASE_URL=https://ose.jfly.dev   test an already running server instead of spawning one
//   OSE_E2E_PORT=7788                   port for the spawned server
//   OSE_E2E_HEADED=1                    run the browsers with a window (Firefox only exposes WebGPU headed)
import { defineConfig, devices } from "@playwright/test";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const port = Number(process.env.OSE_E2E_PORT ?? 7788);
const external = process.env.OSE_BASE_URL;
const baseURL = external ?? `http://127.0.0.1:${port}`;
const headless = !process.env.OSE_E2E_HEADED;

// WebGPU flags, measured on 2026-10-23 with Playwright 1.56.1 (Chromium 141, Firefox 142), see ADR-0004:
// - the Chromium headless shell never exposes an adapter; the full `chromium` channel does, headless included,
//   on a machine with a GPU. `--use-angle=swiftshader` makes the adapter disappear, so it is not used.
// - on Linux runners without a GPU, Chromium's documented software path is Vulkan over SwiftShader.
// - Firefox 142 needs `dom.webgpu.enabled` and only returns an adapter with a window, not headless.
const chromiumArgs = [
  "--enable-unsafe-webgpu",
  "--ignore-gpu-blocklist",
  // On Windows Playwright's Chromium cannot load dxil.dll ("DynamicLib.Open: dxil.dll Windows Error: 87"), so
  // device creation fails and the client drops to 2D; FXC instead of DXC works (2026-10-25).
  ...(process.platform === "win32" ? ["--disable-dawn-features=use_dxc"] : []),
  ...(process.platform === "linux"
    ? ["--enable-features=Vulkan", "--use-vulkan=swiftshader", "--use-angle=vulkan", "--disable-vulkan-surface"]
    : []),
];

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : [["list"]],
  use: {
    baseURL,
    headless,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        channel: "chromium", // the full browser, not the headless shell: WebGPU needs it
        launchOptions: { args: chromiumArgs },
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
