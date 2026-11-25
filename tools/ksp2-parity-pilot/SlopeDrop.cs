using System;
using System.Collections;
using System.Globalization;
using System.IO;
using KSP.Game;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // Landing on a slope (thread "Fix relaunch, EVA, landing and parachute glitches": the engine bell sank into
    // the ground on landing). "slope <craft> <speed> <degrees>": the drop craft (craft-drop-<craft>.json) is
    // teleported over the land west of the KSC, point by point on a lat/lon grid, until the terrain collider under
    // it is a plane within 0.75 degrees of the wanted slope (five raycasts 3 m apart agree). Then it is set down
    // 1 m above that point falling at `speed` m/s, upright, and every physics tick logs its tilt from the local
    // vertical and from the ground's normal, the height of its root part over the terrain, its vertical speed,
    // situation and part count (slope-trace.csv), for 15 s. slope-probe.csv lists every point tried.
    public static class SlopeDrop
    {
        static GameInstance G => GameManager.Instance.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        static string F(double x) => x.ToString("R", Inv);

        static void ToSurface(VesselComponent vessel, double lat, double lon, double above, double vspeed)
        {
            var state = new KSP.Sim.State.SurfaceTeleportState { ReferenceBodyGuid = vessel.mainBody.Name, Altitude = above, Latitude = lat, Longitude = lon, VerticalSpeed = vspeed };
            G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject, state);
        }

        static Transform Root(VesselComponent v)
        {
            foreach (var part in v.SimulationObject.PartOwner.Parts)
                if (G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) return view.transform;
            return null;
        }

        static Vector3 Up(VesselComponent v, Vector3 at)
        {
            if (G.SpaceSimulation.TryGetViewObject(v.mainBody.SimulationObject, out var body)) return (at - body.transform.position).normalized;
            return Vector3.up;
        }

        // the nearest hit that is not a part (parts have rigidbodies; the terrain does not)
        static bool Ground(Vector3 from, Vector3 down, out RaycastHit ground)
        {
            ground = default;
            float best = float.MaxValue;
            foreach (var h in Physics.RaycastAll(from, down, 400f))
                if (h.rigidbody == null && h.distance < best) { best = h.distance; ground = h; }
            return best < float.MaxValue;
        }

        // the ground plane under `at`: (normal, slope from `up` in degrees, spread of the five hits' normals, height of `at` over the centre hit)
        static bool Plane(Vector3 at, Vector3 up, out Vector3 normal, out double slope, out double spread, out double height)
        {
            normal = up; slope = spread = height = double.NaN;
            var e1 = Vector3.Cross(up, Mathf.Abs(Vector3.Dot(up, Vector3.forward)) < 0.9f ? Vector3.forward : Vector3.right).normalized;
            var e2 = Vector3.Cross(up, e1).normalized;
            var offsets = new[] { Vector3.zero, e1 * 3f, -e1 * 3f, e2 * 3f, -e2 * 3f };
            var hits = new RaycastHit[5];
            for (int i = 0; i < 5; i++)
                if (!Ground(at + offsets[i] + up * 20f, -up, out hits[i])) return false;
            normal = Vector3.Cross(hits[1].point - hits[2].point, hits[3].point - hits[4].point).normalized;
            if (Vector3.Dot(normal, up) < 0) normal = -normal;
            slope = Vector3.Angle(normal, up);
            spread = 0;
            foreach (var h in hits) spread = Math.Max(spread, Vector3.Angle(h.normal, normal));
            height = Vector3.Dot(at - hits[0].point, up);
            return true;
        }

        public static IEnumerator Run(VesselComponent vessel, string dir, string craft, double speed, double degrees, Action<string> mark)
        {
            G.ViewController.flightInputHandler.OverrideInputThrottleCutoff(true);
            yield return new WaitForSeconds(3f);
            double lat = double.NaN, lon = double.NaN, found = double.NaN;
            using (var w = new StreamWriter(Path.Combine(dir, "out", "slope-probe.csv"), false) { AutoFlush = true })
            {
                w.WriteLine("lat,lon,slope_deg,spread_deg");
                for (double dlon = 0; dlon <= 1.2 && double.IsNaN(found); dlon += 0.05)
                    for (double dlat = -0.3; dlat <= 0.3 && double.IsNaN(found); dlat += 0.05)
                    {
                        double la = -0.0972 + dlat, lo = -74.8 - dlon;
                        ToSurface(vessel, la, lo, 30, 0);
                        yield return new WaitForSeconds(1.0f);
                        var v = G.ViewController.GetActiveSimVessel();
                        var root = v == null ? null : Root(v);
                        if (root == null) continue;
                        var up = Up(v, root.position);
                        if (!Plane(root.position, up, out _, out var slope, out var spread, out _)) { w.WriteLine(F(la) + "," + F(lo) + ",,"); continue; }
                        w.WriteLine(F(la) + "," + F(lo) + "," + F(slope) + "," + F(spread));
                        if (Math.Abs(slope - degrees) < 0.75 && spread < 1.0) { lat = la; lon = lo; found = slope; }
                    }
            }
            if (double.IsNaN(found)) { mark("slope: no point within 0.75 deg of " + F(degrees)); yield break; }
            mark("slope: " + F(found) + " deg at " + F(lat) + "," + F(lon) + "; " + craft + " falling at " + F(speed));
            // still, 20 m up, to read the plane once more where it will land; then 1 m up, falling
            ToSurface(vessel, lat, lon, 20, 0);
            yield return new WaitForSeconds(1.0f);
            ToSurface(vessel, lat, lon, 1.0, -speed);
            double start = G.UniverseModel.UniverseTime;
            using (var w = new StreamWriter(Path.Combine(dir, "out", "slope-trace.csv"), false) { AutoFlush = true })
            {
                w.WriteLine("t,parts,alt_m,vspeed,tilt_up_deg,tilt_ground_deg,slope_deg,root_over_ground_m,situation");
                double t = 0;
                while (t < 15)
                {
                    yield return new WaitForFixedUpdate();
                    t = G.UniverseModel.UniverseTime - start;
                    var v = G.ViewController.GetActiveSimVessel();
                    if (v == null) { w.WriteLine(F(t) + ",0,,,,,,,gone"); mark("slope: vessel gone at " + F(t)); break; }
                    var root = Root(v);
                    if (root == null) continue;
                    var up = Up(v, root.position);
                    Plane(root.position, up, out var normal, out var slope, out _, out var height);
                    int parts = 0;
                    foreach (var _ in v.SimulationObject.PartOwner.Parts) parts++;
                    w.WriteLine(string.Join(",", new[] { F(t), parts.ToString(Inv), F(v.AltitudeFromSeaLevel), F(v.VerticalSrfSpeed),
                        F(Vector3.Angle(root.up, up)), F(Vector3.Angle(root.up, normal)), F(slope), F(height), v.Situation.ToString() }));
                }
            }
            mark("slope: finished");
        }
    }
}
