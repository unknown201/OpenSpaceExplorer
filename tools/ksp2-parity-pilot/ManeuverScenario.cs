using System;
using System.Collections;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text;
using KSP.Game;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // Maneuver panel reference (docs/ui-parity-ksp2.md M14): in a 100 km Kerbin orbit, plan a small node the stages
    // can fly, then a second node far beyond the vessel's delta-v, and record how KSP2 shows them: screenshots in
    // flight and on the map, every visible UI text (ui-*.txt), and the nodes' own fields (nodes-*.txt: burn
    // duration and required delta-v as KSP2 computes them) next to the vessel's delta-v (deltav-*.txt).
    // The maneuver API is reached by reflection and every lookup is logged, so a name that differs in this build
    // shows in steps.log rather than failing the build.
    public static class ManeuverScenario
    {
        static GameInstance G => GameManager.Instance.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        const BindingFlags Pub = BindingFlags.Public | BindingFlags.Instance;

        public static IEnumerator Run(VesselComponent vessel, string dir, Action<string> mark, Func<string, IEnumerator> shot)
        {
            yield return OrbitScenarios.ToOrbit(vessel, "Kerbin", 700000, 0, mark);
            vessel = G.ViewController.GetActiveSimVessel();
            vessel.SetAutopilotEnableDisable(true);
            yield return new WaitForSeconds(5f);
            DumpDeltaV(dir, "deltav-start.txt", vessel);
            Api(dir, vessel);
            yield return shot("m00-orbit-flight");

            // KSP2 makes a node's gizmo as it is added, which needs the map: plan with the map open
            double ut = G.UniverseModel.UniverseTime;
            bool map = Map("EnableFlightMap", mark);
            yield return new WaitForSeconds(4f);
            object small = AddNode(vessel, ut + 600, 50, mark);
            yield return new WaitForSeconds(8f);
            DumpNodes(dir, "nodes-small.txt", vessel, mark);
            Texts(dir, "ui-small-map.txt");
            yield return shot("m01-small-map");
            Map("DisableMap", mark);
            yield return new WaitForSeconds(4f);
            Texts(dir, "ui-small-flight.txt");
            yield return shot("m02-small-flight");

            // far more than any stock stage holds: the second node, after the first
            Map("EnableFlightMap", mark);
            yield return new WaitForSeconds(4f);
            object big = AddNode(vessel, ut + 1500, 20000, mark);
            yield return new WaitForSeconds(8f);
            DumpNodes(dir, "nodes-short.txt", vessel, mark);
            Texts(dir, "ui-short-map.txt");
            yield return shot("m03-short-map");
            Map("DisableMap", mark);
            yield return new WaitForSeconds(4f);
            Texts(dir, "ui-short-flight.txt");
            yield return shot("m04-short-flight");
            DumpDeltaV(dir, "deltav-end.txt", vessel);
            mark("maneuver scenario done (small node " + (small != null) + ", short node " + (big != null) + ", map " + map + ")");
        }

        static object Maneuvers(Action<string> mark)
        {
            var sim = G.SpaceSimulation;
            var m = sim?.GetType().GetProperty("Maneuvers", Pub)?.GetValue(sim);
            if (m == null) mark("no SpaceSimulation.Maneuvers");
            return m;
        }

        static Type NodeType => typeof(VesselComponent).Assembly.GetTypes().FirstOrDefault(t => t.Name == "ManeuverNodeData");

        static string Members(Type t) => t == null ? "null" : t.FullName + "\n" + string.Join("\n",
            t.GetProperties(Pub).Select(p => "  prop " + p.PropertyType.Name + " " + p.Name + (p.CanWrite ? " (set)" : ""))
            .Concat(t.GetFields(Pub).Select(f => "  field " + f.FieldType.Name + " " + f.Name))
            .Concat(t.GetMethods(Pub).Where(m => !m.IsSpecialName).Select(m => "  method " + m.ReturnType.Name + " " + m.Name + "(" + string.Join(",", m.GetParameters().Select(x => x.ParameterType.Name)) + ")")));

        // What this build offers for maneuvers: the node type, the maneuver store, and the vessel's maneuver members.
        static void Api(string dir, VesselComponent vessel)
        {
            var sb = new StringBuilder();
            try
            {
                sb.AppendLine(Members(NodeType));
                sb.AppendLine(Members(G.SpaceSimulation?.GetType().GetProperty("Maneuvers", Pub)?.PropertyType));
                var so = vessel.SimulationObject;
                foreach (var p in so.GetType().GetProperties(Pub).Where(p => p.Name.Contains("Maneuver")))
                    sb.AppendLine("SimulationObject." + p.Name + ": " + Members(p.PropertyType));
                foreach (var p in vessel.GetType().GetProperties(Pub).Where(p => p.Name.Contains("Maneuver")))
                    sb.AppendLine("VesselComponent." + p.Name + ": " + Members(p.PropertyType));
            }
            catch (Exception e) { sb.AppendLine("failed: " + e); }
            File.WriteAllText(Path.Combine(dir, "out", "maneuver-api.txt"), sb.ToString());
        }

        static object AddNode(VesselComponent vessel, double ut, double prograde, Action<string> mark)
        {
            var type = NodeType;
            if (type == null) { mark("no ManeuverNodeData type"); return null; }
            var ctor = type.GetConstructors().FirstOrDefault(c => c.GetParameters().Length == 3);
            if (ctor == null) return null;
            object node;
            try { node = ctor.Invoke(new object[] { vessel.SimulationObject.GlobalId, false, ut }); }
            catch (Exception e) { mark("ManeuverNodeData ctor failed: " + (e.InnerException ?? e).Message); return null; }
            // KSP2's burn vector: x radial, y normal, z prograde (a property or a field, by build)
            var prop = type.GetProperty("BurnVector", Pub);
            var field = type.GetField("BurnVector", Pub);
            var vt = prop?.PropertyType ?? field?.FieldType;
            if (vt == null) mark("no ManeuverNodeData.BurnVector (see maneuver-api.txt)");
            else
                try
                {
                    var v = Activator.CreateInstance(vt, 0.0, 0.0, prograde);
                    if (prop != null && prop.CanWrite) prop.SetValue(node, v); else field?.SetValue(node, v);
                    mark("BurnVector set via " + (prop != null && prop.CanWrite ? "property" : "field"));
                }
                catch (Exception e) { mark("BurnVector failed: " + (e.InnerException ?? e).Message); }
            // the node's place on the orbit, which UpdateNodeDetails needs (it threw a null reference without it)
            Call(node, "SetManeuverState", mark, vessel.Orbit);
            Call(node, "InitializeTransform", mark);
            var m = Maneuvers(mark);
            var add = m?.GetType().GetMethods(Pub).FirstOrDefault(x => x.Name == "AddNodeToVessel" && x.GetParameters().Length == 1);
            if (add != null)
                try { add.Invoke(m, new[] { node }); mark("node via AddNodeToVessel at UT " + ut.ToString("F0", Inv) + ", prograde " + prograde.ToString("F0", Inv)); Settle(m, node, mark); return node; }
                catch (Exception e) { mark("AddNodeToVessel failed: " + (e.InnerException ?? e).GetType().Name + " " + (e.InnerException ?? e).Message + "\n" + (e.InnerException ?? e).StackTrace); }
            // the vessel's own maneuver plan
            var so = vessel.SimulationObject;
            foreach (var p in so.GetType().GetProperties(Pub).Where(p => p.Name.Contains("Maneuver")))
            {
                object plan;
                try { plan = p.GetValue(so); } catch { continue; }
                if (plan == null) { mark("SimulationObject." + p.Name + " is null"); continue; }
                foreach (var am in plan.GetType().GetMethods(Pub).Where(x => x.Name == "AddNode" && x.GetParameters().Length >= 1 && x.GetParameters()[0].ParameterType.IsAssignableFrom(type)))
                {
                    // a flag that clears or replaces the other nodes stays off (the first node went when the second came)
                    var args = am.GetParameters().Select((x, i) => i == 0 ? node
                        : x.ParameterType == typeof(bool) ? (object)!(x.Name.IndexOf("clear", StringComparison.OrdinalIgnoreCase) >= 0 || x.Name.IndexOf("replace", StringComparison.OrdinalIgnoreCase) >= 0 || x.Name.IndexOf("remove", StringComparison.OrdinalIgnoreCase) >= 0)
                        : x.HasDefaultValue ? x.DefaultValue : null).ToArray();
                    mark(p.Name + ".AddNode(" + string.Join(", ", am.GetParameters().Select((x, i) => x.ParameterType.Name + " " + x.Name + "=" + (i == 0 ? "node" : Convert.ToString(args[i], Inv)))) + ")");
                    try { am.Invoke(plan, args); mark("node via " + p.Name + ".AddNode at UT " + ut.ToString("F0", Inv) + ", prograde " + prograde.ToString("F0", Inv)); Settle(plan, node, mark); Settle(m, node, mark); return node; }
                    catch (Exception e) { mark(p.Name + ".AddNode failed: " + (e.InnerException ?? e).GetType().Name + " " + (e.InnerException ?? e).Message + "\n" + (e.InnerException ?? e).StackTrace); }
                }
            }
            return null;
        }

        // Ask KSP2 to work the node out (burn delta-v and duration): UpdateChangeOnNode with its own burn vector,
        // then RefreshManeuverNodeState for each node index, whichever this object has.
        static void Settle(object owner, object node, Action<string> mark)
        {
            if (owner == null) return;
            var burn = node.GetType().GetField("BurnVector", Pub)?.GetValue(node);
            foreach (var u in owner.GetType().GetMethods(Pub).Where(x => x.Name == "UpdateChangeOnNode"))
            {
                var ps = u.GetParameters();
                var args = ps.Select(x => x.ParameterType.IsInstanceOfType(node) ? node : burn != null && x.ParameterType.IsInstanceOfType(burn) ? burn : x.HasDefaultValue ? x.DefaultValue : null).ToArray();
                try { u.Invoke(owner, args); mark(owner.GetType().Name + ".UpdateChangeOnNode(" + string.Join(",", ps.Select(x => x.ParameterType.Name)) + ") ok"); }
                catch (Exception e) { mark(owner.GetType().Name + ".UpdateChangeOnNode(" + string.Join(",", ps.Select(x => x.ParameterType.Name)) + ") failed: " + (e.InnerException ?? e).GetType().Name + " " + (e.InnerException ?? e).Message); }
            }
            var refresh = owner.GetType().GetMethods(Pub).FirstOrDefault(x => x.Name == "RefreshManeuverNodeState" && x.GetParameters().Length == 1);
            if (refresh != null)
                for (int i = 0; i < 2; i++)
                    try { refresh.Invoke(owner, new object[] { i }); }
                    catch (Exception e) { mark("RefreshManeuverNodeState(" + i + ") failed: " + (e.InnerException ?? e).Message); }
        }

        static void Call(object o, string name, Action<string> mark, params object[] args)
        {
            var m = o.GetType().GetMethods(Pub).FirstOrDefault(x => x.Name == name && x.GetParameters().Length == args.Length);
            if (m == null) { mark("no " + o.GetType().Name + "." + name); return; }
            try { m.Invoke(o, args); mark(o.GetType().Name + "." + name + " ok"); }
            catch (Exception e) { mark(o.GetType().Name + "." + name + " failed: " + (e.InnerException ?? e).GetType().Name + " " + (e.InnerException ?? e).Message); }
        }

        static bool Map(string name, Action<string> mark)
        {
            var map = G.GetType().GetProperty("Map", Pub)?.GetValue(G);
            var m = map?.GetType().GetMethods(Pub).FirstOrDefault(x => x.Name == name && x.GetParameters().All(p => p.HasDefaultValue));
            if (m == null) { mark("no Map." + name); return false; }
            try { m.Invoke(map, m.GetParameters().Select(p => p.DefaultValue).ToArray()); mark("map: " + name); return true; }
            catch (Exception e) { mark("map " + name + " failed: " + (e.InnerException ?? e).Message); return false; }
        }

        static void DumpNodes(string dir, string file, VesselComponent vessel, Action<string> mark)
        {
            var m = Maneuvers(mark);
            var get = m?.GetType().GetMethods(Pub).FirstOrDefault(x => x.Name == "GetNodesForVessel" && x.GetParameters().Length == 1);
            IEnumerable list = null;
            try { list = get?.Invoke(m, new object[] { vessel.SimulationObject.GlobalId }) as IEnumerable; }
            catch (Exception e) { mark("GetNodesForVessel failed: " + (e.InnerException ?? e).Message); }
            var sb = new StringBuilder();
            int i = 0;
            if (list != null) foreach (var n in list) { if (i >= 8 || n == null) break; sb.AppendLine("# node " + i++); Props(sb, n, 0, "  "); }
            else sb.AppendLine("no GetNodesForVessel");
            File.WriteAllText(Path.Combine(dir, "out", file), sb.ToString());
            mark(file + ": " + i + " nodes");
        }

        // Every active TextMeshPro text on screen with its hierarchy path.
        static void Texts(string dir, string file)
        {
            try { TextsUnsafe(dir, file); }
            catch (Exception e) { File.WriteAllText(Path.Combine(dir, "out", file), "failed: " + e); }
        }

        static void TextsUnsafe(string dir, string file)
        {
            var tmp = AppDomain.CurrentDomain.GetAssemblies().Select(a => a.GetType("TMPro.TMP_Text")).FirstOrDefault(t => t != null);
            var sb = new StringBuilder();
            if (tmp == null) sb.AppendLine("no TMPro.TMP_Text");
            else
                foreach (var o in UnityEngine.Object.FindObjectsOfType(tmp))
                {
                    var c = o as Component;
                    if (c == null || !c.gameObject.activeInHierarchy) continue;
                    var text = tmp.GetProperty("text")?.GetValue(o) as string;
                    if (string.IsNullOrWhiteSpace(text)) continue;
                    var path = c.transform.name;
                    for (var p = c.transform.parent; p != null; p = p.parent) path = p.name + "/" + path;
                    sb.AppendLine(path + "\t" + text.Replace("\n", "\\n"));
                }
            File.WriteAllText(Path.Combine(dir, "out", file), sb.ToString());
        }

        // The vessel's delta-v by stage, read by name (a recursive walk of VesselDeltaV hung KSP2: some getters
        // compute or loop): only these fields, each read on its own.
        static readonly string[] TotalFields = { "TotalDeltaVActual", "TotalDeltaVVac", "TotalDeltaVASL", "TotalBurnTime", "TotalMass", "StartMass", "EndMass" };
        static readonly string[] StageFields = { "Stage", "DeltaVActual", "DeltaVinVac", "DeltaVatASL", "StageBurnTime", "StartMass", "EndMass", "ThrustActual", "ThrustVac", "IspActual", "IspVac", "TWRActual" };

        static void DumpDeltaV(string dir, string file, VesselComponent vessel)
        {
            var sb = new StringBuilder();
            object dv = Get(vessel, "VesselDeltaV");
            if (dv == null) sb.AppendLine("no VesselDeltaV");
            else
            {
                foreach (var f in TotalFields) Flat(sb, dv, f, "");
                if (Get(dv, "StageInfo") is IEnumerable stages && !(stages is string))
                {
                    int i = 0;
                    foreach (var st in stages)
                    {
                        if (i >= 12 || st == null) break;
                        sb.AppendLine("stage[" + i++ + "]");
                        foreach (var f in StageFields) Flat(sb, st, f, "  ");
                    }
                }
                else sb.AppendLine("no StageInfo");
            }
            File.WriteAllText(Path.Combine(dir, "out", file), sb.ToString());
        }

        static object Get(object o, string name)
        {
            try { return o?.GetType().GetProperty(name, Pub)?.GetValue(o) ?? o?.GetType().GetField(name, Pub)?.GetValue(o); }
            catch (Exception e) { return "<" + (e.InnerException ?? e).GetType().Name + ">"; }
        }

        // One field or property when it is a number, flag, enum, string or small struct (Vector3d): never walked.
        static void Flat(StringBuilder sb, object o, string name, string indent)
        {
            var v = Get(o, name);
            if (v == null) return;
            var t = v.GetType();
            if (v is string || t.IsPrimitive || t.IsEnum || t.IsValueType)
            {
                string text;
                try { text = v is IFormattable f ? f.ToString(null, Inv) : v.ToString(); } catch (Exception e) { text = "<" + e.GetType().Name + ">"; }
                sb.AppendLine(indent + name + " = " + text);
            }
        }

        // A node's own fields: every simple public property and field, one level, no getters that return objects.
        static void Props(StringBuilder sb, object o, int depth, string indent)
        {
            var t = o.GetType();
            foreach (var p in t.GetProperties(Pub))
            {
                if (p.GetIndexParameters().Length > 0) continue;
                var pt = p.PropertyType;
                if (pt == typeof(string) || pt.IsPrimitive || pt.IsEnum || pt.IsValueType) Flat(sb, o, p.Name, indent);
            }
            foreach (var f in t.GetFields(Pub))
            {
                var ft = f.FieldType;
                if (ft == typeof(string) || ft.IsPrimitive || ft.IsEnum || ft.IsValueType) Flat(sb, o, f.Name, indent);
            }
        }
    }
}
