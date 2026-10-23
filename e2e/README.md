# Browser end-to-end tests

Playwright drives the real WASM client in Chromium and Firefox against `ose-server` and walks a fixed
chain of player steps (`src/steps.ts`). Each step ends `PASSED`, `BLOCKED` (the client lacks the
feature, named with its roadmap phase), `SKIPPED` (this browser or machine cannot do it, e.g. no WebGPU
adapter) or `FAILED` (a bug). `tests/gameplay.spec.ts` holds the frontier: what must pass, what is
expected to be blocked. See `docs/ADR-0004-browser-e2e.md`.

```
cd e2e
npm ci                                   # once; pinned @playwright/test
npx playwright install chromium firefox  # once; the browsers Playwright drives
npm run build-client                     # trunk build --release of crates/ose-client into target/web
npm test                                 # spawns ose-server on 127.0.0.1:7788 and runs both browsers
npm run test:firefox                     # one browser
OSE_BASE_URL=https://ose.jfly.dev npm test   # against the live site (house network / Tailscale)
OSE_E2E_HEADED=1 npm test                # with browser windows; Firefox only has WebGPU this way
```

The gameplay table for each browser is printed in the run and attached to the test as
`gameplay-report` and `gameplay-results.json` (`npx playwright show-report` after a CI run).

## Client hooks

The steps find features through a small set of attributes the client is asked to render. A new feature
is "done" for the test when its hook exists and behaves; move its line in the frontier from `Blocked`
to `Pass` in the same change.

| hook | meaning |
|---|---|
| `#status` | text set by the WASM start function once the module runs |
| `canvas` | the render surface |
| `[data-ose="ut"]` | universal time readout, updated from server `TimeState` |
| `[data-ose="body"]` with `data-ose-id` | one element per celestial body in the map view |
| `[data-ose="warp"]` | the time-warp control; a click raises the warp factor |
| `[data-ose="vessel"]` | the player's active vessel in the map view |
| `[data-ose="maneuver-node"]` | a maneuver node on the player's trajectory |
| `[data-ose="warp-down"]` | lowers the warp factor one step |
| `canvas[data-ose-scale]` | metres per CSS pixel of the map camera, refreshed every frame |
| `canvas[data-ose-focus]` | id and star-space x y of the focused body, refreshed every frame |
