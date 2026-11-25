using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using KSP;
using KSP.Game;
using KSP.Modules;
using KSP.Sim;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // How each part meets the ground (thread "Fix relaunch, EVA, landing and parachute glitches": Josh asked that
    // when a rocket or plane lands its individual parts collide and react properly). Every "strike <mode>" run
    // first lists every part's colliders (colliders.csv: type, size, convex; collider-points.csv: each collider's
    // vertices or corners in its part's own frame, so they can be laid over our model hulls), then logs, every
    // physics tick, each contact a part makes with anything that is not a part (the terrain, the runway): the
    // part, its collider, the point in the part's frame, the normal, separation, the pair's impulse and relative
    // speed (strike-contacts.csv); the craft's speed, vertical speed, height over the ground, pitch, roll and
    // situation (strike-trace.csv); and every part that leaves the craft (strike-parts.csv, by the ids in strike-part-ids.csv). Modes:
    //   land    Stock04_Jet (craft-jet.json) takes off from Runway_1, climbs to 25 m over the ground, throttle
    //           cut, and is flown down onto the runway (pitch held to sink at 1.5 m/s), gear down; 15 s rolling.
    //   belly   the same with the gear raised before the descent: it lands on its belly.
    //   tail    rolls at full throttle to 38 m/s, cuts the throttle and holds the stick full back until it is down
    //           to 8 m/s: too slow to fly away, it rotates on its main gear until the tail meets the runway.
    //           (Run 2 cut as it lifted at 50 m/s and nosed over; run 3 held 18 degrees from 25 m and climbed to
    //           470 m; run 4 cut at 52 m/s with the stick back and climbed to 49 m; run 5 centred the stick at
    //           12 degrees and 52 m/s, hopped and rolled out on its wheels.)
    //   taxi    ground handling on the runway, with every wheel logged each tick (taxi-wheels.csv: grounded, load,
    //           forward and sideways slip, rpm, motor and brake torque, steer angle, suspension travel):
    //             settle 5 s (engine staged at zero throttle, brakes on); accel brakes off, full throttle to 25 m/s;
    //             coast throttle cut 12 s; brake brakes on until under 0.2 m/s (20 s at most); taxi brakes off,
    //             full throttle to 10 m/s; steer throttle cut, SAS off, wheel steering full right 10 s while it
    //             coasts; stop steering centred, brakes on 8 s.
    //   stack   the drop stack (craft-drop-stack.json: pod, tanks, Swivel) set down upright on Runway_1 at 4 m/s.
    // Only inputs and action groups are used; physics is never modified.
    public static class GroundStrike
    {
        static GameInstance G => GameManager.Instance.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        static string F(double x) => x.ToString("R", Inv);

        // every wheel collider of the craft, by part index
        static IEnumerable<(int, WheelCollider)> Wheels(VesselComponent v)
        {
            foreach (var (i, _, frame) in Parts(v))
            {
                if (frame == null) continue;
                foreach (var wc in frame.GetComponentsInChildren<WheelCollider>()) yield return (i, wc);
            }
        }

        public static string Craft(string mode) => mode == "stack" ? "craft-drop-stack.json" : "craft-jet.json";

        // Contacts of one part with anything that has no rigidbody (terrain, runway, buildings).
        public sealed class ContactLog : MonoBehaviour
        {
            public static StreamWriter W;
            public static double Start;
            public int Index;
            public string Part;
            public Transform Frame;
            readonly List<ContactPoint> points = new List<ContactPoint>(16);

            static bool Claims(PartBehavior owner, Collider col)
            {
                var m = typeof(PartBehavior).GetProperty("Colliders");
                object v = m != null ? m.GetValue(owner) : typeof(PartBehavior).GetField("Colliders")?.GetValue(owner);
                if (v is Collider[] arr) return Array.IndexOf(arr, col) >= 0;
                return false;
            }

            void OnCollisionEnter(Collision c) => Log("enter", c);
            void OnCollisionStay(Collision c) => Log("stay", c);

            void Log(string kind, Collision c)
            {
                if (W == null || c.rigidbody != null || Frame == null) return;
                double t = G.UniverseModel.UniverseTime - Start;
                c.GetContacts(points);
                var rel = c.relativeVelocity;
                // which rigidbody took the contact, whether that body's part claims the collider (KSP2's
                // HandleCollisionEnter ignores one it does not), and the vessel the part belongs to now
                var body = c.contactCount > 0 && points.Count > 0 && points[0].thisCollider != null ? points[0].thisCollider.attachedRigidbody : null;
                var owner = body != null ? body.GetComponent<PartBehavior>() : null;
                string bodyName = body != null ? body.name + "#" + body.GetInstanceID().ToString(Inv) : "";
                string ownerName = owner != null ? owner.Model?.Name ?? "" : "";
                string vesselName = owner != null ? owner.Model?.PartOwner?.SimulationObject?.Vessel?.Name ?? "" : "";
                string physics = owner != null ? owner.Model?.PhysicsMode.ToString() ?? "" : "";
                foreach (var p in points)
                {
                    var at = Frame.InverseTransformPoint(p.point);
                    string claimed = owner != null && p.thisCollider != null ? (Claims(owner, p.thisCollider) ? "1" : "0") : "";
                    W.WriteLine(string.Join(",", new[] { F(t), kind, Index.ToString(Inv), Part, p.thisCollider != null ? p.thisCollider.name : "",
                        p.otherCollider != null ? p.otherCollider.name : "", p.otherCollider != null ? p.otherCollider.gameObject.layer.ToString(Inv) : "",
                        F(at.x), F(at.y), F(at.z), F(p.normal.x), F(p.normal.y), F(p.normal.z), F(p.separation),
                        F(c.impulse.magnitude), F(rel.magnitude), F(Vector3.Dot(rel, p.normal)), bodyName, ownerName, claimed, vesselName, physics }));
                }
            }
        }

        static IEnumerable<(int, PartComponent, Transform)> Parts(VesselComponent v)
        {
            int i = 0;
            foreach (var part in v.SimulationObject.PartOwner.Parts)
            {
                G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view);
                yield return (i++, part, view != null ? view.transform : null);
            }
        }

        static Vector3 Up(VesselComponent v, Vector3 at) =>
            G.SpaceSimulation.TryGetViewObject(v.mainBody.SimulationObject, out var body) ? (at - body.transform.position).normalized : Vector3.up;

        static double OverGround(Vector3 at, Vector3 up)
        {
            double best = double.NaN;
            foreach (var h in Physics.RaycastAll(at + up * 5f, -up, 600f))
                if (h.rigidbody == null && (double.IsNaN(best) || h.distance < best)) best = h.distance;
            return best - 5;
        }

        static void DumpColliders(VesselComponent v, string dir)
        {
            using (var w = new StreamWriter(Path.Combine(dir, "out", "colliders.csv"), false))
            using (var pw = new StreamWriter(Path.Combine(dir, "out", "collider-points.csv"), false))
            {
                w.WriteLine("index,part,collider,type,enabled,trigger,layer,convex,cx,cy,cz,sx,sy,sz,radius,height,direction,suspension,vertices,attached_body,physics_mode");
                pw.WriteLine("index,part,collider,x,y,z");
                foreach (var (i, part, frame) in Parts(v))
                {
                    if (frame == null) { w.WriteLine(i.ToString(Inv) + "," + part.Name + ",,noview"); continue; }
                    foreach (var col in frame.GetComponentsInChildren<Collider>(true))
                    {
                        string type = col.GetType().Name, convex = "", radius = "", height = "", direction = "", suspension = "", verts = "";
                        Vector3 centre = Vector3.zero, size = Vector3.zero;
                        var pts = new List<Vector3>();
                        void Corners(Vector3 c, Vector3 s)
                        {
                            for (int k = 0; k < 8; k++)
                                pts.Add(c + Vector3.Scale(s * 0.5f, new Vector3((k & 1) == 0 ? -1 : 1, (k & 2) == 0 ? -1 : 1, (k & 4) == 0 ? -1 : 1)));
                        }
                        if (col is BoxCollider b) { centre = b.center; size = b.size; Corners(b.center, b.size); }
                        else if (col is SphereCollider s) { centre = s.center; radius = F(s.radius); Corners(s.center, Vector3.one * s.radius * 2); }
                        else if (col is CapsuleCollider cc)
                        {
                            centre = cc.center; radius = F(cc.radius); height = F(cc.height); direction = cc.direction.ToString(Inv);
                            var ext = Vector3.one * cc.radius * 2; ext[cc.direction] = cc.height; Corners(cc.center, ext);
                        }
                        else if (col is WheelCollider wc) { centre = wc.center; radius = F(wc.radius); suspension = F(wc.suspensionDistance); }
                        else if (col is MeshCollider mc)
                        {
                            convex = mc.convex ? "1" : "0";
                            var mesh = mc.sharedMesh;
                            if (mesh != null)
                            {
                                Vector3[] vs = null;
                                try { vs = mesh.isReadable ? mesh.vertices : null; } catch (Exception) { vs = null; }
                                if (vs != null && vs.Length > 0) { pts.AddRange(vs); verts = vs.Length.ToString(Inv); }
                                else { verts = "unreadable"; centre = mesh.bounds.center; size = mesh.bounds.size; Corners(mesh.bounds.center, mesh.bounds.size); }
                            }
                        }
                        w.WriteLine(string.Join(",", new[] { i.ToString(Inv), part.Name, col.name, type, col.enabled ? "1" : "0", col.isTrigger ? "1" : "0", col.gameObject.layer.ToString(Inv), convex,
                            F(centre.x), F(centre.y), F(centre.z), F(size.x), F(size.y), F(size.z), radius, height, direction, suspension, verts,
                            col.attachedRigidbody != null ? col.attachedRigidbody.name + "#" + col.attachedRigidbody.GetInstanceID().ToString(Inv) : "", part.PhysicsMode.ToString() }));
                        // the collider's points in the part's frame (collider-local -> world -> part)
                        foreach (var q in pts)
                        {
                            var at = frame.InverseTransformPoint(col.transform.TransformPoint(q));
                            pw.WriteLine(i.ToString(Inv) + "," + part.Name + "," + col.name + "," + F(at.x) + "," + F(at.y) + "," + F(at.z));
                        }
                    }
                }
            }
        }

        static void Attach(VesselComponent v)
        {
            foreach (var (i, part, frame) in Parts(v))
            {
                if (frame == null) continue;
                var rb = frame.GetComponentInChildren<Rigidbody>();
                if (rb == null) rb = frame.GetComponentInParent<Rigidbody>();
                var host = rb != null ? rb.gameObject : frame.gameObject;
                var log = host.GetComponent<ContactLog>();
                if (log == null) log = host.AddComponent<ContactLog>();
                log.Index = i; log.Part = part.Name; log.Frame = frame;
            }
        }

        static void SetGroup(KSPActionGroup group, bool on)
        {
            if (G.ViewController.TryGetActiveVehicle(out var v)) v.ControllableActionGroups?.SetGroup(group, on);
        }

        public static IEnumerator Run(MonoBehaviour host, VesselComponent vessel, string dir, string mode, Action<string> mark)
        {
            var input = G.ViewController.flightInputHandler;
            yield return new WaitForSeconds(2f);
            DumpColliders(vessel, dir);
            mark("strike " + mode + ": colliders listed");
            string phase = "start";
            bool logging = true;
            var names = new Dictionary<int, string>();
            foreach (var (i, part, _) in Parts(vessel)) names[i] = part.Name;
            ContactLog.Start = G.UniverseModel.UniverseTime;
            ContactLog.W = new StreamWriter(Path.Combine(dir, "out", "strike-contacts.csv"), false);
            ContactLog.W.WriteLine("t,kind,index,part,collider,other,other_layer,px,py,pz,nx,ny,nz,separation,impulse,rel_speed,rel_normal,body,body_part,claimed,vessel,physics_mode");
            Attach(vessel);
            IEnumerator Log()
            {
                using (var w = new StreamWriter(Path.Combine(dir, "out", "strike-trace.csv"), false))
                using (var pw = new StreamWriter(Path.Combine(dir, "out", "strike-parts.csv"), false))
                {
                    w.WriteLine("t,phase,parts,srf_speed,vspeed,over_ground_m,pitch_deg,roll_deg,heading_deg,in_pitch,situation");
                    StreamWriter ww = null;
                    if (mode == "taxi")
                    {
                        ww = new StreamWriter(Path.Combine(dir, "out", "taxi-wheels.csv"), false);
                        ww.WriteLine("t,phase,index,wheel,grounded,load_n,forward_slip,sideways_slip,rpm,motor_torque,brake_torque,steer_deg,travel_m,suspension_m,radius_m,throttle,wheel_steer,brakes,thrust_kn");
                    }
                    pw.WriteLine("t,lost_id");
                    double last = double.NegativeInfinity;
                    var left = new HashSet<string>();
                    foreach (var (_, part, _) in Parts(vessel)) left.Add(part.GlobalId.ToString());
                    while (logging)
                    {
                        yield return new WaitForFixedUpdate();
                        var v = G.ViewController.GetActiveSimVessel();
                        if (v == null) continue;
                        double ut = G.UniverseModel.UniverseTime;
                        if (ut <= last) continue;
                        last = ut;
                        double t = ut - ContactLog.Start;
                        var now = new HashSet<string>();
                        Transform root = null;
                        foreach (var (_, part, frame) in Parts(v)) { now.Add(part.GlobalId.ToString()); if (root == null) root = frame; }
                        if (now.Count < left.Count)
                        {
                            foreach (var id in left) if (!now.Contains(id)) pw.WriteLine(F(t) + "," + id);
                            pw.Flush();
                            left = now;
                        }
                        double over = double.NaN;
                        if (root != null) over = OverGround(root.position, Up(v, root.position));
                        w.WriteLine(string.Join(",", new[] { F(t), phase, now.Count.ToString(Inv), F(v.SrfSpeedMagnitude), F(v.VerticalSrfSpeed), F(over),
                            F(v.Pitch_HorizonRelative), F(v.Roll_HorizonRelative), F(v.Heading), F(v.flightCtrlState.pitch), v.Situation.ToString() }));
                        if (ww != null)
                        {
                            double thrust = 0;
                            foreach (var part in v.SimulationObject.PartOwner.Parts)
                                if (part.TryGetModuleData<PartComponentModule_Engine, Data_Engine>(out var de)) thrust += de.FinalThrustValue;
                            string brakes = v.GetActionGroupState(KSPActionGroup.Brakes).ToString();
                            foreach (var (i, wc) in Wheels(v))
                            {
                                bool grounded = wc.GetGroundHit(out var hit);
                                wc.GetWorldPose(out var pos, out _);
                                var top = wc.transform.TransformPoint(wc.center);
                                ww.WriteLine(string.Join(",", new[] { F(t), phase, i.ToString(Inv), wc.name, grounded ? "1" : "0",
                                    F(grounded ? hit.force : 0), F(grounded ? hit.forwardSlip : 0), F(grounded ? hit.sidewaysSlip : 0), F(wc.rpm),
                                    F(wc.motorTorque), F(wc.brakeTorque), F(wc.steerAngle), F((top - pos).magnitude), F(wc.suspensionDistance), F(wc.radius),
                                    F(v.flightCtrlState.mainThrottle), F(v.flightCtrlState.wheelSteer), brakes, F(thrust) }));
                            }
                        }
                    }
                    ww?.Dispose();
                }
            }
            void Phase(string p) { phase = p; mark("strike " + mode + " " + p); }
            // the part names by global id, for strike-parts.csv
            using (var idw = new StreamWriter(Path.Combine(dir, "out", "strike-part-ids.csv"), false))
            {
                idw.WriteLine("index,id,part");
                foreach (var (i, part, _) in Parts(vessel)) idw.WriteLine(i.ToString(Inv) + "," + part.GlobalId + "," + part.Name);
            }
            var logger = host.StartCoroutine(Log());
            float t0;
            if (mode == "stack")
            {
                input.OverrideInputThrottleCutoff(true);
                var state = new KSP.Sim.State.SurfaceTeleportState { ReferenceBodyGuid = vessel.mainBody.Name, Altitude = 1.0, Latitude = vessel.Latitude, Longitude = vessel.Longitude, VerticalSpeed = -4.0 };
                Phase("drop");
                G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject, state);
                yield return new WaitForSeconds(1f);
                Attach(G.ViewController.GetActiveSimVessel());
                yield return new WaitForSeconds(11f);
            }
            else if (mode == "taxi")
            {
                // SAS holds it straight while it speeds up, coasts and brakes; off for the steering
                vessel.SetAutopilotEnableDisable(true);
                SetGroup(KSPActionGroup.Brakes, true);
                input.OverrideInputThrottleCutoff(true);
                vessel.ActivateNextStage();
                Phase("settle");
                yield return new WaitForSeconds(5f);
                SetGroup(KSPActionGroup.Brakes, false);
                input.OverrideInputMaxThrottle(true);
                Phase("accel");
                t0 = 0;
                while (t0 < 60 && (G.ViewController.GetActiveSimVessel()?.SrfSpeedMagnitude ?? 0) < 25) { t0 += Time.deltaTime; yield return null; }
                input.OverrideInputThrottleCutoff(true);
                Phase("coast");
                yield return new WaitForSeconds(12f);
                SetGroup(KSPActionGroup.Brakes, true);
                Phase("brake");
                t0 = 0;
                while (t0 < 20 && (G.ViewController.GetActiveSimVessel()?.SrfSpeedMagnitude ?? 0) > 0.2) { t0 += Time.deltaTime; yield return null; }
                SetGroup(KSPActionGroup.Brakes, false);
                input.OverrideInputMaxThrottle(true);
                Phase("taxi");
                t0 = 0;
                while (t0 < 30 && (G.ViewController.GetActiveSimVessel()?.SrfSpeedMagnitude ?? 0) < 10) { t0 += Time.deltaTime; yield return null; }
                input.OverrideInputThrottleCutoff(true);
                vessel.SetAutopilotEnableDisable(false);
                InputHolder.WheelSteer = 1f;
                Phase("steer");
                yield return new WaitForSeconds(10f);
                InputHolder.WheelSteer = null;
                FlightCtrlStateInputOverride.InputWheelSteeringOverride = 0f;
                SetGroup(KSPActionGroup.Brakes, true);
                Phase("stop");
                yield return new WaitForSeconds(8f);
            }
            else
            {
                vessel.SetAutopilotEnableDisable(true);
                SetGroup(KSPActionGroup.Brakes, false);
                input.OverrideInputMaxThrottle(true);
                vessel.ActivateNextStage();
                Phase("roll");
                if (mode == "tail")
                {
                    t0 = 0;
                    while (t0 < 90 && (G.ViewController.GetActiveSimVessel()?.SrfSpeedMagnitude ?? 0) < 38) { t0 += Time.deltaTime; yield return null; }
                    input.OverrideInputThrottleCutoff(true);
                    InputHolder.Pitch = 1f;
                    Phase("cut and rotate");
                    t0 = 0;
                    bool flying = false;
                    double maxPitch = -90;
                    while (t0 < 40)
                    {
                        var v = G.ViewController.GetActiveSimVessel();
                        if (v == null) break;
                        maxPitch = Math.Max(maxPitch, v.Pitch_HorizonRelative);
                        if (v.Situation == VesselSituations.Flying) flying = true;
                        else if (flying) { Phase("struck"); flying = false; }
                        if (v.SrfSpeedMagnitude < 8) break;
                        t0 += Time.deltaTime; yield return null;
                    }
                    InputHolder.Pitch = null;
                    SetGroup(KSPActionGroup.Brakes, true);
                    Phase("after (pitch peaked at " + maxPitch.ToString("F1", Inv) + ")");
                    yield return new WaitForSeconds(5f);
                    goto done;
                }
                double rotateAt = 60;
                t0 = 0;
                while (t0 < 90 && (G.ViewController.GetActiveSimVessel()?.SrfSpeedMagnitude ?? 0) < rotateAt) { t0 += Time.deltaTime; yield return null; }
                Phase("rotate");
                InputHolder.Pitch = 1f;
                t0 = 0;
                while (t0 < 10 && (G.ViewController.GetActiveSimVessel()?.Pitch_HorizonRelative ?? 0) < 10) { t0 += Time.deltaTime; yield return null; }
                InputHolder.Pitch = null;
                Phase("climb");
                t0 = 0;
                while (t0 < 30)
                {
                    var v = G.ViewController.GetActiveSimVessel();
                    Transform root = null;
                    if (v != null) foreach (var (_, _, frame) in Parts(v)) { root = frame; break; }
                    if (root != null && OverGround(root.position, Up(v, root.position)) > 25) break;
                    t0 += Time.deltaTime; yield return null;
                }
                if (mode == "belly")
                {
                    // the group's state may not match the deployed gear at launch: raise it through a set
                    SetGroup(KSPActionGroup.Gear, true);
                    yield return new WaitForSeconds(0.2f);
                    SetGroup(KSPActionGroup.Gear, false);
                    Phase("gear up (group " + G.ViewController.GetActiveSimVessel()?.GetActionGroupState(KSPActionGroup.Gear) + ")");
                    yield return new WaitForSeconds(3f);
                }
                input.OverrideInputThrottleCutoff(true);
                Phase("descend");
                // pitch held to sink at 1.5 m/s, nose kept between -5 and 12 degrees
                t0 = 0;
                while (t0 < 90)
                {
                    var v = G.ViewController.GetActiveSimVessel();
                    if (v == null) break;
                    if (v.Situation == VesselSituations.Landed || v.Situation == VesselSituations.Splashed) break;
                    double err = -1.5 - v.VerticalSrfSpeed;
                    double pitch = v.Pitch_HorizonRelative;
                    float cmd = Mathf.Clamp((float)(0.25 * err), -0.6f, 0.6f);
                    if (pitch > 12) cmd = Mathf.Min(cmd, -0.2f);
                    if (pitch < -5) cmd = Mathf.Max(cmd, 0.3f);
                    InputHolder.Pitch = cmd;
                    t0 += Time.deltaTime; yield return null;
                }
                InputHolder.Pitch = null;
                Phase("down");
                SetGroup(KSPActionGroup.Brakes, true);
                yield return new WaitForSeconds(15f);
            }
        done:
            InputHolder.Clear();
            logging = false;
            yield return new WaitForFixedUpdate();
            yield return new WaitForFixedUpdate();
            host.StopCoroutine(logger);
            ContactLog.W?.Dispose();
            ContactLog.W = null;
            mark("strike " + mode + ": finished");
        }
    }
}
