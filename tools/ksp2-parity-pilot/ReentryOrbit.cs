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

        // the craft file: "reentry" (pod over heat shield), "pod" (the bare pod), "stack" (pod, six tanks, Swivel),
        // "k1" and "k2" (the stock Kerbal K1 and K2, 38 and 48 parts: radial boosters, nose cones, fins, solar panels),
        // "jet" (the stock jet), "fairing" (the pod under a closed AE-FF125 fairing nose)
        public static string Craft(string name) => name == "reentry" ? "craft-reentry.json" : name == "stack" ? "craft-stack.json" : name == "k1" ? "craft-k1.json" : name == "k2" ? "craft-k2.json" : name == "jet" ? "craft-jet.json" : name == "fairing" ? "craft-fairing-nose.json" : name == "asparagus" ? "craft-asparagus.json" : name == "asym" ? "craft-asym.json" : name == "sparrow" ? "craft-sparrow.json" : "craft-drop-" + name + ".json";

        // the thermal constants KSP2 runs with (PhysicsSettings, after its settings file)
        public static string ThermalSettings()
        {
            string F(double x) => x.ToString("R", Inv);
            return "thermal settings: background epsilon " + F(PhysicsSettings.GLOBAL_BACKGROUND_PART_TEMPERATURE_EPSILON)
                + " occlude area " + F(PhysicsSettings.OccludeConvectionAreaMultiplier)
                + " max angular velocity " + F(PhysicsSettings.MaxAngularVelocity);
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
            using (var fw = new StreamWriter(Path.Combine(dir, "out", "reentryorbit-forces.csv"), false))
            using (var ow = new StreamWriter(Path.Combine(dir, "out", "reentryorbit-occlusion.csv"), false))
            {
                ow.WriteLine("t,rank,index,part,max_dot,min_dot,centroid_dot,min_x,min_y,max_x,max_y,projected_radius,exposed,cube_cx,cube_cy,cube_cz,cube_sx,cube_sy,cube_sz,vel_x,vel_y,vel_z,px,py,pz");
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
                    if (fw != null) LogForces(v, fw, t);
                    if (++occlusionTick % 5 == 0) LogOcclusion(v, ow, t);
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

        // T11 shadows: KSP2's own occlusion pass (ThermalBehavior._thermalOcclusionGraph, sorted as it shades) every
        // 0.1 s: each part in its order, its projected outline (world x and y of its corners square to the flow),
        // the drag cube that outline comes from, the velocity it was projected along, and the exposure it got.
        static int occlusionTick;
        static readonly System.Reflection.FieldInfo GraphField = typeof(ThermalBehavior).GetField("_thermalOcclusionGraph", System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);
        static readonly System.Reflection.FieldInfo VelocityField = typeof(ThermalBehavior).GetField("_vesselVelocity", System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);
        public static void LogOcclusion(VesselComponent v, StreamWriter w, float t)
        {
            string F(double x) => x.ToString("R", Inv);
            if (!G.SpaceSimulation.TryGetViewObject(v.SimulationObject, out var view)) return;
            var tb = view.GetComponent<ThermalBehavior>();
            if (tb == null || GraphField == null) return;
            var graph = GraphField.GetValue(tb) as KSP.ThermalOcclusionGraph;
            if (graph == null) return;
            var vel = VelocityField != null ? (Vector3d)VelocityField.GetValue(tb) : Vector3d.zero;
            var order = new List<PartComponent>(v.SimulationObject.PartOwner.Parts);
            int rank = 0;
            foreach (var d in graph.ConvectionOccluderData)
            {
                if (d == null || d.Part == null) continue;
                var pm = d.Part.Model;
                var b = d.ProjectedBounds;
                var drag = d.PartDragData;
                var c = drag != null ? drag.weightedCenter : Vector3.zero;
                var s = drag != null ? drag.weightedSize : Vector3.zero;
                var pos = d.Part.transform.position;
                w.WriteLine(string.Join(",", t.ToString("F3", Inv), rank.ToString(Inv), order.IndexOf(pm).ToString(Inv), pm.Name, F(b.MaximumDot), F(b.MinimumDot), F(b.WorldCentroidDot),
                    F(b.MinExtents.x), F(b.MinExtents.y), F(b.MaxExtents.x), F(b.MaxExtents.y), F(d.ProjectedRadius), F(pm.ThermalData.ExposedAreaProportion),
                    F(c.x), F(c.y), F(c.z), F(s.x), F(s.y), F(s.z), F(vel.x), F(vel.y), F(vel.z), F(pos.x), F(pos.y), F(pos.z)));
                rank++;
            }
        }

        // T11 tumble: every aerodynamic force on each part this tick in the part's own frame (KSP2's IForce lists:
        // force and where it acts), with the part's spin, airflow, mass, centre of mass, inertia and angular drag
        // as PhysX has them (Unity frame, part-local).
        public static bool forcesHeader;
        public static void LogForces(VesselComponent v, StreamWriter w, float t)
        {
            string F(double x) => x.ToString("R", Inv);
            string V(Vector3 x) => F(x.x) + "," + F(x.y) + "," + F(x.z);
            if (!forcesHeader)
            {
                w.WriteLine("t,alt_m,mach,q_kpa,density,part,ax,ay,az,wx,wy,wz,mass,comx,comy,comz,ix,iy,iz,irx,iry,irz,irw,angular_drag,kind,fx,fy,fz,px,py,pz");
                forcesHeader = true;
            }
            foreach (var part in v.SimulationObject.PartOwner.Parts)
            {
                if (!G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) continue;
                var pb = view.GetComponent<PartBehavior>();
                var body = pb?.Rigidbody?.activeRigidBody;
                var rb = pb?.Rigidbody?.Model;
                if (body == null || rb == null) { w.WriteLine("# " + part.Name + " no rigidbody (behaviour " + (pb != null) + ", rigidbody " + (pb?.Rigidbody != null) + ", model " + (rb != null) + ")"); continue; }
                var tr = body.transform;
                var air = tr.InverseTransformDirection(body.velocity);
                var spin = tr.InverseTransformDirection(body.angularVelocity);
                var r = body.inertiaTensorRotation;
                string head = t.ToString("F3", Inv) + "," + F(v.AltitudeFromSeaLevel) + "," + F(v.MachNumber) + "," + F(v.DynamicPressure_kPa) + "," + F(v.AtmDensity) + "," + part.Name + ","
                    + V(air) + "," + V(spin) + "," + F(body.mass) + "," + V(body.centerOfMass) + "," + V(body.inertiaTensor) + "," + F(r.x) + "," + F(r.y) + "," + F(r.z) + "," + F(r.w) + "," + F(body.angularDrag);
                foreach (var list in new[] { rb.Forces, rb.SingleFrameForces })
                    foreach (var f in list)
                    {
                        var P = f.RelativePosition; var Fo = f.RelativeForce;
                        w.WriteLine(head + "," + f.GetType().Name + "," + F(Fo.x) + "," + F(Fo.y) + "," + F(Fo.z) + "," + F(P.x) + "," + F(P.y) + "," + F(P.z));
                    }
            }
        }

        static IEnumerator Shot(string dir, string name)
        {
            yield return new WaitForEndOfFrame();
            ScreenCapture.CaptureScreenshot(Path.Combine(dir, "out", name + ".png"));
        }
    }
}
