# UI and interaction parity with KSP2: gap audit

Audited 2026-10-25 against the KSP2 0.2.2 decompile (`reference/ksp2/Assembly-CSharp/`), the default bindings
embedded in `KSP.Input/GameInput.cs` and `docs/03-ui-screens-and-flows.md`, by driving our client (origin/main
c863af4) headless through every screen. Screen and flow inventory lives in doc 03; this file is the
controls-and-feel delta and the order we close it in. Status column: **open**, **done** (with commit), **server**
(needs a protocol change first), **other thread** (owned elsewhere).

## 1. KSP2's default keyboard and mouse bindings

Recovered from the embedded input asset (keyboard and mouse only; gamepad left out).

| Map | Binding |
|---|---|
| Flight | pitch W/S, yaw A/D, roll Q/E; translate J/L, I/K, H/N; precision CapsLock (inputs x0.1); trim Alt+axis; SAS T; RCS R; throttle Shift/Ctrl held (1.0 per second), Z full, X cut; stage Space; gear G; brakes B; lights U; action groups 1..0; map M; focus next/prev `]` `[`; camera mode V; fine camera Alt; mouselook Tab; free camera middle mouse; zoom wheel; camera pan arrows; wheel throttle W/S and steer A/D; interact F, B, Space; vessel labels F4; aero GUI F12; Delete toggles rotation/translation |
| Global | Esc pause menu; F5 quicksave, F9 quickload (hold); warp `.` up, `,` down, `/` stop; F2 hide UI; Shift expands tooltips, right click freezes them; right click opens the part context panel; Enter confirms dialogs; Alt+F8 cheats, Alt+F7 debug, Shift+F10 biome debug |
| VAB | right-drag orbits the camera, wheel zooms (Shift x4 faster), arrows pan, Home resets the camera; left click picks and places; Delete/Backspace deletes; Ctrl+C/V copy/paste; Ctrl+Z/Y undo/redo; W/S, A/D, Q/E rotate the held part; Space resets its rotation; X/Shift+X symmetry; C angle snap; Alt disables stack attach; F frame of reference; Ctrl+F search; Ctrl+A select the main assembly; middle click favourites a part; `\` locks the target; Q/E move the tool up and down |
| Map | right-drag rotates, wheel zooms, Home resets, left click focuses (double click to control), middle mouse free camera, M back to flight |
| R&D | left-drag pans, arrows pan, wheel zooms, Shift expands part tooltips, Left/Right scroll the tree |
| KSC | M opens the Tracking Station map, wheel zooms |
| EVA | W/S forward/back, A/D turn, Q/E strafe, Shift/Ctrl up and down (jetpack), Shift run, Space jump, I/K J/L rotate |
| Audio | Numpad1 mute game, Numpad2 mute music |

Camera rule that runs through every map: **the right button drags the camera, the left button is for picking**.

## 2. Gaps by screen

### Global

| # | KSP2 | Ours today | Status |
|---|---|---|---|
| G1 | Warp `.` `,` `/` everywhere in session; the warp widget stays visible in flight (top left) | `.` `,` `/` work; UT and warp stay up in flight | done |
| G2 | F2 hides all UI (screenshots) | F2 fades the interface | done |
| G3 | Tooltips on every HUD control, naming the key; Shift expands | a few `title` attributes, many controls without | open |
| G4 | Notification toasts (warp limited, out of charge, lost link, vessel lost ...) | one-line note inside the menu card; flight warnings in a small label | open |
| G5 | F5/F9 quicksave/quickload | missing | server |
| G6 | Settings screen: input bindings, audio sliders, UI scale, tooltips on/off, graphics | button disabled | open (client-only settings) |
| G7 | Esc menu: resume, save, load, revert to launch/VAB, recover, KSC, Tracking Station, Mission Control, R&D, VAB, settings, quit | resume, KSC, sessions, sound, leave | other thread (restyle in progress); revert/save/load server |

### Flight

| # | KSP2 | Ours today | Status |
|---|---|---|---|
| F1 | W/S/A/D/Q/E rotate continuously while held; CapsLock precision | W/S/A/D turn the target at 25°/s while held, Caps Lock x0.1; no roll | done (roll needs server) |
| F2 | Shift/Ctrl held ramps the throttle at 100%/s | same | done |
| F3 | T toggles SAS; SAS mode buttons around the navball (stability, prograde, retrograde, normal, anti-normal, radial in/out, target, anti-target, maneuver) | T holds the current direction; SAS buttons with the original's icons beside the navball, lit for the active mode | done (target/maneuver modes: server) |
| F4 | G gear, B brakes, U lights, R RCS, 1..0 action groups; action bar with state lights (gear, lights, brakes, solar, radiators, science, abort) | G, B (held) and an action bar with state lights for gear, brakes, solar, chutes, science | done; lights, RCS, abort, action groups: server |
| F5 | Rovers: W/S wheel throttle, A/D steer, B brakes | same, through `Drive` while landed | done |
| F6 | Staging stack: one card per stage with part icons and fuel bars, Space fires the next | "stage 1 of 3" text and a STAGE button | open |
| F7 | Orbital readout Ap/Pe with time to each; altimeter sea level / terrain toggle; speed orbit / surface / target toggle | Ap/Pe with countdowns, SURFACE/ORBIT speed toggle; no terrain altitude or target speed | done (AGL, target: later) |
| F8 | Right-drag orbits the camera, wheel zooms, V cycles camera modes, Home resets | right-drag (and left) orbits, Home resets; no V camera modes yet | partly done |
| F9 | `]` `[` switch to the next/previous vessel nearby | missing | open |
| F10 | F to board/interact on EVA, EVA from the crew portrait | protocol has GoEva/Board/PlantFlag; no client keys | open |
| F11 | Vessel labels F4 | F4 toggles labels | done |
| F12 | Go button / pre-launch throttle | not needed (no launch sequence) | skip |

### Map view and Tracking Station

| # | KSP2 | Ours today | Status |
|---|---|---|---|
| M1 | Right-drag rotates, Home resets, M back to flight, `]` `[` cycle focus | right-drag (and left) rotates, Home and F frame; no `]` `[` cycling yet | partly done |
| M2 | Click an orbit to add a node; six-handle gizmo dragged in 3D; warp-to and trash on the node | "Add node" button and four sliders | open |
| M3 | Ap/Pe/SOI markers with tooltips (time and altitude) | none | open |
| M4 | Vessel labels drawn once | each vessel label appears twice in the map (screenshot 11) | open (bug) |
| M5 | Tracking Station object list, filters, focus/control/recover | rebuilt by "Build 3D world and menu replica" (unpushed) | other thread |

### VAB (Josh's #6)

| # | KSP2 | Ours today | Status |
|---|---|---|---|
| V1 | Header: vehicle name, New/Save/Load, launch site, Launch with total Δv | header bar: New, undo, redo, name, Δv after saving, Save, Launch, close; no Load or launch site yet | partly done |
| V2 | Toolbar: undo/redo, symmetry flyout, angle snap, frame of reference, CoM/CoT/CoL markers, engineer's report | undo/redo in the header; no symmetry, snap or markers yet | partly done |
| V3 | Right-drag orbits, wheel zooms (Shift faster), arrows pan, Home resets | right-drag orbits, wheel zooms (Shift faster), Home resets; no pan | partly done |
| V4 | Delete key, Ctrl+Z/Y, Ctrl+C/V, WASDQE rotate the held part, Space resets, X symmetry, C snap; grab a placed part and move it with its subtree | Delete/Backspace, Ctrl+Z/Y, Ctrl+F, Ctrl+N, Ctrl+S; grabbing a placed part carries its subtree, dropped nowhere it goes back; no copy, rotate, symmetry or snap yet | partly done |
| V5 | Picker part tooltip (mass, cost, size, stats, description), Shift expands | tooltip with title, subtitle, maker, mass, size, crew, tanks, thrust, Isp, description, research lock | done (no cost in our data) |
| V6 | Staging stack panel with per-stage Δv/TWR/burn time, drag a part between stages | stage cards with icons (last to fire on top), click selects, earlier/later buttons; no per-stage Δv or drag | partly done |
| V7 | Parts manager on right click (part settings) | none | later |

### KSC, R&D, Mission Control, main menu

| # | KSP2 | Ours today | Status |
|---|---|---|---|
| K1 | Building hover tooltip and click, wheel zoom, M opens the Tracking Station map | hover/click, zoom, M, right or left drag | done |
| K2 | Launch-site flyout (Launchpad 1-4, runway) | single Launchpad button | later |
| R1 | R&D tree pans by drag and arrows, zooms with the wheel, part tooltips | fixed layout with page scroll; part names only | open |
| C1 | Mission Control: track a mission, in-flight mission tracker, turn-in button when complete | log and briefing only | open |
| A1 | Main menu Settings, Extras/Credits | disabled | open (Settings via G6) |

## 3. Order of work

1. **Input layer** (`crates/ose-client/src/input.rs`): one KSP2 keymap, held-key state read every frame. Flight rotation held at a rate with CapsLock precision, throttle ramp, T SAS (stability assist holds the current facing), G gear, B brakes, rover W/S/A/D through `Drive`, `/` stop warp, F2 hide UI, F4 labels, `]` `[` vessel cycle, Home camera reset, V camera mode. (F1, F2, F3 part, F4 part, F5, F8, F9, F11, G1, G2, M1)
2. **Right-drag cameras** in flight, map, VAB, KSC; context menu suppressed on the canvas; left stays for picking. (F8, M1, V3, K1)
3. **Flight HUD**: warp/UT widget in flight, SAS mode ring around the navball, action bar with gear/brakes/solar/chutes state, Ap/Pe readout, altitude and speed mode toggles, staging cards, tooltips with keys on every control. (F3, F4, F6, F7, G1, G3)
4. **Toasts** for flight warnings and server refusals. (G4)
5. **VAB** (#6): header and toolbar, keys, part tooltips, staging panel. (V1-V6)
6. **Map**: Ap/Pe markers, click-to-add node, duplicate label fix. (M2-M4)
7. **Settings** screen with the binding list, tooltips toggle, audio. (G6)
8. **R&D** pan/zoom, Mission Control tracking. (R1, C1)

Server asks (send through the coordinator when we get there): roll and free rotation rates, SAS target and
maneuver modes, lights/RCS/custom action groups, quicksave/quickload, revert to launch/VAB.
