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
| `[data-ose="menu"]` | the main menu shown at start; `[data-ose="player-name"]` and `[data-ose="play"]` connect, then `[data-ose="session-list"]` lists sessions, each `[data-ose="session"]` with a `[data-ose="session-join"]` button; `[data-ose="create-session"]` holds the create form (`create-name`, `create-mode`, `create-preset`, `difficulty`, `create`), `[data-ose="agency"]` the flag and colours; `[data-ose="resume"]` and `[data-ose="leave"]` on the paused screen |
| `[data-ose="player"]`, `[data-ose="campaign"]`, `[data-ose="warp-holder"]` | header readouts: who you are, the campaign and its members, who holds warp |
| `[data-ose="breadcrumbs"]` | star > body > vessel for what the camera looks at; each crumb is clickable |
| `canvas` | the render surface: `[data-ose="gpu"]` carries `data-ose-renderer="webgpu"` once the wgpu renderer is up; the 2D `#map` canvas is the fallback |
| `[data-ose="gpu"][data-ose-vab-scene]` | set once the VAB hall (the original's OAB interior) is drawn with the craft; the 2D `vab-canvas` is hidden then but still reports `data-ose-vab-parts` |
| `[data-ose="gpu"][data-ose-ksc-scene]` | set once the KSC campus (the original's `KSC.prefab`, ADR-0007) is drawn behind the hub panel; `[data-ose="ksc-tooltip"]` names the facility under the pointer (`data-ose-facility`), a click on it opens that facility |
| `[data-ose="gpu"][data-ose-menu-vessel]` | set once the main menu's 3D backdrop (the original's menu scene, ADR-0007) is drawn; the value names the vessel on show. Absent when the server has no exported assets |
| `[data-ose="ut"]` | universal time readout, updated from server `TimeState` |
| `[data-ose="body"]` with `data-ose-id` | one element per celestial body in the map view |
| `[data-ose="warp"]` | the time-warp control; a click raises the warp factor |
| `[data-ose="vessel"]` | the player's active vessel in the map view |
| `[data-ose="ksc"]` | the Kerbal Space Center hub shown after joining; buttons `ksc-launchpad`, `ksc-tracking` (map), `ksc-vab`, `ksc-rnd`, `ksc-mission`, `ksc-training` (disabled until their phase); `[data-ose="ksc-button"]` in the HUD reopens it |
| `[data-ose="launchpad"]` | the launch pad: `[data-ose="craft-list"]` with one `[data-ose="craft"]` per library entry and a `[data-ose="craft-launch"]` button; `[data-ose="launch-altitude"]` in km |
| `[data-ose="vab"]` | the Vehicle Assembly Building: `vab-categories` (buttons with `data-ose-category`), `vab-search`, `vab-part` rows (`data-ose-part`), `vab-canvas` (`data-ose-vab-parts` = parts drawn), `vab-name`, `vab-remove`, `vab-clear`, `vab-stage-up/down`, `vab-save`, `vab-launch`, `vab-report` (server engineer report), `vab-back`; the launch pad's `craft-edit` opens a library craft in it |
| `[data-ose="flight"]` | the flight HUD, class `open` after `craft-launch` rolls a craft out to the pad: `data-ose-vessel`, `data-ose-altitude` (m), `data-ose-situation`, `data-ose-stress` (worst joint load over its limit), `data-ose-flex` (parts bending); `[data-ose="flight-navball"]` gets `data-ose-navball="drawn"` once the navball renders (WebGPU only); `flight-throttle-full`, `flight-throttle-cut`, `flight-stage` buttons (keys Z, X, Space). `craft-orbit` on the launch pad puts a craft straight into orbit instead |
| `[data-ose="audio"]` | hidden sound state: `data-ose-music` (the scene's music key: `music_menu`, `music_ksc`, `music_vab`, `music_kerbin`, `music_mun`), `data-ose-playing` (the track playing once audio is unlocked by a gesture), `data-ose-sfx` and `data-ose-sfx-count` (last one-shot), `data-ose-engine` (engine loop level, 0 silent), `data-ose-muted`; `[data-ose="sound-toggle"]` mutes |
| `[data-ose="mission"]` | Mission Control (`ksc-mission` opens it, `mc-back` returns): `mc-tab-primary`/`mc-tab-secondary`, `mc-mission` rows (`data-ose-mission`, class `active`/`complete`), `mc-briefing` |
| `[data-ose="rnd"]` | the R&D Center (`ksc-rnd` opens it, `rnd-back` returns): `rd-node` buttons (`data-ose-node`, class `researched`, `available` or `locked`), `rd-info` panel for the selected node with `rd-research` (UnlockTechNode) and `rd-note`, `rd-summary` |
| `[data-ose="tracking"]` | the Tracking Station over the map (`ksc-tracking` or the Esc menu's `to-tracking` opens it, `ts-back` returns): `ts-vessel` rows (`data-ose-id`), `ts-tab-all` / `ts-tab-mine`, `ts-info` card for the selected vessel with `ts-focus`, `ts-fly` (yours) and `ts-recover` (yours, landed on Kerbin) |
| `[data-ose="add-node"]` | adds a maneuver node on the player's vessel (sends `ScheduleBurn`) |
| `[data-ose="maneuver-node"]` | the maneuver node on the player's trajectory; `data-ose-ut` is the burn time, `data-ose-prograde` the delta-v |
| `[data-ose="clear-node"]` | removes the node (`ClearBurn`) |
| `[data-ose="warp-down"]` | lowers the warp factor one step |
| `[data-ose="vessel"]` also carries `data-ose-id` (vessel id); the player's own vessel has class `mine` and the camera follows it; a click follows that vessel |
| `canvas[data-ose-scale]` | metres per CSS pixel of the map camera, refreshed every frame |
| `canvas[data-ose-focus]` | id and star-space x y of the focused body, refreshed every frame |
