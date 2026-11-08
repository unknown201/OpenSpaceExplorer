using System;
using System.Collections;
using System.Globalization;
using System.IO;
using KSP.Game;
using KSP.Modules;
using KSP.Sim;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // T41 jet engines (docs/ksp2-feature-gap-analysis.md P05/P06), on Stock04_Jet (craft-jet.json, the stock
    // assembly as shipped) launched from Runway_1:
    //   idle         engine staged at zero throttle, brakes on, 5 s
    //   static-up    brakes on, full throttle 20 s: spool-up and the intake-air limit at standstill
    //   static-down  throttle cut 10 s: spool-down
    //   roll         brakes off, full throttle 25 s: intake air and thrust against speed
    //   reverse      throttle cut, the Wheesley switched to its Reverse mode, 1 s, then full throttle 15 s
    //   stop         throttle cut, brakes on, 8 s
    // Every physics tick: the engine's requested/current throttle, spool, final and resulting thrust, flow
    // multiplier, propellant delivery, flameout, mode, state, and the intakes' air supply. Only inputs and the
    // engine's own mode action are used; physics is never modified.
    public static class JetScenario
    {
        static GameInstance G => GameManager.Instance?.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;

        public static IEnumerator Run(MonoBehaviour host, VesselComponent vessel, string dir, Action<string> mark)
        {
            var input = G.ViewController.flightInputHandler;
            string phase = "idle";
            bool logging = true;
            IEnumerator Log()
            {
                using (var w = new StreamWriter(Path.Combine(dir, "out", "jet-trace.csv"), false))
                {
                    w.WriteLine("t,phase,srf_speed,mach,density,static_kpa,alt_m,vspeed,mass_t,main_throttle,requested_throttle,current_throttle,spool,final_thrust_kn,resulting_thrust_kn,flow_mult,amount_received,current_requirement,flameout,mode,state,ignited,intake_units,situation,ut");
                    double start = G.UniverseModel.UniverseTime, last = double.NegativeInfinity;
                    while (logging)
                    {
                        yield return new WaitForFixedUpdate();
                        var v = G.ViewController.GetActiveSimVessel();
                        if (v == null) continue;
                        double ut = G.UniverseModel.UniverseTime;
                        if (ut <= last) continue;
                        last = ut;
                        Data_Engine e = null;
                        double intake = 0;
                        foreach (var part in v.SimulationObject.PartOwner.Parts)
                        {
                            if (e == null && part.TryGetModuleData<PartComponentModule_Engine, Data_Engine>(out var de)) e = de;
                            if (part.TryGetModuleData<PartComponentModule_ResourceIntake, Data_ResourceIntake>(out var di)) intake += di.ResourceUnits;
                        }
                        var ps = e?.CurrentPropellantState;
                        string F(double x) => x.ToString("R", Inv);
                        w.WriteLine(string.Join(",", new[] {
                            F(ut - start), phase, F(v.SrfSpeedMagnitude), F(v.MachNumber), F(v.AtmDensity), F(v.StaticPressure_kPa),
                            F(v.AltitudeFromSeaLevel), F(v.VerticalSrfSpeed), F(v.totalMass), F(v.flightCtrlState.mainThrottle),
                            F(e?.RequestedThrottle ?? 0), F(e?.currentThrottle ?? 0), F(e?.EngineSpool ?? 0), F(e?.FinalThrustValue ?? 0),
                            F(e?.ResultingThrust ?? 0), F(e?.FlowMultiplier ?? 0), F(ps?.amountReceived ?? 0), F(ps?.currentRequirement ?? 0),
                            e != null && e.Flameout ? "1" : "0", (e?.currentEngineModeIndex ?? -1).ToString(Inv), e?.State.ToString() ?? "",
                            e != null && e.EngineIgnited ? "1" : "0", F(intake), v.Situation.ToString(), F(ut) }));
                    }
                }
            }
            void Phase(string name) { phase = name; mark("jet " + name); }

            input.OverrideInputThrottleCutoff(true);
            vessel.SetAutopilotEnableDisable(true);
            SetGroup(KSPActionGroup.Brakes, true);
            yield return new WaitForSeconds(1f);
            var logger = host.StartCoroutine(Log());
            vessel.ActivateNextStage();
            Phase("idle");
            yield return new WaitForSeconds(5f);
            Phase("static-up");
            input.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(20f);
            Phase("static-down");
            input.OverrideInputThrottleCutoff(true);
            yield return new WaitForSeconds(10f);
            Phase("roll");
            SetGroup(KSPActionGroup.Brakes, false);
            input.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(25f);
            Phase("reverse");
            input.OverrideInputThrottleCutoff(true);
            var engine = EngineModule(G.ViewController.GetActiveSimVessel());
            if (engine != null) engine.ChangeEngineMode(1); else mark("jet: no Module_Engine view");
            yield return new WaitForSeconds(1f);
            input.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(15f);
            Phase("stop");
            input.OverrideInputThrottleCutoff(true);
            SetGroup(KSPActionGroup.Brakes, true);
            yield return new WaitForSeconds(8f);
            logging = false;
            yield return new WaitForFixedUpdate();
            yield return new WaitForFixedUpdate();
            host.StopCoroutine(logger);
        }

        static Module_Engine EngineModule(VesselComponent v)
        {
            if (v == null) return null;
            foreach (var part in v.SimulationObject.PartOwner.Parts)
            {
                if (!part.TryGetModule<PartComponentModule_Engine>(out _)) continue;
                if (!G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) continue;
                var behavior = view.GetComponent<PartBehavior>();
                var m = behavior?.GetModule<Module_Engine>();
                if (m != null) return m;
            }
            return null;
        }

        static void SetGroup(KSPActionGroup group, bool on)
        {
            if (G.ViewController.TryGetActiveVehicle(out var v)) v.ControllableActionGroups?.SetGroup(group, on);
        }
    }
}
