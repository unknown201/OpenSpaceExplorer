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
            yield return shot("m00-orbit-flight");

            double ut = G.UniverseModel.UniverseTime;
            object small = AddNode(vessel, ut + 600, 50, mark);
            yield return new WaitForSeconds(4f);
            DumpNodes(dir, "nodes-small.txt", vessel, mark);
            Texts(dir, "ui-small-flight.txt");
            yield return shot("m01-small-flight");
            bool map = ToggleMap(dir, mark);
            yield return new WaitForSeconds(4f);
            Texts(dir, "ui-small-map.txt");
            yield return shot("m02-small-map");

            // far more than any stock stage holds: the second node, after the first
            object big = AddNode(vessel, ut + 1500, 20000, mark);
            yield return new WaitForSeconds(4f);
            DumpNodes(dir, "nodes-short.txt", vessel, mark);
            Texts(dir, "ui-short-map.txt");
            yield return shot("m03-short-map");
            if (map) { ToggleMap(dir, mark); yield return new WaitForSeconds(4f); }
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

        static object AddNode(VesselComponent vessel, double ut, double prograde, Action<string> mark)
        {
            var type = typeof(VesselComponent).Assembly.GetTypes().FirstOrDefault(t => t.Name == "ManeuverNodeData");
            if (type == null) { mark("no ManeuverNodeData type"); return null; }
            var id = vessel.SimulationObject.GlobalId;
            var ctor = type.GetConstructors().FirstOrDefault(c => c.GetParameters().Length == 3);
            mark("ManeuverNodeData ctors: " + string.Join(" | ", type.GetConstructors().Select(c => string.Join(",", c.GetParameters().Select(p => p.ParameterType.Name + " " + p.Name)))));
            if (ctor == null) return null;
            object node;
            try { node = ctor.Invoke(new object[] { id, false, ut }); }
            catch (Exception e) { mark("ManeuverNodeData ctor failed: " + e.InnerException?.Message); return null; }
            var burn = type.GetProperty("BurnVector", Pub);
            if (burn != null)
            {
                // KSP2's burn vector: x radial, y normal, z prograde
                var v = Activator.CreateInstance(burn.PropertyType, 0.0, 0.0, prograde);
                burn.SetValue(node, v);
            }
            else mark("no ManeuverNodeData.BurnVector");
            var m = Maneuvers(mark);
            var add = m?.GetType().GetMethods(Pub).FirstOrDefault(x => x.Name == "AddNodeToVessel" && x.GetParameters().Length == 1);
            if (add == null) { mark("no AddNodeToVessel: " + string.Join(",", m?.GetType().GetMethods(Pub).Select(x => x.Name).Distinct() ?? new string[0])); return null; }
            try { add.Invoke(m, new[] { node }); }
            catch (Exception e) { mark("AddNodeToVessel failed: " + e.InnerException?.Message); return null; }
            mark("node at UT " + ut.ToString("F0", Inv) + ", prograde " + prograde.ToString("F0", Inv) + " m/s");
            return node;
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

        // The map toggle: whatever this build calls it, on the game's Map object (every method name goes to map-api.txt).
        static bool ToggleMap(string dir, Action<string> mark)
        {
            var map = G.GetType().GetProperty("Map", Pub)?.GetValue(G);
            if (map == null) { mark("no Game.Map"); return false; }
            var methods = map.GetType().GetMethods(Pub);
            File.WriteAllText(Path.Combine(dir, "out", "map-api.txt"),
                string.Join("\n", methods.Select(x => x.ReturnType.Name + " " + x.Name + "(" + string.Join(",", x.GetParameters().Select(p => p.ParameterType.Name)) + ")")));
            foreach (var name in new[] { "ToggleMap", "ToggleMapView", "OpenMap", "ShowMap", "EnterMapView" })
            {
                var m = methods.FirstOrDefault(x => x.Name == name && x.GetParameters().Length == 0);
                if (m == null) continue;
                try { m.Invoke(map, null); mark("map: " + name); return true; }
                catch (Exception e) { mark("map " + name + " failed: " + e.InnerException?.Message); }
            }
            mark("no map toggle found (see map-api.txt)");
            return false;
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
