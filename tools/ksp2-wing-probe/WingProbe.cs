using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Reflection;
using BepInEx;
using KSP.Game;
using KSP.IO;
using KSP.Modules;
using KSP.OAB;
using KSP.Sim;
using KSP.Sim.impl;
using KSP.Sim.Definitions;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // Procedural wing reference for OSE (docs/ksp2-feature-gap-analysis.md G04): what the original computes for a
    // shaped wing. Does nothing unless BepInEx/ose-wings/run.txt exists (kept apart from the parity pilot's
    // BepInEx/ose-parity/run.txt, which must be absent while this runs). Then: open the newest save of the
    // "OSE Parity" campaign, load BepInEx/ose-wings/craft.json in the VAB, dump every procedural part, reshape
    // parts with the lines of shapes.txt ("<part name> KEY=value ..." with OPTION=n for the mesh option) through
    // the editor's own slider API and dump after each rebake, launch, dump the parts in flight and quit.
    // Each dump row: mass modifier, lift and control areas, centre of pressure, flap pivot frame, and the lift
    // the original's Module_LiftingSurface.GetLiftVector gives at fixed angles of attack.
    [BepInPlugin("org.openspaceexplorer.wingprobe", "OSE Wing Probe", "0.1.0")]
    public sealed class WingProbe : BaseUnityPlugin
    {
        const string Campaign = "OSE Parity";
        string dir;
        StreamWriter rows;

        void Awake()
        {
            dir = Path.Combine(Paths.GameRootPath, "BepInEx", "ose-wings");
            if (!File.Exists(Path.Combine(dir, "run.txt"))) { Logger.LogInfo("OSE wing probe idle (no run.txt)."); return; }
            Directory.CreateDirectory(Path.Combine(dir, "out"));
            StartCoroutine(Run());
        }

        static GameInstance G => GameManager.Instance?.Game;
        static GameState State => G?.GlobalGameState?.GetState() ?? GameState.Invalid;
        static string F(double v) => v.ToString("R", CultureInfo.InvariantCulture);
        static string V(Vector3 v) => F(v.x) + " " + F(v.y) + " " + F(v.z);
        static string Q(Quaternion q) => F(q.x) + " " + F(q.y) + " " + F(q.z) + " " + F(q.w);

        void Mark(string s)
        {
            Logger.LogInfo("[wings] " + s);
            File.AppendAllText(Path.Combine(dir, "out", "steps.log"), DateTime.UtcNow.ToString("HH:mm:ss.fff", CultureInfo.InvariantCulture) + " " + s + "\n");
        }

        IEnumerator WaitState(GameState s, float timeout)
        {
            float t = 0;
            while (State != s && t < timeout) { t += Time.unscaledDeltaTime; yield return null; }
            if (State != s) throw new Exception("timed out waiting for " + s + " (now " + State + ")");
        }

        IEnumerator Shot(string name)
        {
            yield return new WaitForEndOfFrame();
            ScreenCapture.CaptureScreenshot(Path.Combine(dir, "out", name + ".png"));
            yield return new WaitForSeconds(0.5f);
        }

        IEnumerator Run()
        {
            Mark("start");
            float t = 0;
            while (State != GameState.MainMenu && t < 600) { t += Time.unscaledDeltaTime; yield return null; }
            yield return new WaitForSeconds(8f);
            var slm = G.SaveLoadManager;
            var campaignDir = slm.GetCampaignFolderPath(CampaignType.SinglePlayer, Campaign);
            string path = null;
            DateTime newest = DateTime.MinValue;
            if (Directory.Exists(campaignDir))
                foreach (var f in Directory.GetFiles(campaignDir, "*.json"))
                    if (File.GetLastWriteTimeUtc(f) > newest) { newest = File.GetLastWriteTimeUtc(f); path = f; }
            if (path == null) { Mark("no " + Campaign + " save; run the parity pilot once first"); Application.Quit(); yield break; }
            Mark("loading " + path);
            slm.LoadGameFromFile(path, (_, ok) => Mark("load finished " + ok));
            yield return WaitState(GameState.KerbalSpaceCenter, 600);
            yield return new WaitForSeconds(15f);

            var snapshot = IOProvider.FromJsonFile<OABHistoricalSnapshot>(Path.Combine(dir, "craft.json"));
            bool shown = false;
            G.OAB.ShowOAB(new OABConfig(OABVariant.VAB, OABEnvironmentType.Terrestrial, OABConstructionType.User, "Kerbin"), snapshot, null, () => shown = true);
            t = 0;
            while (!shown && t < 300) { t += Time.unscaledDeltaTime; yield return null; }
            yield return new WaitForSeconds(15f);
            Mark("vab");
            yield return Shot("vab-stock");

            rows = new StreamWriter(Path.Combine(dir, "out", "wings.csv"), false);
            rows.WriteLine("label,index,part,mirrored,option,sliders,mass_t,mass_modifier_t,lifting_ctrl_area,ctrl_area,deflection_lift_coeff,cop_offset,ctrl_pivot_offset,ctrl_cop_offset,part_rot,pivot_rot_in_part,lift_5deg_n,lift_10deg_n,ctrl_active,measurements");
            DumpOab("stock");

            var shapes = Path.Combine(dir, "shapes.txt");
            if (File.Exists(shapes))
            {
                int n = 0;
                foreach (var raw in File.ReadAllLines(shapes))
                {
                    var line = raw.Trim();
                    if (line.Length == 0 || line.StartsWith("#")) continue;
                    n++;
                    yield return Reshape(line, "shape" + n);
                }
            }
            rows.Flush();

            G.OAB.Current.eventsUI.OnLaunch?.Invoke();
            yield return WaitState(GameState.FlightView, 600);
            VesselComponent vessel = null;
            t = 0;
            while ((vessel = G.ViewController.GetActiveSimVessel()) == null && t < 120) { t += Time.unscaledDeltaTime; yield return null; }
            yield return new WaitForSeconds(12f);
            DumpFlight(vessel);
            rows.Close();
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            Mark("done");
            yield return new WaitForSeconds(2f);
            Application.Quit();
        }

        static List<ObjectAssemblyPart> OabParts()
        {
            var list = new List<ObjectAssemblyPart>();
            foreach (var p in G.OAB.Current.Stats.MainAssembly.Parts)
                if (p is ObjectAssemblyPart oap) list.Add(oap);
            return list;
        }

        IEnumerator Reshape(string line, string label)
        {
            var words = line.Split(new[] { ' ', '\t' }, StringSplitOptions.RemoveEmptyEntries);
            ObjectAssemblyPart target = null;
            foreach (var p in OabParts()) if (p.Name == words[0] && !p.IsSymmetryMirrored) { target = p; break; }
            if (target == null) { Mark(label + ": no part " + words[0]); yield break; }
            var proc = target.PartTransform.GetComponentInChildren<Module_ProceduralPart>();
            var baker = proc?.ProceduralMeshBaker;
            if (baker == null) { Mark(label + ": " + words[0] + " has no mesh baker"); yield break; }
            baker.SetMeshVisibility(ProcedurallyEditablePart.VisibilityMode.DynamicMesh);
            yield return new WaitForSeconds(1f);
            for (int i = 1; i < words.Length; i++)
            {
                var kv = words[i].Split('=');
                if (kv.Length != 2) continue;
                float v = float.Parse(kv[1], CultureInfo.InvariantCulture);
                if (kv[0] == "OPTION") { baker.SelectedMeshOptionIndex = (int)v; yield return new WaitForSeconds(2f); }
                else baker.SetMeshSliderValue(kv[0], v);
            }
            yield return new WaitForSeconds(2f);
            baker.SetMeshVisibility(ProcedurallyEditablePart.VisibilityMode.StaticMesh);
            float t = 0;
            while (!baker.IsMeshClean && t < 30) { t += Time.unscaledDeltaTime; yield return null; }
            yield return new WaitForSeconds(2f);
            proc.ReadValuesFromMeshBaker();
            var cs = target.PartTransform.GetComponentInChildren<Module_ControlSurface>();
            cs?.GetType().GetMethod("MeshDataChanged", BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public)?.Invoke(cs, null);
            yield return new WaitForSeconds(1f);
            Mark(label + " " + line + " clean " + baker.IsMeshClean);
            yield return Shot("vab-" + label);
            DumpOab(label);
        }

        static T Data<T>(PartBehaviourModule m) where T : ModuleData
        {
            if (m == null) return null;
            return m.DataModules.TryGetByType<T>(out var d) ? d : null;
        }

        void DumpOab(string label)
        {
            int i = 0;
            foreach (var p in OabParts())
            {
                var root = p.PartTransform;
                var proc = root.GetComponentInChildren<Module_ProceduralPart>();
                if (proc == null) { i++; continue; }
                var pp = Data<Data_ProceduralPart>(proc);
                var lift = root.GetComponentInChildren<Module_LiftingSurface>();
                var ls = lift?.DataLiftingSurface;
                var cd = Data<Data_ControlSurface>(lift);
                double mass = p.DryMass;
                Row(label, i, p.Name, p.IsSymmetryMirrored, proc, pp, ls, cd, lift, mass, root);
                i++;
            }
            rows.Flush();
        }

        void Row(string label, int i, string name, bool mirrored, Module_ProceduralPart proc, Data_ProceduralPart pp, Data_LiftingSurface ls, Data_ControlSurface cd, Module_LiftingSurface lift, double mass, Transform root)
        {
            var sliders = new List<string>();
            if (pp != null) foreach (var kv in pp.sliderData) sliders.Add(kv.Key + "=" + F(kv.Value));
            var meas = new List<string>();
            var baker = proc?.ProceduralMeshBaker;
            if (baker != null) foreach (var kv in baker.SliderValueMeasurementStorage) meas.Add(kv.Key + "=" + F(kv.Value));
            string pivotRot = "";
            if (cd != null && baker?.BakedMeshObject != null)
            {
                var pivot = baker.BakedMeshObject.transform.FindChildRecursive(cd.CtrlSurfacePivotTransformName);
                if (pivot != null) pivotRot = Q(Quaternion.Inverse(root.rotation) * pivot.rotation) + " at " + V(root.InverseTransformPoint(pivot.position));
            }
            string l5 = "", l10 = "";
            if (lift != null && ls != null)
            {
                // The lift axis is the part's -X; flow at a fixed angle of attack, 1 kPa, Mach 0.3.
                foreach (var aoa in new[] { 5.0, 10.0 })
                {
                    float s = (float)Math.Sin(aoa * Math.PI / 180.0);
                    var nv = new Vector3(s, (float)Math.Cos(aoa * Math.PI / 180.0), 0f);
                    var v = lift.GetLiftVector(Vector3.left, -s, s, 1000.0, 0.3f, ls.deflectionLiftCoeff, nv);
                    if (aoa == 5.0) l5 = F(v.magnitude); else l10 = F(v.magnitude);
                }
            }
            rows.WriteLine(string.Join(",", new[] {
                label, i.ToString(CultureInfo.InvariantCulture), name, mirrored ? "1" : "0",
                pp != null ? pp.meshOptionIndex.ToString(CultureInfo.InvariantCulture) : "",
                string.Join(" ", sliders), F(mass), pp != null ? F(pp.MassModifierAmount) : "",
                cd != null ? F(cd.LiftingCtrlSurfaceArea) : "", cd != null ? F(cd.CtrlSurfaceArea) : "",
                ls != null ? F(ls.deflectionLiftCoeff) : "", ls != null ? V(ls.CoPOffset) : "",
                cd != null ? V(cd.CtrlSurfacePivotOffset) : "", cd != null ? V(cd.CoPressureCtrlSurfaceOffset) : "",
                Q(root.rotation), pivotRot, l5, l10, cd != null && cd.IsCtrlSurfaceActive ? "1" : "0", string.Join(" ", meas) }));
        }

        void DumpFlight(VesselComponent vessel)
        {
            int i = 0;
            foreach (var part in vessel.SimulationObject.PartOwner.Parts)
            {
                if (G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view))
                {
                    var proc = view.GetComponentInChildren<Module_ProceduralPart>();
                    if (proc != null)
                    {
                        var lift = view.GetComponentInChildren<Module_LiftingSurface>();
                        Row("flight", i, part.Name, false, proc, Data<Data_ProceduralPart>(proc), lift?.DataLiftingSurface, Data<Data_ControlSurface>(lift), lift, part.DryMass, view.transform);
                    }
                }
                i++;
            }
            rows.Flush();
        }
    }
}
