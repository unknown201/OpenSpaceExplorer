using System;
using System.Collections;
using System.Globalization;
using System.IO;
using KSP;
using KSP.Game;
using KSP.Sim;
using KSP.Sim.impl;
using KSP.Sim.State;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // Orbital reference runs (docs/ksp2-side-by-side-gaps.md T8, T9, T21). Each starts from the orbital VAB spawn,
    // then teleports the craft into a known orbit so both games start from the same elements.
    public static class OrbitScenarios
    {
        static GameInstance G => GameManager.Instance.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;

        static IEnumerator ToOrbit(VesselComponent vessel, string body, double sma, double ecc, Action<string> mark)
        {
            var state = new KeplerOrbitState
            {
                referenceBodyGuid = body, semiMajorAxis = sma, eccentricity = ecc, inclination = 0, longitudeOfAscendingNode = 0,
                argumentOfPeriapsis = 0, meanAnomalyAtEpoch = 0, epoch = G.UniverseModel.UniverseTime,
            };
            G.SpaceSimulation.TeleportSimObjectToOrbit(vessel.SimulationObject, state, true);
            mark("teleported to " + body + " sma " + sma.ToString("F0", Inv) + " ecc " + ecc.ToString("F4", Inv));
            yield return new WaitForSeconds(8f);
        }

        static string Row(VesselComponent v, float t, string phase)
        {
            var o = v.Orbit;
            double[] x = { t, G.UniverseModel.UniverseTime, o.ApoapsisArl, o.PeriapsisArl, o.semiMajorAxis, o.eccentricity, v.AltitudeFromSeaLevel, v.OrbitalSpeed,
                v.totalMass, G.ViewController.TimeWarp.CurrentRate };
            return phase + "," + v.mainBody.Name + "," + string.Join(",", Array.ConvertAll(x, d => d.ToString("R", Inv)));
        }

        const string Header = "phase,body,t,ut,ap_m,pe_m,sma_m,ecc,alt_m,orbital_speed,mass_t,warp";

        // T8: 100 km circular Kerbin orbit, 60 s real at x100 then 30 s at x1000 (one row per real second), then a
        // Mun-centric orbit (pe 100 km, ap beyond the SOI) warped at x100 until the SOI changes.
        public static IEnumerator Warp(VesselComponent vessel, string dir, Action<string> mark)
        {
            yield return ToOrbit(vessel, "Kerbin", 700000, 0, mark);
            using (var w = new StreamWriter(Path.Combine(dir, "out", "warp-trace.csv"), false) { AutoFlush = true })
            {
                w.WriteLine(Header);
                float t = 0;
                var tw = G.ViewController.TimeWarp;
                foreach (var (phase, index, seconds) in new[] { ("x1", 0, 10f), ("x100", 5, 60f), ("x1000", 6, 30f), ("x1-after", 0, 10f) })
                {
                    tw.SetRateIndex(index, true);
                    mark(phase + " rate " + tw.CurrentRate.ToString(Inv));
                    float until = t + seconds;
                    while (t < until) { yield return new WaitForSecondsRealtime(1f); t += 1f; w.WriteLine(Row(G.ViewController.GetActiveSimVessel(), t, phase)); }
                }
                tw.StopTimeWarp(true);
                vessel = G.ViewController.GetActiveSimVessel();
                yield return ToOrbit(vessel, "Mun", 1650000, 0.8181818181818182, mark);
                w.WriteLine(Row(G.ViewController.GetActiveSimVessel(), t, "mun-start"));
                tw.SetRateIndex(5, true);
                string body = "Mun";
                float limit = t + 600;
                while (t < limit)
                {
                    yield return new WaitForSecondsRealtime(0.2f); t += 0.2f;
                    var v = G.ViewController.GetActiveSimVessel();
                    if (v == null) break;
                    w.WriteLine(Row(v, t, "mun-x100"));
                    if (v.mainBody.Name != body) { mark("SOI change " + body + " -> " + v.mainBody.Name + " at UT " + G.UniverseModel.UniverseTime.ToString("R", Inv)); body = v.mainBody.Name; break; }
                }
                tw.StopTimeWarp(true);
                for (int i = 0; i < 10; i++) { yield return new WaitForSecondsRealtime(1f); t += 1f; w.WriteLine(Row(G.ViewController.GetActiveSimVessel(), t, "after-soi")); }
            }
        }

        // T9: run-1 craft in a 100 km circular orbit, SAS prograde, full throttle until the orbital speed has grown by
        // 100 m/s, then cut. KSP2's node UI is not driven: this is the burn the node would ask for, flown by speed.
        public static IEnumerator Burn(VesselComponent vessel, string dir, Action<string> mark)
        {
            yield return ToOrbit(vessel, "Kerbin", 700000, 0, mark);
            vessel = G.ViewController.GetActiveSimVessel();
            vessel.SetAutopilotEnableDisable(true);
            vessel.SetAutopilotMode(AutopilotMode.Prograde);
            yield return new WaitForSeconds(15f);
            using (var w = new StreamWriter(Path.Combine(dir, "out", "burn-trace.csv"), false) { AutoFlush = true })
            {
                w.WriteLine(Header);
                float t = 0;
                for (int i = 0; i < 5; i++) { yield return new WaitForFixedUpdate(); t += Time.fixedDeltaTime; w.WriteLine(Row(vessel, t, "before")); }
                double v0 = vessel.OrbitalSpeed, m0 = vessel.totalMass, ut0 = G.UniverseModel.UniverseTime;
                mark("burn start UT " + ut0.ToString("R", Inv) + " speed " + v0.ToString("R", Inv) + " mass " + m0.ToString("R", Inv));
                G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
                vessel.ActivateNextStage();
                while (vessel.OrbitalSpeed < v0 + 100 && t < 120) { yield return new WaitForFixedUpdate(); t += Time.fixedDeltaTime; w.WriteLine(Row(vessel, t, "burn")); }
                G.ViewController.flightInputHandler.OverrideInputThrottleCutoff(true);
                mark("burn end UT " + G.UniverseModel.UniverseTime.ToString("R", Inv) + " speed " + vessel.OrbitalSpeed.ToString("R", Inv) + " mass " + vessel.totalMass.ToString("R", Inv)
                    + " fuel used t " + (m0 - vessel.totalMass).ToString("R", Inv));
                for (int i = 0; i < 300; i++) { yield return new WaitForFixedUpdate(); t += Time.fixedDeltaTime; if (i % 10 == 0) w.WriteLine(Row(vessel, t, "after")); }
                mark("after: ap " + vessel.Orbit.ApoapsisArl.ToString("R", Inv) + " pe " + vessel.Orbit.PeriapsisArl.ToString("R", Inv));
            }
        }

        // T21: in a 100 km orbit the first Kerbal goes out, then jetpack up (EVA up/down axis) 2 s, coast 2 s, forward
        // 2 s, coast 2 s. Velocity and mass (EVA propellant) every physics tick come from the normal trace.
        public static IEnumerator Jetpack(VesselComponent vessel, string dir, Action<string> mark, Action<VesselComponent, string> begin, Action stop)
        {
            yield return ToOrbit(vessel, "Kerbin", 700000, 0, mark);
            vessel = G.ViewController.GetActiveSimVessel();
            PartBehavior pod = null;
            foreach (var part in vessel.SimulationObject.PartOwner.Parts)
                if (part.Name.StartsWith("pod_") && G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) { pod = view.GetComponent<PartBehavior>(); break; }
            mark("TryStartEVA " + (pod != null && EVAUtils.TryStartEVA(pod, 0)));
            float t = 0; VesselComponent kerbal = null;
            while (t < 30) { t += Time.deltaTime; kerbal = G.ViewController.GetActiveSimVessel(); if (kerbal != null && kerbal.IsKerbalEVA) break; yield return null; }
            yield return new WaitForSeconds(6f);
            mark("kerbal " + (kerbal != null && kerbal.IsKerbalEVA) + " mass " + kerbal?.totalMass.ToString("R", Inv));
            InputHolder.EvaJump = true; yield return new WaitForSeconds(0.3f); InputHolder.EvaJump = false; // let go if holding the ladder
            yield return new WaitForSeconds(3f);
            begin(kerbal, "jetpack-trace.csv");
            yield return new WaitForSeconds(1f);
            var h = G.ViewController.evaInputHandler;
            var upDown = typeof(EVAInputHandler).GetField("_moveUpDown", System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);
            float held = 0;
            mark("jetpack up 2 s");
            while (held < 2f) { upDown?.SetValue(h, 1f); held += Time.deltaTime; yield return null; }
            upDown?.SetValue(h, 0f);
            yield return new WaitForSeconds(2f);
            mark("jetpack forward 2 s");
            InputHolder.EvaFrontBack = 1f; yield return new WaitForSeconds(2f); InputHolder.EvaFrontBack = 0f;
            yield return new WaitForSeconds(2f);
            stop();
            mark("kerbal mass after " + G.ViewController.GetActiveSimVessel()?.totalMass.ToString("R", Inv));
        }
    }
}
