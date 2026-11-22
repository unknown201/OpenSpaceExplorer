using System;
using System.Collections;
using System.Globalization;
using System.IO;
using KSP;
using KSP.Game;
using KSP.Sim;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // EVA 1:1 comparison with OSE (thread "Fix EVA in orbit and other situations"). The run-1 craft (Mk1 pod, two
    // tanks, Swivel, chute) in a 100 km circular orbit ("evaorbit") or on the pad ("evapad"). The first Kerbal goes
    // out and holds the hatch ladder 10 s, lets go and waits 5 s, then (orbit) turns the jetpack on and pushes up
    // 1 s, coasts 3 s, forward 1 s, coasts 3 s, or (pad) walks forward 3 s and stands 3 s. Every physics tick
    // logs the Kerbal's centre of mass relative to the pod part, in the pod's own frame (Unity axes), the
    // relative velocity, the locomotion handler, whether it counts as on the ground, its situation and mass
    // (eva-compare.csv). Screenshots at fixed times of each phase.
    public static class EvaCompare
    {
        static GameInstance G => GameManager.Instance.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;

        // Puts the craft `above` m over the terrain at a latitude/longitude, still (the body's own turn).
        static void ToSurface(VesselComponent vessel, double lat, double lon, double above)
        {
            var state = new KSP.Sim.State.SurfaceTeleportState { ReferenceBodyGuid = vessel.mainBody.Name, Altitude = above, Latitude = lat, Longitude = lon, VerticalSpeed = 0 };
            G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject, state);
        }

        // Boards the Kerbal back into the pod; filled in from the decompiled boarding call (see run-eva-compare.ps1).
        static bool TryBoard(VesselComponent kerbal, PartBehavior pod, Action<string> mark)
        {
            return EvaBoard.TryBoard(kerbal, pod, mark);
        }

        // mode: orbit (100 km orbit), pad, flight (2500 m over the land west of the KSC, falling from rest),
        // splash (afloat on the sea east of the KSC), board (pad, back in after 3 s on the ladder)
        public static IEnumerator Run(VesselComponent vessel, string dir, string mode, Action<string> mark)
        {
            bool orbit = mode == "orbit";
            G.ViewController.flightInputHandler.OverrideInputThrottleCutoff(true);
            if (orbit) yield return OrbitScenarios.ToOrbit(vessel, "Kerbin", 700000, 0, mark);
            else yield return new WaitForSeconds(3f);
            if (mode == "splash")
            {
                // the teleport height is over the terrain (the sea floor here): find the floor, then set it down
                // 3 m above the water and let it settle afloat
                double asl0 = G.ViewController.GetActiveSimVessel().AltitudeFromSeaLevel;
                ToSurface(vessel, -0.0972, -73.5, 50);
                // the teleport lands a few frames later: wait until the altitude has moved off the pad
                float waited = 0;
                while (waited < 10f && Math.Abs(G.ViewController.GetActiveSimVessel().AltitudeFromSeaLevel - asl0) < 1.0) { waited += Time.deltaTime; yield return null; }
                yield return new WaitForSeconds(0.5f);
                double asl = G.ViewController.GetActiveSimVessel().AltitudeFromSeaLevel;
                mark("probe asl " + asl.ToString("R", Inv));
                ToSurface(vessel, -0.0972, -73.5, 53 - asl);
                yield return new WaitForSeconds(20f);
                var v = G.ViewController.GetActiveSimVessel();
                mark("afloat? situation " + v.Situation + " asl " + v.AltitudeFromSeaLevel.ToString("R", Inv));
            }
            if (mode == "flight")
            {
                ToSurface(vessel, -0.0972, -74.8, 2500);
                yield return new WaitForSeconds(0.2f);
                var v = G.ViewController.GetActiveSimVessel();
                mark("dropped, situation " + v.Situation + " asl " + v.AltitudeFromSeaLevel.ToString("R", Inv));
            }
            vessel = G.ViewController.GetActiveSimVessel();
            PartComponent podPart = null; Transform pod = null;
            foreach (var part in vessel.SimulationObject.PartOwner.Parts)
                if (part.Name.StartsWith("pod_") && G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) { podPart = part; pod = view.transform; break; }
            mark("pod " + (pod != null) + " situation " + vessel.Situation);
            bool ok = pod != null && EVAUtils.TryStartEVA(pod.GetComponent<PartBehavior>(), 0);
            mark("TryStartEVA " + ok);
            float wait = 0; VesselComponent kerbal = null;
            while (wait < 30) { wait += Time.deltaTime; kerbal = G.ViewController.GetActiveSimVessel(); if (kerbal != null && kerbal.IsKerbalEVA) break; yield return null; }
            if (kerbal == null || !kerbal.IsKerbalEVA) { mark("no kerbal"); yield break; }
            mark("kerbal out, rcs " + kerbal.IsRCSEnabled + " situation " + kerbal.Situation);
            var space = G.UniverseView.PhysicsSpace;
            var frame = G.UniverseModel.inertialReferenceFrame.bodyMotionFrame;
            double ut0 = G.UniverseModel.UniverseTime;
            var w = new StreamWriter(Path.Combine(dir, "out", "eva-compare.csv"), false) { AutoFlush = true };
            w.WriteLine("phase,t,x,y,z,dist,dvx,dvy,dvz,handler,on_ground,situation,mass_t,alt_m,jetpack,rcs");
            string phase = "ladder";
            Func<float> now = () => (float)(G.UniverseModel.UniverseTime - ut0);
            Action row = () =>
            {
                try
                {
                    var k = G.ViewController.GetActiveSimVessel();
                    if (k == null || !k.IsKerbalEVA || pod == null) return;
                    var b = G.ViewController.GetBehaviorIfLoaded(k.SimulationObject.Kerbal);
                    Vector3 kc = space.PositionToPhysics(k.CenterOfMass);
                    Vector3 rel = Quaternion.Inverse(pod.rotation) * (kc - pod.position);
                    var dv = Vector.Reframed(k.OrbitalVelocity, frame.localFrame).vector - Vector.Reframed(vessel.OrbitalVelocity, frame.localFrame).vector;
                    var dvl = Quaternion.Inverse(pod.rotation) * new Vector3((float)dv.x, (float)dv.y, (float)dv.z);
                    w.WriteLine(phase + "," + now().ToString("R", Inv) + "," + rel.x.ToString("R", Inv) + "," + rel.y.ToString("R", Inv) + "," + rel.z.ToString("R", Inv) + ","
                        + rel.magnitude.ToString("R", Inv) + "," + dvl.x.ToString("R", Inv) + "," + dvl.y.ToString("R", Inv) + "," + dvl.z.ToString("R", Inv) + ","
                        + (b != null ? b.CharacterCurrentLocomotionHandlerType.ToString() : "") + "," + (b != null && b.IsOnGround()) + "," + k.Situation + ","
                        + k.totalMass.ToString("R", Inv) + "," + k.AltitudeFromSeaLevel.ToString("R", Inv) + "," + (b != null && b.CharacterState.IsJetpackInUse) + "," + k.IsRCSEnabled);
                }
                catch (Exception e) { mark("row failed " + e.Message); }
            };
            // hold `seconds`, a row every physics tick, screenshots at the given offsets into the phase
            IEnumerator Hold(float seconds, params float[] shots)
            {
                float start = now(); int next = 0;
                while (now() - start < seconds)
                {
                    yield return new WaitForFixedUpdate();
                    row();
                    if (next < shots.Length && now() - start >= shots[next])
                    {
                        ScreenCapture.CaptureScreenshot(Path.Combine(dir, "out", "eva-" + mode + "-" + phase + "-" + shots[next].ToString("0.0", Inv) + "s.png"));
                        mark("shot " + phase + " " + shots[next].ToString("0.0", Inv));
                        next++;
                    }
                }
            }
            if (mode == "board")
            {
                yield return Hold(3f, 1f);
                phase = "board"; mark("board " + TryBoard(G.ViewController.GetActiveSimVessel(), pod.GetComponent<PartBehavior>(), mark));
                float b = now();
                while (now() - b < 10f)
                {
                    yield return new WaitForFixedUpdate();
                    var a = G.ViewController.GetActiveSimVessel();
                    if (a == null || !a.IsKerbalEVA) { mark("aboard after " + (now() - b).ToString("R", Inv) + " s, active " + a?.Name + " crew in pod " + G.SessionManager.KerbalRosterManager.GetAllKerbalsInSimObject(pod.GetComponent<PartBehavior>().Model.GlobalId).Count); break; }
                    row();
                }
                w.Dispose();
                yield break;
            }
            yield return Hold(mode == "flight" ? 5f : 10f, 0.5f, 2f, 5f, 9.5f);
            phase = "letgo"; mark("let go");
            InputHolder.EvaJump = true;
            yield return Hold(0.3f);
            InputHolder.EvaJump = false;
            yield return Hold(5f, 1f, 4.5f);
            if (mode == "flight")
            {
                phase = "fall"; yield return Hold(8f, 2f, 7.5f);
            }
            else if (mode == "splash")
            {
                phase = "float"; yield return Hold(5f, 4.5f);
                phase = "swim"; mark("swim forward 6 s"); InputHolder.EvaFrontBack = 1f;
                yield return Hold(6f, 3f);
                InputHolder.EvaFrontBack = 0f;
                phase = "settle"; yield return Hold(3f, 2.5f);
            }
            else if (orbit)
            {
                kerbal = G.ViewController.GetActiveSimVessel();
                kerbal.IsRCSEnabled = true;
                if (G.ViewController.TryGetActiveVehicle(out var vehicle)) vehicle.ControllableActionGroups?.SetGroup(KSPActionGroup.RCS, true);
                var behaviour = G.ViewController.GetBehaviorIfLoaded(kerbal.SimulationObject.Kerbal);
                if (behaviour != null) { behaviour.Model.KerbalData.JetpackAlignToCameraEnabled = false; if (!behaviour.CharacterState.IsJetpackInUse) behaviour.OnToggleJetpack(); }
                phase = "pack-on"; mark("jetpack on " + (behaviour != null && behaviour.CharacterState.IsJetpackInUse));
                yield return Hold(1f);
                phase = "up"; mark("jetpack up 1 s");
                InputHolder.KerbalMove = new Vector3(0, 1, 0); InputHolder.EvaUpDown = 1f;
                yield return Hold(1f, 0.9f);
                InputHolder.KerbalMove = Vector3.zero; InputHolder.EvaUpDown = 0f;
                phase = "coast1"; yield return Hold(3f, 2.5f);
                phase = "forward"; mark("jetpack forward 1 s");
                InputHolder.KerbalMove = new Vector3(0, 0, 1); InputHolder.EvaFrontBack = 1f;
                yield return Hold(1f);
                InputHolder.KerbalMove = Vector3.zero; InputHolder.EvaFrontBack = 0f;
                phase = "coast2"; yield return Hold(3f, 2.5f);
            }
            else
            {
                phase = "walk"; mark("walk forward 3 s");
                InputHolder.EvaFrontBack = 1f;
                yield return Hold(3f, 2.5f);
                InputHolder.EvaFrontBack = 0f;
                phase = "stand"; yield return Hold(3f, 2.5f);
                // keep walking forward (off the pod's edge if still on it, else on along the ground) with nothing
                // else pressed: does the pack fire, is RCS on for a surface EVA
                phase = "walkoff"; mark("walk forward 6 s, rcs " + G.ViewController.GetActiveSimVessel()?.IsRCSEnabled);
                InputHolder.EvaFrontBack = 1f;
                yield return Hold(6f, 3f);
                InputHolder.EvaFrontBack = 0f;
                phase = "settle"; yield return Hold(3f, 2.5f);
            }
            InputHolder.KerbalMove = null; InputHolder.EvaUpDown = null; InputHolder.EvaFrontBack = null; InputHolder.EvaJump = null;
            w.Dispose();
            mark("eva compare done, kerbal mass " + G.ViewController.GetActiveSimVessel()?.totalMass.ToString("R", Inv));
        }
    }
}
