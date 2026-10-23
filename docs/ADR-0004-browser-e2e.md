# ADR-0004: Browser gameplay tests in Chromium and Firefox

Date: 2026-10-23. Status: accepted (Josh, project thread "Chrome and Firefox gameplay tests").

## Context

`ose-bot` (ADR-0003) plays the game through the protocol, but nobody checks the thing players actually
run: the trunk-built WASM client in a browser. The client is early (a page that loads the module and
sets a status line); the map view, WebSocket streaming and the WebGPU renderer are in progress. We want
a test that loads the real page in the two browsers we care about today, exercises whatever gameplay
exists, keeps running as features land, and tells the difference between "not built yet" and "broken".

## Decision

1. **Playwright, in `e2e/`, pinned.** `@playwright/test` 1.56.1 with its own Chromium (141) and Firefox
   (142) builds. It is a Node package beside the Cargo workspace, not a crate: the browsers, the
   WebSocket inspection and the per-browser projects come for free, and it runs the same on Windows
   and the Linux runner. The client is built with `trunk build --release --dist target/web` and served
   by `ose-server --static-dir`, which is the production shape; Playwright's `webServer` starts the
   server with `cargo run`. `OSE_BASE_URL` points the same tests at a running deployment, including
   https://ose.jfly.dev from the house network.
2. **The session is a fixed chain of named steps** (`e2e/src/steps.ts`), mirroring the bot: page loads,
   WASM starts, clean console, WebGPU adapter, render canvas, WebSocket session, time display, map
   bodies, time warp, player vessel, maneuver node. Each ends `PASSED`, `BLOCKED { feature }`,
   `SKIPPED { reason }` or `FAILED { error }`. Blocked steps detect absence from the page (no
   `<canvas>`, no WebSocket opened within 3 s, no hook element), so when the feature appears the same
   step starts exercising it and either passes or fails for a real reason.
3. **The test encodes a frontier** (`e2e/tests/gameplay.spec.ts`), per browser project: `Pass`,
   `PassOrSkip` or `Blocked` per step. It fails on any `FAILED` outcome and on regressions; a blocked
   step that passes prints `PROGRESS:` and does not fail. Today, in both browsers: 3 pass (page, WASM
   start, clean console), 1 skipped headless (WebGPU adapter), 7 blocked.
4. **The client exposes hooks, not internals.** Steps find features through `data-ose="..."`
   attributes (`ut`, `body` with `data-ose-id`, `warp`, `vessel`, `maneuver-node`) and the existing
   `#status` line. The client thread adds the attribute when it renders the thing; the test never
   reaches into WASM memory or JS globals. The list lives in `e2e/README.md`.
5. **CI runs it on every push** as the `browser-e2e` job in `.github/workflows/ci.yml`: builds the
   client and server, `npx playwright install --with-deps chromium firefox`, runs both projects, and
   uploads the HTML report and traces when something fails. Gameplay tables are in the job log.

## WebGPU in automated browsers (measured 2026-10-23, Windows 11, RTX 5090)

`navigator.gpu` exists only in a secure context: probing it on `about:blank` says undefined in every
browser, so the step runs on the served `http://127.0.0.1` page. With that:

| browser | mode | result |
|---|---|---|
| Chromium headless shell (Playwright default) | headless | `requestAdapter()` null |
| Chromium full build (`channel: "chromium"`) | headless | adapter `nvidia blackwell`, 15 features |
| Chromium full + `--enable-unsafe-webgpu --ignore-gpu-blocklist` | headless | adapter, 21 features (used) |
| Chromium full + `--use-angle=swiftshader` | headless | null (so not used) |
| Chromium full + `--use-angle=d3d11` | headless | adapter, 21 features |
| Firefox 142, default prefs | headless or headed | `navigator.gpu` undefined |
| Firefox 142, `dom.webgpu.enabled` + `gfx.webgpu.ignore-blocklist` | headless | null |
| Firefox 142, same prefs | headed | adapter, 11 features |
| Chromium full, Linux `ubuntu-latest` runner, no GPU, `--enable-features=Vulkan --use-vulkan=swiftshader --use-angle=vulkan --disable-vulkan-surface` | headless | adapter `google swiftshader`, 19 features (CI run 39997611201) |
| Firefox 142, Linux runner, prefs set | headless | `NotSupportedError: WebGPU is only available on Windows, and in Nightly and Early Beta builds on other platforms` |

Consequences: the Chromium project uses the full browser channel and the two flags, plus the Vulkan
over SwiftShader software path on Linux, so CI exercises WebGPU in Chromium without a GPU. The Firefox
project sets the prefs and gets WebGPU only on Windows with `OSE_E2E_HEADED=1` (a window); the
Playwright Firefox build on Linux refuses WebGPU outright until Mozilla ships it there. `webgpu_adapter`
is therefore `PassOrSkip` in the frontier: a missing adapter is an environment fact, not a game failure. When the renderer lands, `render_canvas`
should pass wherever an adapter exists and the client should fall back to a visible "no WebGPU" message
elsewhere, which the step can then check for instead of failing.

## Consequences

- `cd e2e && npm ci && npx playwright install chromium firefox && npm run build-client && npm test`
  is the local loop; about 20 s for both browsers once the server is built.
- A client feature is finished when its hook exists and its step passes in both browsers; the frontier
  moves in the same change.
- The live site can be checked from the house with `OSE_BASE_URL=https://ose.jfly.dev npm test`;
  GitHub's runners cannot reach it (LAN/Tailscale only), so that run is manual.
