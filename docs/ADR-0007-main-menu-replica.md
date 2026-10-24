# ADR-0007: The main menu as a replica of the original, screen by screen

Date: 2026-10-24. Status: accepted (Josh: a detailed, faithful 3D replica of KSP2 in the browser, built in
the order a new player meets it, using the original's assets).

## Decision

- The client is built screen by screen after the original, in the order a player meets them: main menu,
  campaign creation, the KSC hub, the VAB, the launch pad, flight and map. Each screen is laid out from the
  original's UI prefab (`docs/03-ui-screens-and-flows.md` names them; `tools/` dumps them with UnityPy) at the
  1920x1080 reference size, scaled with one CSS unit `--u = min(100vh/1080, 100vw/1920)`.
- The main menu backdrop is the original's menu scene, `kspFlow.unity` in `boot-ksp_scenes_all.bundle`:
  a scaled Kerbin (`kerbin_menu_d/_n`, the `GeoSphere1000mSphereMapped` mesh) turning at 1 degree per
  second, two cloud layers over it (`Cloudsphere_03`, a cubemap), an atmosphere shell, the sun and three
  fill lights, a starfield cubemap, and one of three stock vessels (`MM_Vessel_Default`, `_03`, `_05`)
  spinning at 2 degrees per second, picked at random, as the original's `RandomlyEnableChildrenOnStart`
  does. `tools/export_menu_scene.py` writes `assets/web/menu/scene.json` with the camera, lights, placements
  and spin rates, `menu/kerbin.glb`, `menu/clouds.glb` (alpha blended), `menu/sky.glb` (an inverted sphere),
  the two cubemaps unwrapped to panoramas under `textures/`, and the menu's sprites under `ui/`. Vessels are
  lists of part placements; the client draws the same `parts/<id>.glb` files the editor uses, so the
  exporter needs no second copy of the part meshes.
- `ose-client::menu3d` draws that scene with the WebGPU renderer whenever no session is joined. The HTML
  menu over it follows `MainMenu.prefab`: the logo top-left, the item column under it, the account bar
  top-right (here the pilot's name), and the campaign menu as the original's two columns (campaign list on
  the left with "Start a new campaign", details on the right). The e2e hooks (`data-ose="menu"`, `play`,
  `player-name`, `session-join`, ...) are unchanged; `data-ose-menu-vessel` on the GPU canvas names the
  vessel on show once the scene is up.
- Renderer additions for this: a fill light and an ambient level per draw, an atmosphere mode (rim glow
  by view angle, brighter on the day side), and alpha-blended primitives (glTF `alphaMode: BLEND`) drawn
  after the opaque ones without depth writes.
- Winding: the exporter now reverses every triangle. The signed volume of the earlier exports was negative
  (clockwise from outside once X is mirrored), so back-face culling had been showing the far inside of
  every sphere and tank. This re-export is the one the server carries from 2026-10-24.

## Consequences

- The menu needs the exported assets on the server; without them the backdrop stays black and the menu
  still works (the e2e suite on GitHub has no game install).
- Fonts: the original's menu fonts (Sbliquid, Realtime Rounded) are not in the bundles as font files, only
  as TextMeshPro atlases, so the page uses a system sans-serif at the original's sizes.
- Not yet: the sun flare and bloom, the ship's team colours (`ApplyTeamColors` masks), the Settings and
  Extras screens, and the menu's hover and fade animations.
