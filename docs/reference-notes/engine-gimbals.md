# Engine gimbals and flight pivot

Investigated against the installed KSP2 `KSP.Modules/Module_Gimbal.cs` and `Data_Gimbal.cs`.
KSP2 preserves each named gimbal transform's initial rotation, maps pitch/yaw/roll relative to
the vessel centre of mass, limits the angles, and smooths actuation using `gimbalResponseSpeed`.
It rotates procedural transforms or drives an animator. Thrust transforms and exhaust follow
the moving hierarchy. The vessel itself rotates about its mass centre.

OSE's physics already resolves bounded gimbal side forces, including reduced axial force to
preserve delivered thrust magnitude. Its renderer instead anchored the lowest mesh point to
the vessel position, although the surface solver treats that position as the wet mass centre.
This produced the apparent pivot at the engine bottoms.

Protocol 40 carries the current mass centre and each firing gimbal's delivered thrust direction.
The client subtracts that mass centre from every part and attached effect, and applies a separate,
response-smoothed turn to each engine's authored bell pivots. Meshes, skins, attached effect
layers and ground-blast rays follow the same turn. Fixed mounts stay in their rest pose.
Fuel burn and separation update the pivot from the simulation's mass distribution.

Animated engines with childless proxy gimbals use their outer bell bones. This provides procedural
bell steering; the original animator's linked piston animations and asymmetric per-axis limiter
controls are not reproduced. Deflection comes from delivered forces, so unpowered engines return
to neutral rather than moving with raw keyboard input. Visual response smoothing slightly lags
the authoritative force. Existing simulation gimbal limits and thrust application approximations
are unchanged.

The exporters preserve controller metadata in GLB extras and record exact gimbal ancestry on
effect layers, including repeated names on clustered engines. Upgrade existing local assets with:

```powershell
.venv/Scripts/python.exe -B tools/gimbal_metadata.py
```

Normal `export_web_assets.py` and `export_particles.py` exports also produce this metadata.
Rebuild and serve client/server together because the wire protocol changed. Updated part GLBs
and engine effect JSON must accompany the client when deploying.

Verification: client unit tests cover mass-centre placement, mount-preserving nozzle rotation,
pause, neutral return and frame-independent response; the simulation regression checks that
the published direction respects the delivered gimbal cone on a final partial burn.
`e2e/scripts/check-rig-animation.mjs` additionally checks three poses and per-instance independence
for Swivel, Poodle and Mammoth through the production WebGPU rig path.
