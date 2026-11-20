using System;
using System.Collections;
using System.Globalization;
using System.IO;
using System.Reflection;
using KSP.Game;
using KSP.Rendering.impl;
using KSP.Sim;
using KSP.Sim.impl;
using UnityEngine;
using UnityEngine.InputSystem;
using UnityEngine.InputSystem.LowLevel;

namespace OpenSpaceExplorer.Validation
{
    // T27/T28 flight camera orbit and zoom (docs/ksp2-side-by-side-gaps.md) on the pad, with the craft as launched:
    //   tweakables   every flight camera context's live CameraTweakables (sensitivities, limits)
    //   orbit        mouse look on, 10 px of mouse delta a frame for 90 frames, then fine movement (the
    //                CameraFineMovement modifier) for 60 frames
    //   zoom         mouse look off, one wheel notch in every 15 frames, 5 notches, then 3 with fine movement,
    //                then 5 notches out
    // Mouse input goes through the Input System's own mouse (queued state events), so it travels KSP2's whole
    // path: the camera actions, Mouse.SystemDelta, CameraZoom's scroll processor and the rig. Every frame logs
    // the gimbal's heading, pitch and distance, the frame's unscaled delta time, the delta and zoom KSP2 read.
    public static class CameraScenario
    {
        static GameInstance G => GameManager.Instance?.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        static string F(double x) => x.ToString("R", Inv);

        public static IEnumerator Run(string dir, Action<string> mark)
        {
            var cam = G.CameraManager;
            using (var w = new StreamWriter(Path.Combine(dir, "out", "camera-tweakables.csv"), false))
            {
                w.WriteLine("context,field,value");
                foreach (FlightCameraContext ctx in Enum.GetValues(typeof(FlightCameraContext)))
                {
                    CameraTweakables t = null;
                    try { t = cam.GetCameraTweakables(ctx); } catch (Exception e) { mark("camera: no tweakables for " + ctx + ": " + e.Message); }
                    if (t != null) Dump(w, ctx.ToString(), t);
                }
                var active = (cam.FlightCamera as BasicCameraRig)?.Tweakables;
                if (active != null) Dump(w, "active", active);
            }
            // the system mouse, not the VirtualMouse KSP2 adds for gamepads (Mouse.current once it exists):
            // Mouse.SystemDelta reads the real one
            UnityEngine.InputSystem.Mouse mouse = null;
            foreach (var d in InputSystem.devices)
                if (d is UnityEngine.InputSystem.Mouse m && m.name != "VirtualMouse") { mouse = m; break; }
            if (mouse == null) { mark("camera: no system mouse"); yield break; }
            mark("camera: mouse " + mouse.name + ", current " + UnityEngine.InputSystem.Mouse.current?.name);
            var centre = new Vector2(Screen.width * 0.5f, Screen.height * 0.5f);
            using (var w = new StreamWriter(Path.Combine(dir, "out", "camera-trace.csv"), false))
            {
                w.WriteLine("frame,phase,unscaled_dt,sent_dx,sent_scroll,system_dx,zoom_value,fine,heading,pitch,distance,mode");
                int frame = 0;
                string phase = "idle";
                float dx = 0, scroll = 0;
                void Log()
                {
                    var sol = cam.FlightCamera?.ActiveSolution as BasicCameraRigSolution;
                    var g = sol != null ? sol.GimbalState : default;
                    w.WriteLine(string.Join(",", new[] { frame.ToString(Inv), phase, F(Time.unscaledDeltaTime), F(dx), F(scroll),
                        F(global::Mouse.SystemDelta.x), F(cam.CameraZoomValue.y), cam.CameraFineMovementEnabled ? "1" : "0",
                        F(g.heading), F(g.pitch), F(g.distance), cam.FlightCamera?.Mode.ToString() ?? "" }));
                }
                IEnumerator Frames(int n, float sendDx, float sendScroll, int every, bool fine)
                {
                    for (int k = 0; k < n; k++)
                    {
                        dx = sendDx;
                        scroll = every > 0 && k % every == 0 ? sendScroll : 0f;
                        InputSystem.QueueStateEvent(mouse, new MouseState { position = centre, delta = new Vector2(dx, 0f), scroll = new Vector2(0f, scroll) });
                        cam.CameraFineMovementEnabled = fine;
                        yield return null;
                        frame++;
                        Log();
                    }
                }
                for (int k = 0; k < 30; k++) { yield return null; frame++; Log(); }
                CameraMouseLook.SetMouseLook(true);
                phase = "orbit"; mark("camera orbit");
                yield return Frames(90, 10f, 0f, 0, false);
                phase = "orbit-fine"; mark("camera orbit fine");
                yield return Frames(60, 10f, 0f, 0, true);
                phase = "rest";
                yield return Frames(20, 0f, 0f, 0, false);
                CameraMouseLook.SetMouseLook(false);
                phase = "zoom-in"; mark("camera zoom in");
                yield return Frames(75, 0f, 120f, 15, false);
                phase = "zoom-fine"; mark("camera zoom fine");
                yield return Frames(45, 0f, 120f, 15, true);
                phase = "zoom-out"; mark("camera zoom out");
                yield return Frames(75, 0f, -120f, 15, false);
                phase = "rest";
                yield return Frames(30, 0f, 0f, 0, false);
                cam.CameraFineMovementEnabled = false;
            }
        }

        static void Dump(StreamWriter w, string ctx, CameraTweakables t)
        {
            foreach (var f in typeof(CameraTweakables).GetFields(BindingFlags.Public | BindingFlags.Instance))
            {
                var v = f.GetValue(t);
                string s = v is float x ? F(x) : v is double d ? F(d) : v is Array a ? string.Join(" ", (object[])(a as object[] ?? new object[0])) : v?.ToString() ?? "";
                w.WriteLine(ctx + "," + f.Name + "," + s.Replace(",", ";"));
            }
        }
    }
}
