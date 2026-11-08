using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Reflection;
using KSP.Game;
using KSP.Modules;
using KSP.Sim;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // T18 docking (docs/ksp2-side-by-side-gaps.md), on craft-docking.json (make_craft_docking.py): two halves
    // joined port to port in the VAB, launched into orbit.
    //   undock-vab   undock the VAB-joined ports (KSP2 decouples them with the port's ejection force), drift 6 s
    //   calibrate    which RCS translation axis moves the active half along its port's axis
    //   back-off     open the gap to 4 m, hold it
    //   approach-0.2 close at 0.2 m/s to 2 m, then coast in: below the 0.3 m/s capture speed
    //   docked       6 s docked: merged mass and centre of mass
    //   undock-dock  undock again (the flight-docked path: a half-ejection force on each port), drift 6 s
    //   approach-0.6 back off to 4 m, close at 0.6 m/s to 2 m, then coast: above the capture speed
    // Every physics tick: the gap between the two ports' docking transforms, closing speed, lateral offset,
    // facing, both ports' docking states, whether they share a vessel, the active vessel's mass, centre of mass
    // in port A's frame. Only inputs and the ports' own undock action are used; physics is never modified.
    public static class DockingScenario
    {
        const BindingFlags Priv = BindingFlags.NonPublic | BindingFlags.Instance;
        static GameInstance G => GameManager.Instance?.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;

        sealed class Port
        {
            public PartComponent Part;
            public Module_DockingNode Module;
            public Transform Face;
        }

        static bool Resolve(Port p)
        {
            if (p.Part?.SimulationObject == null || !G.SpaceSimulation.TryGetViewObject(p.Part.SimulationObject, out var view)) return false;
            var behavior = view.GetComponent<PartBehavior>();
            if (behavior == null) return false;
            p.Module = behavior.GetModule<Module_DockingNode>();
            p.Face = p.Module == null ? null : (Transform)typeof(Module_DockingNode).GetMethod("GetDockingTransform", Priv).Invoke(p.Module, null);
            return p.Face != null;
        }

        static string State(Port p) =>
            p.Part != null && p.Part.TryGetModule<PartComponentModule_DockingNode>(out var m) ? m.GetCurrentState().ToString() : "?";

        static VesselComponent VesselOf(Port p) => p.Part?.PartOwner?.SimulationObject?.Vessel;

        static Vector3 CentreOfMass(VesselComponent v, out double mass)
        {
            Vector3 sum = Vector3.zero;
            mass = 0;
            foreach (var part in v.SimulationObject.PartOwner.Parts)
            {
                if (!G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) continue;
                double m = part.TotalMass;
                sum += view.transform.position * (float)m;
                mass += m;
            }
            return mass > 0 ? sum / (float)mass : Vector3.zero;
        }

        static void RcsOn(VesselComponent v)
        {
            if (G.ViewController.TryGetActiveVehicle(out var veh)) veh.ControllableActionGroups?.SetGroup(KSPActionGroup.RCS, true);
            v.IsRCSEnabled = true;
        }

        public static IEnumerator Run(VesselComponent vessel, string dir, Action<string> mark)
        {
            var ports = new List<Port>();
            foreach (var part in vessel.SimulationObject.PartOwner.Parts)
                if (part.Name == "dockingport_1v_inline") ports.Add(new Port { Part = part });
            mark("ports " + ports.Count);
            if (ports.Count != 2) yield break;
            // port A is the active half's: nearer the root (the cupola)
            PartComponent root = null;
            foreach (var part in vessel.SimulationObject.PartOwner.Parts) { root = part; break; }
            G.SpaceSimulation.TryGetViewObject(root.SimulationObject, out var rootView);
            foreach (var p in ports) Resolve(p);
            if ((ports[1].Face.position - rootView.transform.position).sqrMagnitude < (ports[0].Face.position - rootView.transform.position).sqrMagnitude)
                ports.Reverse();
            Port a = ports[0], b = ports[1];
            var activeId = vessel.GlobalId;
            mark("port A " + a.Part.GlobalId + " state " + State(a) + ", port B " + b.Part.GlobalId + " state " + State(b) + ", mass " + vessel.totalMass.ToString("R", Inv));

            vessel.SetAutopilotEnableDisable(true);
            RcsOn(vessel);
            yield return new WaitForSeconds(3f);
            mark("rcs " + vessel.IsRCSEnabled + " group " + vessel.GetActionGroupState(KSPActionGroup.RCS));

            var w = new StreamWriter(Path.Combine(dir, "out", "docking-trace.csv"), false);
            w.WriteLine("t,phase,gap_m,closing_mps,lateral_m,facing_dot,state_a,state_b,same_vessel,active_mass_t,a_mass_t,b_mass_t,com_x,com_y,com_z,input");
            string phase = "settle";
            float t = 0, input = 0;
            Vector3 lastRel = Vector3.zero;
            bool haveLast = false;
            float closing = 0, gap = 0;
            Vector3 fwd = Vector3.up;

            // one physics tick: measure and log
            IEnumerator Tick()
            {
                yield return new WaitForFixedUpdate();
                t += Time.fixedDeltaTime;
                if (!Resolve(a) || !Resolve(b)) { w.WriteLine(t.ToString("F3", Inv) + "," + phase + ",,,,,,,,,,,,,,"); haveLast = false; yield break; }
                var rel = b.Face.position - a.Face.position;
                fwd = a.Face.forward;
                gap = rel.magnitude;
                if (haveLast) closing = -Vector3.Dot(rel - lastRel, gap > 1e-4f ? rel / gap : fwd) / Time.fixedDeltaTime;
                lastRel = rel; haveLast = true;
                var lateral = (rel - fwd * Vector3.Dot(rel, fwd)).magnitude;
                var facing = Vector3.Dot(fwd, -b.Face.forward);
                var va = VesselOf(a); var vb = VesselOf(b);
                bool same = va != null && vb != null && va.GlobalId == vb.GlobalId;
                var act = G.ViewController.GetActiveSimVessel();
                double am = 0;
                Vector3 com = act != null ? CentreOfMass(act, out am) : Vector3.zero;
                var local = a.Face.InverseTransformPoint(com);
                w.WriteLine(string.Join(",", new[] {
                    t.ToString("F3", Inv), phase, gap.ToString("R", Inv), closing.ToString("R", Inv), lateral.ToString("R", Inv), facing.ToString("R", Inv),
                    State(a), State(b), same ? "1" : "0", am.ToString("R", Inv),
                    (va?.totalMass ?? 0).ToString("R", Inv), (vb?.totalMass ?? 0).ToString("R", Inv),
                    local.x.ToString("R", Inv), local.y.ToString("R", Inv), local.z.ToString("R", Inv), input.ToString("R", Inv) }));
            }

            IEnumerator Hold(float seconds)
            {
                float end = t + seconds;
                while (t < end) yield return Tick();
            }

            IEnumerator Undock(string name)
            {
                phase = name;
                mark(name + ": undock, gap " + gap.ToString("F3", Inv));
                typeof(Module_DockingNode).GetMethod("UndockInternal", Priv).Invoke(a.Module, null);
                yield return Tick();
                // keep flying the cupola's half
                var va = VesselOf(a);
                if (va != null && !G.ViewController.IsActiveVessel(va)) { G.ViewController.SetActiveVehicle(va); mark("active vessel back to A"); }
                yield return Hold(6f);
                var mine = VesselOf(a);
                if (mine != null) { RcsOn(mine); mine.SetAutopilotEnableDisable(true); mark("rcs " + mine.IsRCSEnabled + ", active " + G.ViewController.IsActiveVessel(mine)); }
                mark(name + " drift: gap " + gap.ToString("F3", Inv) + " m, opening at " + (-closing).ToString("F4", Inv) + " m/s");
            }

            // which translation input pushes A toward B (along +fwd), and which way
            int axis = 2; float sign = 1;
            void SetAxis(int k, float v)
            {
                input = v;
                InputHolder.X = k == 0 ? v : 0f; InputHolder.Y = k == 1 ? v : 0f; InputHolder.Z = k == 2 ? v : 0f;
            }

            IEnumerator Calibrate()
            {
                phase = "calibrate";
                float best = 0;
                for (int k = 0; k < 3; k++)
                {
                    float c0 = closing;
                    SetAxis(k, 1f); yield return Hold(0.5f);
                    float dv = closing - c0;
                    SetAxis(k, -1f); yield return Hold(0.5f);
                    SetAxis(k, 0f); yield return Hold(0.5f);
                    mark("axis " + k + " +1 for 0.5 s: closing " + dv.ToString("F4", Inv) + " m/s");
                    if (Mathf.Abs(dv) > Mathf.Abs(best)) { best = dv; axis = k; sign = Mathf.Sign(dv); }
                }
                mark("approach axis " + axis + " sign " + sign);
            }

            // RCS toward a closing speed (negative opens the gap) until `until` says stop
            IEnumerator Fly(string name, float speed, Func<bool> until, float timeout)
            {
                phase = name;
                float end = t + timeout;
                while (t < end && !until())
                {
                    SetAxis(axis, sign * Mathf.Clamp((speed - closing) * 4f, -1f, 1f));
                    yield return Tick();
                }
                SetAxis(axis, 0f);
            }

            IEnumerator Approach(float speed)
            {
                yield return Fly("back-off", -0.3f, () => gap >= 4f, 40f);
                yield return Fly("hold", 0f, () => false, 3f);
                mark("approach at " + speed.ToString("F1", Inv) + " from " + gap.ToString("F3", Inv) + " m");
                yield return Fly("approach-" + speed.ToString("F1", Inv), speed, () => gap <= 2f, 40f);
                phase = "coast-" + speed.ToString("F1", Inv);
                mark(phase + " from " + gap.ToString("F3", Inv) + " m at " + closing.ToString("F4", Inv) + " m/s");
                float end = t + 30f;
                string sa = State(a);
                while (t < end)
                {
                    yield return Tick();
                    var s = State(a);
                    if (s != sa) { mark("port A " + sa + " -> " + s + " at gap " + gap.ToString("F4", Inv) + " closing " + closing.ToString("F4", Inv)); sa = s; }
                    if (s == "Docked") break;
                }
                phase = "docked";
                yield return Hold(6f);
                var act = G.ViewController.GetActiveSimVessel();
                CentreOfMass(act, out var m);
                mark("after approach at " + speed.ToString("F1", Inv) + ": port A " + State(a) + ", active mass " + act.totalMass.ToString("R", Inv) + " (parts " + m.ToString("R", Inv) + ")");
            }

            yield return Hold(1f);
            yield return Undock("undock-vab");
            yield return Calibrate();
            yield return Approach(0.2f);
            if (State(a) == "Docked")
            {
                yield return Undock("undock-dock");
                yield return Approach(0.6f);
            }
            InputHolder.Clear();
            w.Close();
            mark("finished docking");
        }
    }
}
