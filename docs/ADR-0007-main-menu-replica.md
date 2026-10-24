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

- The KSC hub is the original's campus: `tools/export_ksc.py` reads `KSC.prefab` (`kerbin-ksc_assets_all.bundle`,
  about 40,000 objects) and writes `ksc/ksc.glb`, one glTF where the 1,093 kept LOD0 renderers are nodes
  sharing 174 meshes (the renderer uploads each mesh once and draws it per node), with the lower LODs and the
  thousands of light, sign and tyre-mark props left out, plus a flat ground square where the original has
  procedural terrain. `ksc/scene.json` carries the camera origin and look-at the original starts from and
  the clickable facilities (`KSCTargetableBuilding`) as world-space boxes. `ose-client::ksc3d` orbits that
  camera (drag, wheel), outlines the facility under the pointer with a tooltip, and a click opens it; the
  facility panel follows `KSCMenu.prefab` (VAB, Training Center, R&D Center, Tracking Station, Mission
  Control, Launchpad) 48 px from the left.
- The VAB is the original's assembly hall: `tools/export_vab.py` writes the `Env-Terrestrial` environment of
  `OAB.prefab` to `vab/vab.glb` (376 instances of 114 meshes, the exterior shell, sun rays and light props
  left out) and `vab/scene.json` (the build area centre, the hall light). `ose-client::vab3d` draws the hall
  and the craft from the part GLBs at the build centre, orbits the craft's centre from 20 m (the original's
  `initialCameraPivotDistance`), and a click selects a part. Nodes with a mirroring scale are drawn with a
  front-face-culling pipeline. With WebGPU the parts picker and craft panel float left and right over the
  hall; the 2D side view stays as the fallback and keeps its `data-ose-vab-parts` hook.
- Launch and flight: Launch on the launch pad rolls the craft out to pad 1 (`LaunchSavedOnPad`) and opens the
  flight view (`ose-client::flight3d`); Orbit keeps the old put-in-orbit shortcut. The flight view draws in the
  body's own model frame (the ground holds still): the server's inertial position becomes body-fixed
  latitude and longitude through the body rotation from the embedded system file, the KSC campus is anchored
  on the original's `LaunchPad_spawn_01..04` points (exported with the KSC) at the server's pad sites, and the
  vessel is drawn from its craft's part meshes, pointed by its attitude hold, standing on its lowest point,
  with jettisoned stages hidden. Body spheres are drawn mirrored along Z: their UVs put longitude L at
  azimuth 180 - L (east clockwise from the north pole); mirrored, east, north and up are right-handed, the
  campus fits the four pad sites with a proper rotation (about -5 degrees, 340 m residual, against 1.9 km for
  a mirror), and the KSC sits on its coast with the ocean to the east. The HUD follows the original's layout
  (altimeter top centre, speed, attitude holds and throttle bottom centre, staging bottom left) and its keys
  (Z, X, Shift, Ctrl, Space, W/S, A/D, T, M for the map).
  Protocol 9: the craft is drawn along the server's actual `facing` with its +Z on `roll_ref`, so it turns
  toward a new hold at the simulated rate, and body spin comes from `BodyRotation` (the embedded system file
  is the fallback).
  Protocol 11: each part is bent by the server's `flex` (a rotation about its own origin, then an offset,
  craft frame), blended between the last two snapshots over one 100 ms interval and exaggerated three times
  (`FLEX_EXAGGERATION`) so the wobble reads on screen; the HUD shows `joint_stress` as a green-to-red bar.
  The renderer now uses reversed infinite depth (near 1, far 0, `Greater`), which removed most of the
  z-fighting on the ground seen from altitude.
  Navball: drawn per pixel from the original's own atlas (`Flight-Navball-Texture`: a pitch band across the
  middle and a disk per pole, mapped in `navball.rs`) with its prograde, normal and radial markers; surface
  speed below 36 km, orbital above, as the original switches. Engine flames: a procedural cone
  (`tools/make_fx.py`, `fx/flame.glb`) under each lit engine's nozzle, scaled by throttle.
- Craft poses: KSP2 assemblies store `partState.localPosition` relative to the craft root, not the parent
  (the stock K1 descends monotonically down its 38-part tree), so the client reads stored poses as
  craft-frame poses and the editor now stores root-relative positions too. Crafts saved by the editor
  before 2026-10-24 were parent-relative and draw stacked too closely.

## Consequences

- The menu needs the exported assets on the server; without them the backdrop stays black and the menu
  still works (the e2e suite on GitHub has no game install).
- Fonts: the original's menu fonts (Sbliquid, Realtime Rounded) are not in the bundles as font files, only
  as TextMeshPro atlases, so the page uses a system sans-serif at the original's sizes.
- Not yet at the KSC: the layered "Standard Tileable (Vertex Color)" building shader (only each material's base map is drawn, so buildings read darker than the original), terrain, trees and props, the day cycle, the launch pad flyout.
- Not yet in flight: the original's particle plumes, the maneuver and target markers on the navball, debris vessels drawn as parts, terrain relief under the campus, the original's launch camera.
- Not yet: the sun flare and bloom, the ship's team colours (`ApplyTeamColors` masks), the Settings and
  Extras screens, and the menu's hover and fade animations.
