using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using KSP.Game;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // Where the parachute canopy is drawn (thread "Fix relaunch, EVA, landing and parachute glitches": Josh saw
    // no parachute). "chutelook": the run-1 craft (craft-rotation.json: Mk1 pod, two tanks, Swivel, Mk16 chute)
    // 2500 m over the land west of the KSC, falling from rest, throttle cut; the engine stage, then the chute
    // stage at once. Every 0.1 s until 3 s after it is down (or 240 s): the chute part's canopy transforms (any
    // whose name holds "canopy"), each as height along the local vertical and sideways distance from the pod
    // part, and its scale; the chute part's renderers (enabled, visible to some camera), the box round the
    // enabled ones (centre height over the pod, size), and whether that box is inside the main flight camera's
    // view and how far the camera is from the pod (chute-look.csv). The transform names under the chute part are
    // listed once in chute-transforms.txt.
    public static class ChuteLook
    {
        static GameInstance G => GameManager.Instance.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        static string F(double x) => x.ToString("0.###", Inv);

        static Transform View(PartComponent part) =>
            G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view) ? view.transform : null;

        public static IEnumerator Run(VesselComponent vessel, string dir, Action<string> mark)
        {
            G.ViewController.flightInputHandler.OverrideInputThrottleCutoff(true);
            yield return new WaitForSeconds(3f);
            var state = new KSP.Sim.State.SurfaceTeleportState { ReferenceBodyGuid = vessel.mainBody.Name, Altitude = 2500, Latitude = -0.0972, Longitude = -74.8, VerticalSpeed = 0 };
            G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject, state);
            yield return new WaitForSeconds(1f);
            vessel = G.ViewController.GetActiveSimVessel();
            vessel.ActivateNextStage();
            yield return new WaitForSeconds(0.5f);
            vessel.ActivateNextStage();
            mark("chutelook: staged twice at alt " + F(vessel.AltitudeFromSeaLevel));
            double start = G.UniverseModel.UniverseTime;
            double downAt = double.NaN;
            bool listed = false;
            using (var w = new StreamWriter(Path.Combine(dir, "out", "chute-look.csv"), false) { AutoFlush = true })
            {
                w.WriteLine("t,alt_m,vspeed,situation,canopy,canopy_up_m,canopy_side_m,canopy_scale,renderers,enabled,visible,box_up_m,box_size_m,in_main_camera,camera_dist_m,camera_name");
                while (true)
                {
                    yield return new WaitForSeconds(0.1f);
                    double t = G.UniverseModel.UniverseTime - start;
                    var v = G.ViewController.GetActiveSimVessel();
                    if (v == null || t > 240) break;
                    Transform pod = null, chute = null;
                    foreach (var part in v.SimulationObject.PartOwner.Parts)
                    {
                        if (part.Name.StartsWith("pod_")) pod = View(part);
                        if (part.Name.StartsWith("parachute")) chute = View(part);
                    }
                    if (pod == null || chute == null) { w.WriteLine(F(t) + ",,,missing pod or chute"); continue; }
                    Vector3 up = Vector3.up;
                    if (G.SpaceSimulation.TryGetViewObject(v.mainBody.SimulationObject, out var body)) up = (pod.position - body.transform.position).normalized;
                    if (!listed)
                    {
                        listed = true;
                        var names = new List<string>();
                        foreach (var tr in chute.GetComponentsInChildren<Transform>(true)) names.Add(tr.name + " (" + (tr.gameObject.activeInHierarchy ? "active" : "inactive") + ")");
                        File.WriteAllLines(Path.Combine(dir, "out", "chute-transforms.txt"), names);
                    }
                    string sit = v.Situation.ToString();
                    var cam = Camera.main;
                    Plane[] planes = cam != null ? GeometryUtility.CalculateFrustumPlanes(cam) : null;
                    int all = 0, enabled = 0, visible = 0;
                    Bounds box = default; bool any = false;
                    foreach (var r in chute.GetComponentsInChildren<Renderer>(true))
                    {
                        all++;
                        if (!r.enabled || !r.gameObject.activeInHierarchy) continue;
                        enabled++;
                        if (r.isVisible) visible++;
                        if (!any) { box = r.bounds; any = true; } else box.Encapsulate(r.bounds);
                    }
                    string boxUp = any ? F(Vector3.Dot(box.center - pod.position, up)) : "";
                    string boxSize = any ? F(box.size.magnitude) : "";
                    string inCam = any && planes != null ? (GeometryUtility.TestPlanesAABB(planes, box) ? "1" : "0") : "";
                    string camDist = cam != null ? F((cam.transform.position - pod.position).magnitude) : "";
                    string camName = cam != null ? cam.name : "none";
                    bool wrote = false;
                    foreach (var tr in chute.GetComponentsInChildren<Transform>(true))
                    {
                        if (tr.name.IndexOf("canopy", StringComparison.OrdinalIgnoreCase) < 0) continue;
                        var d = tr.position - pod.position;
                        double h = Vector3.Dot(d, up);
                        w.WriteLine(string.Join(",", new[] { F(t), F(v.AltitudeFromSeaLevel), F(v.VerticalSrfSpeed), sit, tr.name + (tr.gameObject.activeInHierarchy ? "" : "(off)"),
                            F(h), F((d - up * (float)h).magnitude), F(tr.lossyScale.y), all.ToString(Inv), enabled.ToString(Inv), visible.ToString(Inv), boxUp, boxSize, inCam, camDist, camName }));
                        wrote = true;
                    }
                    if (!wrote) w.WriteLine(string.Join(",", new[] { F(t), F(v.AltitudeFromSeaLevel), F(v.VerticalSrfSpeed), sit, "", "", "", "", all.ToString(Inv), enabled.ToString(Inv), visible.ToString(Inv), boxUp, boxSize, inCam, camDist, camName }));
                    if (double.IsNaN(downAt) && (sit == "Landed" || sit == "Splashed")) { downAt = t; mark("chutelook: down at " + F(t)); }
                    if (!double.IsNaN(downAt) && t > downAt + 3) break;
                }
            }
            mark("chutelook: finished");
        }
    }
}
