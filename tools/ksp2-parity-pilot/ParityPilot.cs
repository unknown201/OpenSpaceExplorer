using System;
using System.Collections;
using System.Globalization;
using System.IO;
using BepInEx;
using KSP;
using KSP.Game;
using KSP.IO;
using KSP.OAB;
using KSP.Sim;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // Scripted reference run for the OSE side-by-side report (docs/ksp2-side-by-side-gaps.md).
    // Does nothing unless BepInEx/ose-parity/run.txt exists. Then: open (or create) the "OSE Parity" Sandbox
    // campaign, screenshot the KSC, load BepInEx/ose-parity/craft.json in the VAB, screenshot, launch, hold SAS,
    // full throttle, first stage, and log telemetry every physics tick until the craft lands or is lost.
    // Inputs go through the game's own FlightInputHandler override hooks; physics is never modified.
    // FlightInputHandler consumes an override once and there are several physics ticks per frame, so a value
    // set from a coroutine reaches only the first tick of each frame. This component runs before the handler
    // and re-applies the held values on every FixedUpdate and Update.
    [DefaultExecutionOrder(-32000)]
    public sealed class InputHolder : MonoBehaviour
    {
        public static float? Pitch, Yaw, Roll, X, Y, Z, WheelThrottle, WheelSteer;
        void FixedUpdate() { Apply(); }
        void Update() { Apply(); }
        static void Apply()
        {
            if (Pitch.HasValue) FlightInputHandler.OverrideInputPitch(Pitch.Value);
            if (Yaw.HasValue) FlightInputHandler.OverrideInputYaw(Yaw.Value);
            if (Roll.HasValue) FlightInputHandler.OverrideInputRoll(Roll.Value);
            if (X.HasValue) FlightInputHandler.OverrideInputTranslateX(X.Value);
            if (Y.HasValue) FlightInputHandler.OverrideInputTranslateY(Y.Value);
            if (Z.HasValue) FlightInputHandler.OverrideInputTranslateZ(Z.Value);
            if (WheelThrottle.HasValue || WheelSteer.HasValue)
            {
                FlightInputHandler.OverrideInputTranslateX(X ?? 0f);
                FlightCtrlStateInputOverride.InputWheelThrottleOverride = WheelThrottle ?? 0f;
                FlightCtrlStateInputOverride.InputWheelSteeringOverride = WheelSteer ?? 0f;
            }
        }
        public static void Clear() { Pitch = Yaw = Roll = X = Y = Z = WheelThrottle = WheelSteer = null; }
    }

    [BepInPlugin("org.openspaceexplorer.paritypilot", "OSE Parity Pilot", "0.1.0")]
    [DefaultExecutionOrder(32000)]
    public sealed class ParityPilot : BaseUnityPlugin
    {
        const string Campaign = "OSE Parity";
        string dir;
        StreamWriter trace;
        double start, last;
        string vesselId;
        bool done;

        void Awake()
        {
            dir = Path.Combine(Paths.GameRootPath, "BepInEx", "ose-parity");
            if (!File.Exists(Path.Combine(dir, "run.txt"))) { Logger.LogInfo("OSE parity pilot idle (no run.txt)."); return; }
            Directory.CreateDirectory(Path.Combine(dir, "out"));
            gameObject.AddComponent<InputHolder>();
            StartCoroutine(Run());
        }

        static GameInstance G => GameManager.Instance?.Game;
        static GameState State => G?.GlobalGameState?.GetState() ?? GameState.Invalid;

        void Mark(string s)
        {
            Logger.LogInfo("[parity] " + s);
            File.AppendAllText(Path.Combine(dir, "out", "steps.log"), DateTime.UtcNow.ToString("HH:mm:ss.fff", CultureInfo.InvariantCulture) + " " + s + "\n");
        }

        IEnumerator Shot(string name)
        {
            // wait for the loading curtain to lift and the scene to settle, so the shot is gameplay, not loading art
            float waited = 0;
            while (G?.UI != null && G.UI.GetCurtainVisibility() && waited < 120) { waited += Time.unscaledDeltaTime; yield return null; }
            yield return new WaitForSecondsRealtime(waited > 0 ? 4f : 0.5f);
            yield return new WaitForEndOfFrame();
            ScreenCapture.CaptureScreenshot(Path.Combine(dir, "out", name + ".png"));
            Mark("shot " + name);
            yield return new WaitForSeconds(0.5f);
        }

        IEnumerator WaitState(GameState s, float timeout)
        {
            float t = 0;
            while (State != s && t < timeout) { t += Time.unscaledDeltaTime; yield return null; }
            if (State != s) throw new Exception("timed out waiting for " + s + " (now " + State + ")");
        }

        IEnumerator Run()
        {
            Mark("start");
            float t = 0;
            while (State != GameState.MainMenu && t < 600) { t += Time.unscaledDeltaTime; yield return null; }
            yield return new WaitForSeconds(8f);
            Mark("main menu");
            yield return Shot("01-main-menu");

            var slm = G.SaveLoadManager;
            var campaignDir = slm.GetCampaignFolderPath(CampaignType.SinglePlayer, Campaign);
            string path = null;
            if (Directory.Exists(campaignDir))
            {
                DateTime newest = DateTime.MinValue;
                foreach (var f in Directory.GetFiles(campaignDir, "*.json"))
                    if (File.GetLastWriteTimeUtc(f) > newest) { newest = File.GetLastWriteTimeUtc(f); path = f; }
            }
            if (path != null)
            {
                Mark("loading " + path);
                slm.LoadGameFromFile(path, (_, ok) => Mark("load finished " + ok));
            }
            else
            {
                Mark("creating campaign");
                var dmLoad = GameManager.Instance.DifficultyOptionsDataManager;
                if (dmLoad.GetDifficultyPresetCopy(DifficultyLevel.Normal) == null)
                {
                    dmLoad.Load();
                    t = 0;
                    while (dmLoad.GetDifficultyPresetCopy(DifficultyLevel.Normal) == null && t < 60) { t += Time.unscaledDeltaTime; yield return null; }
                    yield return new WaitForSeconds(2f);
                }
                var ticket = new LoadOrSaveCampaignTicket();
                ticket.SetLoadOrSaveCampaignOperation(LoadOrSaveCampaignOperation.Load_StartNewCampaign);
                ticket.SetCampaignName(Campaign);
                ticket.SetCampaignType(CampaignType.SinglePlayer);
                ticket.SetCampaignReadOnlyFlag(campaignReadOnlyFlag: false);
                ticket.SetGameMode(GameManager.Instance.GameModeManager.GetCurrentGameModeId());
                ticket.SetGameModeData(G.GameModes.Find(gm => gm.name == "Sandbox").data);
                ticket.SetInitializeAsNewCampaign(initializeAsNewCampaign: true);
                ticket.NewGameSetupInfo.Init();
                var dm = GameManager.Instance.DifficultyOptionsDataManager;
                var states = dm.GetDifficultyPresetCopy(DifficultyLevel.Normal) ?? new DifficultyOptionStates();
                dm.AddDefinitionsMissingFromStates(states, DifficultyLevel.Normal);
                Mark("difficulty options " + states.Count);
                ticket.NewGameSetupInfo.SetInitialDifficultyOptionStates(states);
                ticket.NewGameSetupInfo.SetInitialDifficultyLevel(DifficultyLevel.Normal);
                ticket.NewPlayerSetupInfo.Init();
                ticket.NewPlayerSetupInfo.SetAgencyChoice(NewPlayerAgencyChoice.CreateNewAgency);
                ticket.NewPlayerSetupInfo.SetNewAgencyName("OSE Parity");
                ticket.NewPlayerSetupInfo.SetExistingAgencyId(0);
                ticket.NewPlayerSetupInfo.SetIsFTUEEnabled(false);
                ticket.SetOnLoadOrSaveCampaignFinishedCallback((_, ok) => Mark("create finished " + ok));
                ticket.StartLoadOrSaveOperation();
            }
            yield return WaitState(GameState.KerbalSpaceCenter, 600);
            yield return new WaitForSeconds(20f);
            Mark("ksc");
            yield return Shot("02-ksc");

            var scenario = File.ReadAllText(Path.Combine(dir, "run.txt")).Trim();
            Mark("scenario " + scenario);
            double darkUT = -1, brightUT = -1;
            if (scenario == "visuals")
            {
                // KSC every 45 game minutes across one 6 h Kerbin day; remember the darkest and brightest moments
                float darkest = float.MaxValue, brightest = -1f;
                for (int sweep = 0; sweep < 8; sweep++)
                {
                    if (sweep > 0)
                    {
                        double target = G.UniverseModel.UniverseTime + 2700;
                        G.ViewController.TimeWarp.WarpTo(target, 1f, 1000f);
                        float w = 0;
                        while (G.UniverseModel.UniverseTime < target - 1 && w < 120) { w += Time.unscaledDeltaTime; yield return null; }
                        yield return new WaitForSecondsRealtime(4f);
                    }
                    double ut = G.UniverseModel.UniverseTime;
                    yield return new WaitForEndOfFrame();
                    var tex = ScreenCapture.CaptureScreenshotAsTexture();
                    var px = tex.GetPixels32();
                    double sum = 0; for (int i = 0; i < px.Length; i += 97) sum += px[i].r + px[i].g + px[i].b;
                    float lum = (float)(sum / (px.Length / 97.0) / 3.0);
                    File.WriteAllBytes(Path.Combine(dir, "out", "v1-ksc-ut" + ((long)ut).ToString(CultureInfo.InvariantCulture) + "-lum" + ((int)lum) + ".png"), ImageConversion.EncodeToPNG(tex));
                    Destroy(tex);
                    Mark("ksc ut " + ut.ToString("F0", CultureInfo.InvariantCulture) + " lum " + lum.ToString("F0", CultureInfo.InvariantCulture));
                    if (lum < darkest) { darkest = lum; darkUT = ut; }
                    if (lum > brightest) { brightest = lum; brightUT = ut; }
                }
                // go on to the next darkest moment for the night pad shot
                double night = darkUT; while (night < G.UniverseModel.UniverseTime + 60) night += 21600;
                G.ViewController.TimeWarp.WarpTo(night, 1f, 1000f);
                float w2 = 0; while (G.UniverseModel.UniverseTime < night - 1 && w2 < 180) { w2 += Time.unscaledDeltaTime; yield return null; }
                yield return new WaitForSecondsRealtime(4f);
                yield return Shot("v1-ksc-night-before-vab");
            }
            bool orbital = scenario == "rotation" || scenario == "rcs";
            var snapshot = IOProvider.FromJsonFile<OABHistoricalSnapshot>(Path.Combine(dir, scenario == "rcs" ? "craft-rcs.json" : scenario == "rover" ? "craft-rover.json" : scenario == "legs" ? "craft-legs.json" : orbital ? "craft-rotation.json" : (scenario == "k1" || scenario == "partdump") ? "craft-k1.json" : "craft.json"));
            var config = orbital
                ? new OABConfig(OABVariant.VAB, OABEnvironmentType.Orbital, OABConstructionType.User, null)
                : new OABConfig(OABVariant.VAB, OABEnvironmentType.Terrestrial, OABConstructionType.User, "Kerbin");
            bool shown = false;
            G.OAB.ShowOAB(config, snapshot, null, () => shown = true);
            t = 0;
            while (!shown && t < 300) { t += Time.unscaledDeltaTime; yield return null; }
            yield return new WaitForSeconds(10f);
            Mark("vab " + State);
            yield return Shot("03-vab");
            G.OAB.Current.eventsUI.OnLaunch?.Invoke();
            yield return WaitState(GameState.FlightView, 600);
            VesselComponent vessel = null;
            t = 0;
            while ((vessel = G.ViewController.GetActiveSimVessel()) == null && t < 120) { t += Time.unscaledDeltaTime; yield return null; }
            yield return new WaitForSeconds(12f);
            Mark("in flight, situation " + vessel.Situation + ", mass " + vessel.totalMass + ", alt " + vessel.AltitudeFromSeaLevel);
            yield return Shot(orbital ? "04-orbit" : "04-pad");
            if (scenario == "rcs") { yield return Rcs(vessel); yield break; }
            if (orbital) { yield return Rotation(vessel); yield break; }
            if (scenario == "rover") { yield return Rover(vessel); yield break; }
            if (scenario == "eva") { yield return Eva(vessel); yield break; }
            if (scenario == "partdump") { yield return PartDump(vessel); yield break; }
            if (scenario == "legs") { yield return Legs(vessel); yield break; }
            if (scenario == "visuals")
            {
                vessel.SetActionGroup(KSPActionGroup.Lights, true);
                yield return new WaitForSecondsRealtime(3f);
                yield return Shot("v3-pad-night-lights");
                // morning for the flight
                double day = brightUT; while (day < G.UniverseModel.UniverseTime + 60) day += 21600;
                G.ViewController.TimeWarp.WarpTo(day, 1f, 1000f);
                float w3 = 0; while (G.UniverseModel.UniverseTime < day - 1 && w3 < 180) { w3 += Time.unscaledDeltaTime; yield return null; }
                yield return new WaitForSecondsRealtime(4f);
                yield return Shot("v3-pad-day");
            }
            if (scenario == "hop") { yield return Hop(vessel); yield break; }
            if (scenario == "k1") { yield return Staged(vessel); yield break; }

            vessel.SetAutopilotEnableDisable(true);
            G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(0.5f);
            Begin(vessel);
            vessel.ActivateNextStage();
            Mark("staged");

            double[] marks = scenario == "visuals" ? new double[] { 15000, 45000 } : new double[] { 150, 1500, 8000, 25000 };
            string[] names = scenario == "visuals" ? new[] { "v4-flight-15km", "v4-flight-45km" } : new[] { "05-liftoff", "06-ascent-1km", "07-ascent-8km", "08-ascent-25km" };
            bool re40 = false, re25 = false;
            int k = 0;
            bool mapped = false, apo = false;
            double top = 0;
            t = 0;
            while (!done && t < 600)
            {
                t += Time.deltaTime;
                vessel = G.ViewController.GetActiveSimVessel();
                if (vessel == null || vessel.GlobalId.ToString() != vesselId) { Mark("vessel gone"); break; }
                double alt = vessel.AltitudeFromSeaLevel;
                top = Math.Max(top, alt);
                if (k < marks.Length && alt >= marks[k]) { yield return Shot(names[k]); k++; }
                if (!apo && vessel.VerticalSrfSpeed < 0 && alt > 1000) { apo = true; Mark("apoapsis " + top.ToString("F1", CultureInfo.InvariantCulture)); }
                if (scenario == "visuals" && apo && !re40 && alt < 40000) { re40 = true; yield return Shot("v5-reentry-40km"); }
                if (scenario == "visuals" && apo && !re25 && alt < 25000) { re25 = true; yield return Shot("v5-reentry-25km"); }
                if (scenario != "visuals" && apo && !mapped && alt < 30000 && top > 31000)
                {
                    mapped = true;
                    G.GlobalGameState.SetState(GameState.Map3DView);
                    G.Map.EnableFlightMap();
                    yield return new WaitForSeconds(3f);
                    yield return Shot("09-map");
                }
                if (apo && (vessel.Situation == VesselSituations.Landed || vessel.Situation == VesselSituations.Splashed)) { Mark("down " + vessel.Situation); break; }
                yield return null;
            }
            Stop();
            Mark("finished, apoapsis " + top.ToString("F1", CultureInfo.InvariantCulture));
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok\n");
            yield return new WaitForSeconds(3f);
            Application.Quit();
        }

        IEnumerator Rotation(VesselComponent vessel)
        {
            vessel.SetAutopilotEnableDisable(false);
            yield return new WaitForSeconds(2f);
            Begin(vessel, "rotation-trace.csv");
            yield return new WaitForSeconds(2f);
            foreach (var axis in new[] { "pitch", "yaw", "roll" })
            {
                Mark(axis + " +1 for 1 s");
                Set(axis, 1f);
                yield return new WaitForSeconds(1f);
                Set(axis, 0f);
                yield return new WaitForSeconds(4f);
            }
            Mark("SAS on");
            vessel.SetAutopilotEnableDisable(true);
            yield return new WaitForSeconds(12f);
            Stop();
            Mark("finished rotation");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(3f);
            Application.Quit();
        }

        // T2 subsonic drag + T12 parachute: 8 s of full throttle straight up, cut, then the parachute stage
        // below 2.5 km on the way down; logs until the craft is down.
        IEnumerator Hop(VesselComponent vessel)
        {
            vessel.SetAutopilotEnableDisable(true);
            G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(0.5f);
            Begin(vessel, "hop-trace.csv");
            vessel.ActivateNextStage();
            Mark("staged");
            yield return new WaitForSeconds(8f);
            G.ViewController.flightInputHandler.OverrideInputThrottleCutoff(true);
            Mark("cutoff");
            bool apo = false, chute = false;
            float t = 0;
            while (!done && t < 900)
            {
                t += Time.deltaTime;
                vessel = G.ViewController.GetActiveSimVessel();
                if (vessel == null || vessel.GlobalId.ToString() != vesselId) { Mark("vessel gone"); break; }
                double alt = vessel.AltitudeFromSeaLevel;
                if (!apo && vessel.VerticalSrfSpeed < 0) { apo = true; Mark("apoapsis " + alt.ToString("F1", CultureInfo.InvariantCulture)); }
                if (apo && !chute && alt < 2500) { chute = true; vessel.ActivateNextStage(); Mark("parachute staged at " + alt.ToString("F0", CultureInfo.InvariantCulture)); yield return Shot("10-chute"); }
                if (apo && (vessel.Situation == VesselSituations.Landed || vessel.Situation == VesselSituations.Splashed)) { Mark("down " + vessel.Situation); yield return Shot("11-down"); break; }
                yield return null;
            }
            Stop();
            Mark("finished hop");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(3f);
            Application.Quit();
        }

        // T4 staging, decouplers, solid boosters: SAS on, full throttle, first stage; then the next stage each
        // time the mass has stopped falling for 1 s (the burning stage is spent); logs until apoapsis.
        IEnumerator Staged(VesselComponent vessel)
        {
            vessel.SetAutopilotEnableDisable(true);
            G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(0.5f);
            Begin(vessel, "k1-trace.csv");
            vessel.ActivateNextStage();
            Mark("stage 1");
            int stages = 1;
            double lastMass = vessel.totalMass, steadyFor = 0;
            float t = 0;
            while (!done && t < 600)
            {
                t += Time.deltaTime;
                vessel = G.ViewController.GetActiveSimVessel();
                if (vessel == null || vessel.GlobalId.ToString() != vesselId) { Mark("vessel gone"); break; }
                double m = vessel.totalMass;
                if (lastMass - m > 1e-4) { steadyFor = 0; lastMass = m; } else steadyFor += Time.deltaTime;
                if (steadyFor > 1.0 && stages < 5 && vessel.VerticalSrfSpeed > 0)
                {
                    vessel.ActivateNextStage();
                    stages++;
                    steadyFor = 0;
                    lastMass = vessel.totalMass;
                    Mark("stage " + stages + " at alt " + vessel.AltitudeFromSeaLevel.ToString("F0", CultureInfo.InvariantCulture) + " mass " + vessel.totalMass.ToString("F3", CultureInfo.InvariantCulture));
                    yield return Shot("k1-stage" + stages);
                }
                if (vessel.VerticalSrfSpeed < 0 && vessel.AltitudeFromSeaLevel > 1000) { Mark("apoapsis " + vessel.AltitudeFromSeaLevel.ToString("F0", CultureInfo.InvariantCulture)); break; }
                yield return null;
            }
            Stop();
            Mark("finished k1");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(3f);
            Application.Quit();
        }

        // T17 RCS: SAS off, RCS on, translate forward (Z) 2 s, right (X) 2 s, then pitch with RCS 1 s.
        IEnumerator Rcs(VesselComponent vessel)
        {
            vessel.SetAutopilotEnableDisable(false);
            vessel.SetActionGroup(KSPActionGroup.RCS, true);
            vessel.IsRCSEnabled = true;
            yield return new WaitForSeconds(2f);
            Mark("rcs group " + vessel.GetActionGroupState(KSPActionGroup.RCS) + " IsRCSEnabled " + vessel.IsRCSEnabled + " mass " + vessel.totalMass);
            Begin(vessel, "rcs-trace.csv");
            yield return new WaitForSeconds(2f);
            foreach (var axis in new[] { "z", "x", "pitch" })
            {
                Mark(axis + " +1");
                if (axis == "z") InputHolder.Z = 1f; else if (axis == "x") InputHolder.X = 1f; else InputHolder.Pitch = 1f;
                yield return new WaitForSeconds(axis == "pitch" ? 1f : 2f);
                InputHolder.X = InputHolder.Z = InputHolder.Pitch = 0f;
                yield return new WaitForSeconds(3f);
            }
            Stop();
            Mark("finished rcs");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(3f);
            Application.Quit();
        }

        // T16 wheels and driving: release from the pad, full wheel throttle 15 s, coast 3 s, brakes 8 s,
        // then half throttle with full right steering 10 s.
        IEnumerator Rover(VesselComponent vessel)
        {
            vessel.ActivateNextStage();
            yield return new WaitForSeconds(3f);
            Begin(vessel, "rover-trace.csv");
            yield return new WaitForSeconds(1f);
            yield return Drive(15f, 1f, 0f);
            Mark("coast");
            yield return Drive(3f, 0f, 0f);
            Mark("brakes");
            vessel.SetActionGroup(KSPActionGroup.Brakes, true);
            yield return Drive(8f, 0f, 0f);
            vessel.SetActionGroup(KSPActionGroup.Brakes, false);
            Mark("turn");
            yield return Drive(10f, 0.5f, 1f);
            yield return Drive(3f, 0f, 0f);
            yield return Shot("12-rover");
            Stop();
            Mark("finished rover");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(3f);
            Application.Quit();
        }

        // T19/T20 EVA: launch, the first crew member steps out of the pod, then (camera fixed) walks forward 6 s,
        // stands 3 s, jumps, stands 4 s, walks back 4 s; logs the Kerbal vessel.
        IEnumerator Eva(VesselComponent vessel)
        {
            G.ViewController.flightInputHandler.OverrideInputThrottleCutoff(true);
            yield return new WaitForSeconds(3f);
            PartBehavior pod = null;
            foreach (var part in vessel.SimulationObject.PartOwner.Parts)
                if (part.Name.StartsWith("pod_") && G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) { pod = view.GetComponent<PartBehavior>(); break; }
            Mark("pod " + (pod != null));
            bool ok = pod != null && EVAUtils.TryStartEVA(pod, 0);
            Mark("TryStartEVA " + ok);
            float t = 0;
            VesselComponent kerbal = null;
            while (t < 30) { t += Time.deltaTime; kerbal = G.ViewController.GetActiveSimVessel(); if (kerbal != null && kerbal.IsKerbalEVA) break; yield return null; }
            Mark("kerbal active " + (kerbal != null && kerbal.IsKerbalEVA) + " situation " + kerbal?.Situation + " mass " + kerbal?.totalMass);
            yield return new WaitForSeconds(5f);
            yield return Shot("13-eva");
            Begin(kerbal, "eva-trace.csv");
            yield return new WaitForSeconds(1f);
            Mark("walk forward"); InputHolder.Z = -1f; yield return new WaitForSeconds(6f);
            InputHolder.Z = 0f; Mark("stand"); yield return new WaitForSeconds(3f);
            Mark("jump");
            FlightInputHandler.OverrideInputStage(true); yield return null; yield return new WaitForFixedUpdate();
            FlightInputHandler.OverrideInputStage(false);
            yield return new WaitForSeconds(4f);
            Mark("walk back"); InputHolder.Z = 1f; yield return new WaitForSeconds(4f);
            InputHolder.Z = 0f; yield return new WaitForSeconds(2f);
            yield return Shot("14-eva-end");
            Stop();
            Mark("finished eva");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(3f);
            Application.Quit();
        }

        // Per-part mass and drag of the craft standing on the pad, for comparing KSP2's mass model with OSE's.
        IEnumerator PartDump(VesselComponent vessel)
        {
            yield return new WaitForSeconds(5f);
            using (var w = new StreamWriter(Path.Combine(dir, "out", "parts.csv"), false))
            {
                w.WriteLine("index,name,physics_mode,dry_t,resource_t,green_t,total_t,physics_mass_t,drag_area_xp,drag_area_xn,drag_area_yp,drag_area_yn,drag_area_zp,drag_area_zn,drag_cd_xp,drag_cd_xn,drag_cd_yp,drag_cd_yn,drag_cd_zp,drag_cd_zn,drag_enabled");
                int i = 0;
                foreach (var part in vessel.SimulationObject.PartOwner.Parts)
                {
                    string drag = string.Join(",", new string[13]);
                    if (G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view))
                    {
                        var m = view.GetComponent<KSP.Modules.Module_Drag>();
                        if (m != null && m.DataModules.Contains(typeof(KSP.Modules.Data_Drag)))
                        {
                            var d = (KSP.Modules.Data_Drag)m.DataModules[typeof(KSP.Modules.Data_Drag)];
                            var vals = new System.Collections.Generic.List<string>();
                            foreach (var a in d.weightedArea) vals.Add(a.ToString("R", CultureInfo.InvariantCulture));
                            foreach (var c in d.weightedDrag) vals.Add(c.ToString("R", CultureInfo.InvariantCulture));
                            vals.Add(d.DragEnabled ? "1" : "0");
                            drag = string.Join(",", vals);
                        }
                    }
                    double[] mass = { part.DryMass, part.ResourceMass, part.GreenMass, part.TotalMass, part.PhysicsMass };
                    w.WriteLine(i++ + "," + part.Name + "," + part.PhysicsMode + "," + string.Join(",", Array.ConvertAll(mass, x => x.ToString("R", CultureInfo.InvariantCulture))) + "," + drag);
                }
            }
            Mark("parts dumped, vessel mass " + vessel.totalMass.ToString("R", CultureInfo.InvariantCulture));
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(2f);
            Application.Quit();
        }

        // T15 landing legs: legs deployed on the pad 10 s, then 0.3 s of full throttle, cut, and the drop back
        // onto the legs; logs until 15 s after touching down. Also lists the vessel's parts.
        IEnumerator Legs(VesselComponent vessel)
        {
            var names = new System.Collections.Generic.List<string>();
            foreach (var part in vessel.SimulationObject.PartOwner.Parts) names.Add(part.Name);
            Mark("parts " + string.Join(" ", names));
            vessel.SetActionGroup(KSPActionGroup.Gear, true);
            vessel.SetAutopilotEnableDisable(true);
            Begin(vessel, "legs-trace.csv");
            yield return new WaitForSeconds(10f);
            Mark("hop");
            G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
            vessel.ActivateNextStage();
            yield return new WaitForSeconds(0.3f);
            G.ViewController.flightInputHandler.OverrideInputThrottleCutoff(true);
            Mark("cutoff");
            bool up = false; float t = 0, down = -1;
            while (!done && t < 120)
            {
                t += Time.deltaTime;
                vessel = G.ViewController.GetActiveSimVessel();
                if (vessel == null || vessel.GlobalId.ToString() != vesselId) { Mark("vessel gone"); break; }
                if (!up && vessel.Situation == VesselSituations.Flying) up = true;
                if (up && down < 0 && vessel.Situation == VesselSituations.Landed) { down = t; Mark("touchdown"); }
                if (down >= 0 && t - down > 15) break;
                yield return null;
            }
            int left = 0;
            if (vessel != null) foreach (var part in vessel.SimulationObject.PartOwner.Parts) left++;
            Mark("parts left " + left);
            Stop();
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(2f);
            Application.Quit();
        }

        IEnumerator Drive(float seconds, float throttle, float steer)
        {
            // OverrideInputWheelSteering writes the roll override (a game bug), so the holder raises the flag
            // through a zero translation and sets the wheel fields directly
            InputHolder.WheelThrottle = throttle;
            InputHolder.WheelSteer = steer;
            yield return new WaitForSeconds(seconds);
        }

        static void Set(string axis, float v)
        {
            if (axis == "pitch") InputHolder.Pitch = v;
            else if (axis == "yaw") InputHolder.Yaw = v;
            else InputHolder.Roll = v;
        }

        void Begin(VesselComponent vessel, string file = "drag-trace.csv")
        {
            vesselId = vessel.GlobalId.ToString();
            start = G.UniverseModel.UniverseTime;
            last = double.NegativeInfinity;
            trace = new StreamWriter(Path.Combine(dir, "out", file), false) { AutoFlush = true };
            trace.WriteLine("t,alt_m,vspeed,srf_speed,mach,q_kpa,density,static_kpa,mass_t,throttle,qx,qy,qz,qw,wx,wy,wz,sas,pitch,yaw,roll,vx,vy,vz,gx,gy,gz,tx,ty,tz,wheel_throttle,wheel_steer,temp_max_k,temp_pod_k,temp_engine_k,situation,ut");
        }

        void FixedUpdate()
        {
            if (trace == null) return;
            try
            {
                var vessel = G.ViewController.GetActiveSimVessel();
                if (vessel == null || vessel.GlobalId.ToString() != vesselId) { done = true; return; }
                double ut = G.UniverseModel.UniverseTime;
                if (ut <= last) return;
                last = ut;
                var frame = G.UniverseModel.inertialReferenceFrame.bodyMotionFrame;
                var q = frame.localFrame.ToLocalRotation(vessel.ControlTransform.Rotation);
                var w = Vector.Reframed(frame.ToRelativeAngularVelocity(vessel.AngularVelocityMassAvg), frame.localFrame).vector;
                var ov = Vector.Reframed(vessel.OrbitalVelocity, frame.localFrame).vector;
                double tmax = 0, tpod = 0, teng = 0;
                foreach (var part in vessel.SimulationObject.PartOwner.Parts)
                {
                    tmax = Math.Max(tmax, part.Temperature);
                    if (part.Name.StartsWith("pod_")) tpod = part.Temperature;
                    if (part.Name.StartsWith("engine_")) teng = part.Temperature;
                }
                var gv = Vector.Reframed(vessel.gravityForPos, frame.localFrame).vector;
                double[] v = { ut - start, vessel.AltitudeFromSeaLevel, vessel.VerticalSrfSpeed, vessel.SrfSpeedMagnitude, vessel.MachNumber,
                    vessel.DynamicPressure_kPa, vessel.AtmDensity, vessel.StaticPressure_kPa, vessel.totalMass, vessel.flightCtrlState.mainThrottle,
                    q.x, q.y, q.z, q.w, w.x, w.y, w.z, vessel.Autopilot != null && vessel.Autopilot.Enabled ? 1 : 0,
                    vessel.flightCtrlState.pitch, vessel.flightCtrlState.yaw, vessel.flightCtrlState.roll,
                    ov.x, ov.y, ov.z, gv.x, gv.y, gv.z, vessel.flightCtrlState.X, vessel.flightCtrlState.Y, vessel.flightCtrlState.Z,
                    vessel.flightCtrlState.wheelThrottle, vessel.flightCtrlState.wheelSteer, tmax, tpod, teng };
                trace.WriteLine(string.Join(",", Array.ConvertAll(v, x => x.ToString("R", CultureInfo.InvariantCulture))) + "," + vessel.Situation + "," + ut.ToString("R", CultureInfo.InvariantCulture));
            }
            catch (Exception e) { Logger.LogError(e); Stop(); }
        }

        void Stop() { if (trace == null) return; trace.Dispose(); trace = null; }
        void OnDestroy() { Stop(); }
    }
}
