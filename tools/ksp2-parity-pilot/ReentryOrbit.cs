using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using KSP.Game;
using KSP.Sim;
using KSP.Sim.impl;
using KSP.Sim.State;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // T11 orbital reentry (docs/ksp2-side-by-side-gaps.md): the craft on an equatorial Kerbin orbit of 80 x 30 km,
    // put 74 km up on the way down to its periapsis (about 2,260 m/s, 3 degrees below the horizon), SAS holding
    // retrograde. Every tick until it is down or gone: the normal trace (speed against altitude), and each part's
    // temperature, re-entry flux and ablator (reentryorbit-parts.csv); every change in the part list, with the
    // parts that went (reentryorbit-events.csv).
    public static class ReentryOrbit
    {
        static GameInstance G => GameManager.Instance.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;

        // the craft file: "reentry" (pod over heat shield), "pod" (the bare pod), "stack" (pod, six tanks, Swivel)
        public static string Craft(string name) => name == "reentry" ? "craft-reentry.json" : name == "stack" ? "craft-stack.json" : "craft-drop-" + name + ".json";

        // the thermal constants KSP2 runs with (PhysicsSettings, after its settings file)
        public static string ThermalSettings()
        {
            string F(double x) => x.ToString("R", Inv);
            return "thermal settings: background epsilon " + F(PhysicsSettings.GLOBAL_BACKGROUND_PART_TEMPERATURE_EPSILON)
                + " occlude area " + F(PhysicsSettings.OccludeConvectionAreaMultiplier);
        }

        public const double ApoapsisRadius = 680000, PeriapsisRadius = 630000, StartRadius = 674000;

        // mean anomaly (radians) of the point at `r` on the way down to periapsis
        static double MeanAnomalyDescending(double a, double e, double r)
        {
            double p = a * (1 - e * e);
            double nu = 2 * Math.PI - Math.Acos(Math.Max(-1, Math.Min(1, (p / r - 1) / e)));
            double E = 2 * Math.Atan2(Math.Sqrt(1 - e) * Math.Sin(nu / 2), Math.Sqrt(1 + e) * Math.Cos(nu / 2));
            double M = E - e * Math.Sin(E);
            return M < 0 ? M + 2 * Math.PI : M;
        }

        // free: SAS goes off once the craft is in the air (below 70.5 km), so it turns as the airflow turns it
        public static IEnumerator Run(VesselComponent vessel, string dir, Action<string> mark, Action<VesselComponent, string> begin, Action stop, bool free = false)
        {
            double a = (ApoapsisRadius + PeriapsisRadius) / 2, e = (ApoapsisRadius - PeriapsisRadius) / (ApoapsisRadius + PeriapsisRadius);
            var state = new KeplerOrbitState
            {
                referenceBodyGuid = "Kerbin", semiMajorAxis = a, eccentricity = e, inclination = 0, longitudeOfAscendingNode = 0,
                argumentOfPeriapsis = 0, meanAnomalyAtEpoch = MeanAnomalyDescending(a, e, StartRadius), epoch = G.UniverseModel.UniverseTime,
            };
            G.SpaceSimulation.TeleportSimObjectToOrbit(vessel.SimulationObject, state, true);
            yield return new WaitForFixedUpdate();
            vessel = G.ViewController.GetActiveSimVessel();
            mark("teleported: sma " + a.ToString("F0", Inv) + " ecc " + e.ToString("F6", Inv) + " alt " + vessel.AltitudeFromSeaLevel.ToString("F0", Inv)
                + " vspeed " + vessel.VerticalSrfSpeed.ToString("F1", Inv) + " orbital speed " + vessel.OrbitalSpeed.ToString("F1", Inv)
                + " pe " + vessel.Orbit.PeriapsisArl.ToString("F0", Inv));
            vessel.SetAutopilotEnableDisable(true);
            vessel.SetAutopilotMode(AutopilotMode.Retrograde);
            mark("SAS retrograde");
            begin(vessel, "reentryorbit-trace.csv");
            string id = vessel.GlobalId.ToString();
            var db = G.ResourceDefinitionDatabase;
            double ut0 = G.UniverseModel.UniverseTime;
            using (var w = new StreamWriter(Path.Combine(dir, "out", "reentryorbit-parts.csv"), false))
            using (var ev = new StreamWriter(Path.Combine(dir, "out", "reentryorbit-events.csv"), false) { AutoFlush = true })
            {
                w.WriteLine("t,alt_m,srf_speed,mach,density,part,index,temp_k,max_temp_k,reentry_flux,ablator_units,exposed,env_flux,atm_flux,local_flux,surface_flux,thermal_mass,wetted_m2");
                mark("PhysicsSettings occlude area " + PhysicsSettings.OccludeConvectionAreaMultiplier.ToString("R", Inv) + " temp " + PhysicsSettings.OccludeConvectionTemperatureMultiplier.ToString("R", Inv)
                    + " coeff " + PhysicsSettings.OccludeConvectionCoefficientMultiplier.ToString("R", Inv) + " shock temp " + PhysicsSettings.ShockConvectionTemperatureMultiplier.ToString("R", Inv)
                    + " shock coeff " + PhysicsSettings.ShockConvectionCoefficientMultiplier.ToString("R", Inv));
                ev.WriteLine("t,alt_m,srf_speed,mach,parts,gone,names");
                var known = new List<string>();
                foreach (var part in vessel.SimulationObject.PartOwner.Parts) known.Add(part.Name);
                ev.WriteLine("0,," + "," + "," + known.Count + ",," + string.Join(" ", known));
                float t = 0; bool shot60 = false, shot40 = false, shot25 = false;
                double lastLogged = double.NegativeInfinity;
                while (t < 900)
                {
                    yield return new WaitForFixedUpdate();
                    double ut = G.UniverseModel.UniverseTime;
                    t = (float)(ut - ut0);
                    var v = G.ViewController.GetActiveSimVessel();
                    if (v == null || v.GlobalId.ToString() != id) { ev.WriteLine(t.ToString("F2", Inv) + ",,,," + "0,vessel," ); mark("vessel gone at " + t.ToString("F2", Inv)); break; }
                    if (free && v.Autopilot != null && v.Autopilot.Enabled && v.AltitudeFromSeaLevel < 70500) { v.SetAutopilotEnableDisable(false); mark("SAS off at " + t.ToString("F2", Inv)); }
                    if (ut <= lastLogged) continue;
                    lastLogged = ut;
                    string head = t.ToString("F3", Inv) + "," + v.AltitudeFromSeaLevel.ToString("R", Inv) + "," + v.SrfSpeedMagnitude.ToString("R", Inv) + "," + v.MachNumber.ToString("R", Inv);
                    var now = new List<string>();
                    int i = 0;
                    foreach (var part in v.SimulationObject.PartOwner.Parts)
                    {
                        now.Add(part.Name);
                        double units = 0;
                        foreach (var r in part.PartResourceContainer.GetAllResourcesContainedData())
                            if (db.GetResourceNameFromID(r.ResourceID) == "Ablator") units += r.StoredUnits;
                        w.WriteLine(head + "," + v.AtmDensity.ToString("R", Inv) + "," + part.Name + "," + i + "," + part.Temperature.ToString("R", Inv) + "," + part.MaxTemp.ToString("R", Inv)
                            + "," + part.ThermalData.ReentryFlux.ToString("R", Inv) + "," + units.ToString("R", Inv) + "," + part.ThermalData.ExposedAreaProportion.ToString("R", Inv)
                            + "," + part.ThermalData.EnvironmentFlux.ToString("R", Inv) + "," + part.ThermalData.BodyAtmosphereFlux.ToString("R", Inv) + "," + part.ThermalData.BodyLocalFlux.ToString("R", Inv)
                            + "," + part.ThermalData.BodySurfaceFlux.ToString("R", Inv) + "," + part.ThermalMass.ToString("R", Inv) + "," + part.WettedArea.ToString("R", Inv));
                        i++;
                    }
                    if (now.Count != known.Count)
                    {
                        var gone = new List<string>(known);
                        foreach (var n in now) gone.Remove(n);
                        ev.WriteLine(head + "," + now.Count + "," + string.Join(" ", gone) + "," + string.Join(" ", now));
                        mark("parts " + known.Count + " -> " + now.Count + " at " + t.ToString("F2", Inv) + " alt " + v.AltitudeFromSeaLevel.ToString("F0", Inv) + ": lost " + string.Join(" ", gone));
                        known = now;
                    }
                    if (!shot60 && v.AltitudeFromSeaLevel < 60000) { shot60 = true; yield return Shot(dir, "21-reentryorbit-60km"); }
                    if (!shot40 && v.AltitudeFromSeaLevel < 40000) { shot40 = true; yield return Shot(dir, "22-reentryorbit-40km"); }
                    if (!shot25 && v.AltitudeFromSeaLevel < 25000) { shot25 = true; yield return Shot(dir, "23-reentryorbit-25km"); }
                    if (v.Situation == VesselSituations.Landed || v.Situation == VesselSituations.Splashed) { mark("down " + v.Situation + " at " + t.ToString("F2", Inv)); break; }
                }
            }
            stop();
        }

        static IEnumerator Shot(string dir, string name)
        {
            yield return new WaitForEndOfFrame();
            ScreenCapture.CaptureScreenshot(Path.Combine(dir, "out", name + ".png"));
        }
    }
}
