using System;
using System.IO;
using System.Linq;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.Reflection;
using BepInEx;
using KSP.Game;
using KSP.IO;
using KSP.OAB;
using KSP.Sim;
using KSP.Sim.impl;
using Newtonsoft.Json;
using UnityEngine;
using UnityEngine.SceneManagement;

namespace OSE.Validation.Joints
{
    // Controlled structural benchmark: the game builds the real craft and its joints. We snapshot
    // their live settings into an isolated PhysX scene, retain every parallel/reinforcement joint,
    // and load them at 50 Hz. No gameplay physics or existing campaign is changed.
    [BepInPlugin("org.openspaceexplorer.jointparity", "OSE Joint Parity", "0.1.0")]
    public sealed class JointParity : BaseUnityPlugin
    {
        const string Campaign = "OSE Joint Validation";
        string dir;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        static GameInstance G => GameManager.Instance?.Game;
        static GameState State => G?.GlobalGameState?.GetState() ?? GameState.Invalid;
        void Awake()
        {
            dir = Path.Combine(Paths.GameRootPath, "BepInEx", "ose-joint-validation");
            if (!File.Exists(Path.Combine(dir, "run.txt"))) return;
            Application.runInBackground = true;
            Directory.CreateDirectory(Path.Combine(dir, "out"));
            StartCoroutine(Guard(Run()));
        }
        void Mark(string s)
        {
            Logger.LogInfo(s);
            File.AppendAllText(Path.Combine(dir, "out", "steps.log"), DateTime.UtcNow.ToString("O") + " " + s + "\n");
        }
        IEnumerator Guard(IEnumerator r)
        {
            while (true)
            {
                object current;
                try { if (!r.MoveNext()) break; current = r.Current; }
                catch (Exception e) { Mark("FAILED " + e); File.WriteAllText(Path.Combine(dir, "out", "failed.txt"), e.ToString()); yield break; }
                if (current is IEnumerator nested) yield return Guard(nested); else yield return current;
            }
        }
        IEnumerator Wait(GameState s, float limit = 300)
        {
            float start = Time.realtimeSinceStartup;
            while (State != s && Time.realtimeSinceStartup - start < limit) yield return null;
            if (State != s) throw new Exception("Waiting for " + s + ", found " + State);
        }
        IEnumerator Run()
        {
            Mark("starting");
            yield return Wait(GameState.MainMenu, 600);
            yield return new WaitForSecondsRealtime(3);
            Mark("main menu ready");
            string seed = Path.Combine(G.SaveLoadManager.GetCampaignFolderPath(CampaignType.SinglePlayer, Campaign), "seed.json");
            if (!File.Exists(seed)) throw new Exception("Missing isolated KSC seed: " + seed);
            G.SaveLoadManager.LoadGameFromFile(seed, (_, ok) => Mark("isolated campaign loaded " + ok));
            yield return Wait(GameState.KerbalSpaceCenter, 600);
            yield return new WaitForSecondsRealtime(5);
            var settings = new Dictionary<string, object>();
            foreach (var p in typeof(PhysicsSettings).GetProperties(BindingFlags.Public | BindingFlags.Static))
                if (p.Name.Contains("JOINT") || p.Name.Contains("Joint") || p.Name.Contains("TENSOR"))
                    try { settings[p.Name] = p.GetValue(null); } catch { }
            settings["multiJointsEnabled"] = PersistentProfileManager.MultiJointsEnabled;
            settings["unbreakableJoints"] = G.SessionManager.IsDifficultyOptionEnabled("UnbreakableJoints");
            settings["unity"] = Application.unityVersion;
            settings["gameAssembly"] = typeof(VesselComponent).Assembly.GetName().Version.ToString();
            settings["defaultSolverIterations"] = Physics.defaultSolverIterations;
            settings["defaultSolverVelocityIterations"] = Physics.defaultSolverVelocityIterations;
            File.WriteAllText(Path.Combine(dir, "out", "settings.json"), JsonConvert.SerializeObject(settings, Formatting.Indented));
            foreach (string line in File.ReadAllLines(Path.Combine(dir, "run.txt")))
            {
                string name = line.Trim(); if (name.Length == 0) continue;
                Mark("loading craft " + name);
                var snapshot = IOProvider.FromJsonFile<OABHistoricalSnapshot>(Path.Combine(dir, name + ".json"));
                bool shown = false;
                G.OAB.ShowOAB(new OABConfig(OABVariant.VAB, OABEnvironmentType.Orbital, OABConstructionType.User, null), snapshot, null, () => shown = true);
                float until = Time.realtimeSinceStartup + 180;
                while (!shown && Time.realtimeSinceStartup < until) yield return null;
                if (!shown) throw new Exception("OAB timeout " + name);
                yield return new WaitForSecondsRealtime(3);
                G.OAB.Current.eventsUI.OnLaunch?.Invoke();
                yield return Wait(GameState.FlightView, 600);
                yield return new WaitForSecondsRealtime(5);
                var vessel = G.ViewController.GetActiveSimVessel();
                if (vessel == null) throw new Exception("No active vessel " + name);
                vessel.SetAutopilotEnableDisable(false);
                yield return new WaitForEndOfFrame();
                ScreenCapture.CaptureScreenshot(Path.Combine(dir, "out", name + ".png"));
                var specimen = Capture(vessel, name);
                Mark(name + " parts " + specimen.bodies.Count + " native joints " + specimen.joints.Count);
                File.WriteAllText(Path.Combine(dir, "out", name + "-specimen.json"), JsonConvert.SerializeObject(specimen, Formatting.Indented, new VectorConverter()));
                if (specimen.joints.Count == 0) throw new Exception("No live joints " + name);
                yield return Bench(specimen, name);
                G.ViewController.ObserveKSC();
                yield return Wait(GameState.KerbalSpaceCenter);
                yield return new WaitForSecondsRealtime(3);
            }
            Mark("complete");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            Application.Quit();
        }

        public sealed class Body
        {
            public string guid, part;
            public float[] position, rotation, com, inertia, inertiaRotation;
            public float mass, drag, angularDrag, maxAngularVelocity;
            public int solverIterations, solverVelocityIterations;
        }
        public sealed class Joint
        {
            public int host, connected;
            public Dictionary<string, object> properties;
        }
        public sealed class Specimen
        {
            public string name;
            public List<Body> bodies = new List<Body>();
            public List<Joint> joints = new List<Joint>();
            public List<string> partsWithoutRigidbodies = new List<string>();
        }
        static float[] A(Vector3 v) => new[] { v.x, v.y, v.z };
        static float[] A(Quaternion q) => new[] { q.x, q.y, q.z, q.w };
        static Vector3 V(float[] a) => new Vector3(a[0], a[1], a[2]);
        static Quaternion Q(float[] a) => new Quaternion(a[0], a[1], a[2], a[3]);
        // Copy only the joint physics API; Component/Object properties are intentionally excluded.
        static IEnumerable<PropertyInfo> JointProperties() => typeof(ConfigurableJoint).GetProperties(BindingFlags.Instance | BindingFlags.Public)
            .Where(p => p.CanRead && p.CanWrite && p.GetIndexParameters().Length == 0 && p.Name != "connectedBody" && p.Name != "connectedArticulationBody" &&
                (p.DeclaringType == typeof(ConfigurableJoint) || p.DeclaringType == typeof(UnityEngine.Joint)));
        Specimen Capture(VesselComponent vessel, string name)
        {
            var s = new Specimen { name = name };
            var rbs = new List<Rigidbody>();
            Transform root = null;
            foreach (var part in vessel.SimulationObject.PartOwner.Parts)
            {
                if (!G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) throw new Exception("No part view");
                var rb = view.GetComponent<Rigidbody>();
                if (rb == null) { s.partsWithoutRigidbodies.Add(part.Guid + " " + part.Name); continue; }
                if (root == null) root = view.transform;
                s.bodies.Add(new Body { guid = part.Guid.ToString(), part = part.Name,
                    position = A(root.InverseTransformPoint(rb.position)), rotation = A(Quaternion.Inverse(root.rotation) * rb.rotation),
                    com = A(rb.centerOfMass), mass = rb.mass, inertia = A(rb.inertiaTensor), inertiaRotation = A(rb.inertiaTensorRotation),
                    drag = rb.drag, angularDrag = rb.angularDrag, maxAngularVelocity = rb.maxAngularVelocity,
                    solverIterations = rb.solverIterations, solverVelocityIterations = rb.solverVelocityIterations });
                rbs.Add(rb);
            }
            for (int i = 0; i < rbs.Count; i++) foreach (var j in rbs[i].GetComponents<ConfigurableJoint>())
            {
                int k = rbs.IndexOf(j.connectedBody);
                if (k < 0) throw new Exception("Joint outside craft");
                var props = new Dictionary<string, object>();
                foreach (var p in JointProperties()) props[p.Name] = p.GetValue(j);
                s.joints.Add(new Joint { host = i, connected = k, properties = props });
            }
            return s;
        }
        IEnumerator Bench(Specimen s, string name)
        {
            // Two identical repetitions catch nondeterministic construction/solver outcomes.
            // Opposite forces act at the SAME spatial point, so net force and torque are zero.
            int target = Enumerable.Range(1, s.bodies.Count - 1).OrderByDescending(i => V(s.bodies[i].position).sqrMagnitude).First();
            var application = V(s.bodies[target].position) + Q(s.bodies[target].rotation) * V(s.bodies[target].com);
            using (var w = new StreamWriter(Path.Combine(dir, "out", name + "-native.csv")))
            using (var loads = new StreamWriter(Path.Combine(dir, "out", name + "-inputs.csv")))
            {
                w.WriteLine("case,tick,t_s,load_kn,joint,host,connected,alive,force_kn,torque_knm,angle_rad,anchor_error_m,fx,fy,fz,tx,ty,tz");
                loads.WriteLine("case,tick,part,fx,fy,fz,ox,oy,oz");
                foreach (int repeat in new[] { 0, 1 }) foreach (int axis in new[] { 0, 1, 2, 3 }) foreach (int sign in new[] { 1, -1 })
                {
                    string id = "r" + repeat + "-" + "xyzt"[axis] + (sign > 0 ? "+" : "-");
                    var scene = SceneManager.CreateScene("joint-bench-" + id, new CreateSceneParameters(LocalPhysicsMode.Physics3D));
                    var ps = scene.GetPhysicsScene();
                    var bodies = new List<Rigidbody>();
                    foreach (var b in s.bodies)
                    {
                        var go = new GameObject(b.part); SceneManager.MoveGameObjectToScene(go, scene);
                        go.transform.SetPositionAndRotation(V(b.position), Q(b.rotation));
                        var rb = go.AddComponent<Rigidbody>(); rb.useGravity = false;
                        rb.mass = b.mass; rb.centerOfMass = V(b.com); rb.inertiaTensor = V(b.inertia); rb.inertiaTensorRotation = Q(b.inertiaRotation);
                        rb.drag = b.drag; rb.angularDrag = b.angularDrag; rb.maxAngularVelocity = b.maxAngularVelocity;
                        rb.solverIterations = b.solverIterations; rb.solverVelocityIterations = b.solverVelocityIterations;
                        rb.sleepThreshold = 0; bodies.Add(rb);
                    }
                    var joints = new List<ConfigurableJoint>();
                    foreach (var j in s.joints)
                    {
                        var c = bodies[j.host].gameObject.AddComponent<ConfigurableJoint>();
                        c.autoConfigureConnectedAnchor = false;
                        c.connectedBody = bodies[j.connected];
                        foreach (var p in JointProperties()) if (j.properties.TryGetValue(p.Name, out var val)) p.SetValue(c, val);
                        joints.Add(c);
                    }
                    Physics.SyncTransforms();
                    bool broken = false; int tick = 0;
                    var schedule = Enumerable.Repeat(0f, 30).Concat(Enumerable.Repeat(1f, 30)).Concat(Enumerable.Repeat(0f, 60))
                        .Concat(Enumerable.Range(0, 285).Select(k => Mathf.Min(1000000f, Mathf.Pow(1.05f, k))))
                        .Concat(Enumerable.Repeat(1000000f, 30));
                    foreach (float level in schedule)
                    {
                        // A 1 kN pulse and 1.2 s ring-down precede a 5%-per-tick load ramp.
                        {
                            Vector3 force = (axis == 0 ? Vector3.right : axis == 1 ? Vector3.up : Vector3.forward) * (sign * level);
                            // The point and direction follow the root's rotation, matching OSE's vessel-frame inputs.
                            Vector3 point = bodies[0].position + bodies[0].rotation * application;
                            Vector3 worldForce = bodies[0].rotation * force;
                            // Prescribed offsets relative to each part origin are fixed in vessel axes.
                            // This avoids feeding reference-solver deflection back into OSE's load history.
                            var targetPoint = bodies[target].position + bodies[0].rotation * (application - V(s.bodies[target].position));
                            var applied = new List<(int part, Vector3 force, Vector3 point)> { (target, worldForce, targetPoint), (0, -worldForce, point) };
                            if (axis == 3)
                            {
                                applied.Clear();
                                var lever = bodies[0].rotation * Vector3.right * .5f;
                                foreach (var p in new[] { target, 0 })
                                {
                                    var centre = bodies[p].worldCenterOfMass;
                                    var f = bodies[0].rotation * Vector3.forward * (sign * level * (p == target ? 1 : -1));
                                    applied.Add((p, f, centre + lever)); applied.Add((p, -f, centre - lever));
                                }
                            }
                            foreach (var pair in applied)
                            {
                                var rb = bodies[pair.part]; rb.AddForceAtPosition(pair.force, pair.point, ForceMode.Force);
                                var off = Quaternion.Inverse(bodies[0].rotation) * (pair.point - rb.position);
                                var f = Quaternion.Inverse(bodies[0].rotation) * pair.force;
                                loads.WriteLine(string.Join(",", new[] { id, tick.ToString(), pair.part.ToString(), N(f.x), N(f.y), N(f.z), N(off.x), N(off.y), N(off.z) }));
                            }
                            ps.Simulate(0.02f);
                            for (int n = 0; n < joints.Count; n++)
                            {
                                var j = joints[n]; var spec = s.joints[n]; bool alive = j != null;
                                var relative = Quaternion.Inverse(bodies[spec.connected].rotation) * bodies[spec.host].rotation;
                                var rest = Quaternion.Inverse(Q(s.bodies[spec.connected].rotation)) * Q(s.bodies[spec.host].rotation);
                                var delta = relative * Quaternion.Inverse(rest);
                                // Quaternion.Angle rounds small angles to zero; atan2 retains microradians.
                                float angle = 2f * Mathf.Atan2(new Vector3(delta.x, delta.y, delta.z).magnitude, Mathf.Abs(delta.w));
                                float err = alive ? Vector3.Distance(j.transform.TransformPoint(j.anchor), j.connectedBody.transform.TransformPoint(j.connectedAnchor)) : float.NaN;
                                var jf = alive ? Quaternion.Inverse(bodies[0].rotation) * j.currentForce : Vector3.one * float.NaN;
                                var jt = alive ? Quaternion.Inverse(bodies[0].rotation) * j.currentTorque : Vector3.one * float.NaN;
                                w.WriteLine(string.Join(",", new[] { id, tick.ToString(), N((tick + 1) * .02f), N(level), n.ToString(), spec.host.ToString(), spec.connected.ToString(), alive ? "1" : "0", N(alive ? j.currentForce.magnitude : float.NaN), N(alive ? j.currentTorque.magnitude : float.NaN), N(angle), N(err), N(jf.x), N(jf.y), N(jf.z), N(jt.x), N(jt.y), N(jt.z) }));
                                if (!alive) broken = true;
                            }
                        }
                        if (broken) break;
                        tick++;
                    }
                    Mark(name + " " + id + " ticks " + tick + " broken " + broken);
                    var unload = SceneManager.UnloadSceneAsync(scene); while (!unload.isDone) yield return null;
                }
            }
        }
        static string N(float x) => x.ToString("R", Inv);

        sealed class VectorConverter : JsonConverter
        {
            public override bool CanConvert(Type t) => t == typeof(Vector3) || t == typeof(Quaternion);
            public override bool CanRead => false;
            public override object ReadJson(JsonReader r, Type t, object old, JsonSerializer s) => throw new NotSupportedException();
            public override void WriteJson(JsonWriter w, object value, JsonSerializer s)
            {
                w.WriteStartObject();
                if (value is Vector3 v) { w.WritePropertyName("x"); w.WriteValue(v.x); w.WritePropertyName("y"); w.WriteValue(v.y); w.WritePropertyName("z"); w.WriteValue(v.z); }
                else if (value is Quaternion q) { w.WritePropertyName("x"); w.WriteValue(q.x); w.WritePropertyName("y"); w.WriteValue(q.y); w.WritePropertyName("z"); w.WriteValue(q.z); w.WritePropertyName("w"); w.WriteValue(q.w); }
                w.WriteEndObject();
            }
        }
    }
}
