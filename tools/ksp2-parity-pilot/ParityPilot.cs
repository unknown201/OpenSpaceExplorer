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
        public static float? Pitch, Yaw, Roll, X, Y, Z, WheelThrottle, WheelSteer, EvaFrontBack, EvaLeftRight, EvaUpDown;
        public static Vector3? KerbalMove;
        public static bool? EvaJump, EvaRun;
        static readonly System.Reflection.BindingFlags Priv = System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance;
        static void SetEva(string field, object value)
        {
            var h = GameManager.Instance?.Game?.ViewController?.evaInputHandler;
            if (h != null) typeof(EVAInputHandler).GetField(field, Priv)?.SetValue(h, value);
        }
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
            if (EvaFrontBack.HasValue) SetEva("_moveFrontBack", EvaFrontBack.Value);
            if (EvaLeftRight.HasValue) SetEva("_moveLeftRight", EvaLeftRight.Value);
            if (EvaUpDown.HasValue) SetEva("_moveUpDown", EvaUpDown.Value);
            if (KerbalMove.HasValue)
            {
                var v = GameManager.Instance?.Game?.ViewController?.GetActiveSimVessel();
                if (v != null && v.IsKerbalEVA)
                {
                    var st = v.flightCtrlState;
                    st.X = KerbalMove.Value.x; st.Y = KerbalMove.Value.y; st.Z = KerbalMove.Value.z;
                    v.SetFlightControlState(st);
                }
            }
            if (EvaJump.HasValue) SetEva("_jump", EvaJump.Value);
            if (EvaRun.HasValue) SetEva("_run", EvaRun.Value);
            if (WheelThrottle.HasValue || WheelSteer.HasValue)
            {
                FlightInputHandler.OverrideInputTranslateX(X ?? 0f);
                FlightCtrlStateInputOverride.InputWheelThrottleOverride = WheelThrottle ?? 0f;
                FlightCtrlStateInputOverride.InputWheelSteeringOverride = WheelSteer ?? 0f;
            }
        }
        public static void Clear() { Pitch = Yaw = Roll = X = Y = Z = WheelThrottle = WheelSteer = EvaFrontBack = EvaLeftRight = EvaUpDown = null; KerbalMove = null; EvaJump = EvaRun = null; }
    }

    [BepInPlugin("org.openspaceexplorer.paritypilot", "OSE Parity Pilot", "0.1.0")]
    [DefaultExecutionOrder(32000)]
    public sealed class ParityPilot : BaseUnityPlugin
    {
        const string Campaign = "OSE Parity";
        string dir;
        StreamWriter trace, comLog;
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
                for (int sweep = 0; sweep < 8; sweep++) // 8 x 45 min = one 6 h Kerbin day
                {
                    if (sweep > 0)
                    {
                        yield return WarpUntil(G.UniverseModel.UniverseTime + 2700);
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
                yield return WarpUntil(night);
                yield return new WaitForSecondsRealtime(4f);
                yield return Shot("v1-ksc-night-before-vab");
            }
            bool orbital = scenario == "rotation" || scenario == "rcs" || scenario == "docking" || scenario == "thrustwarp" || scenario == "warp" || scenario == "burn" || scenario == "jetpack";
            var snapshot = IOProvider.FromJsonFile<OABHistoricalSnapshot>(Path.Combine(dir, scenario == "rcs" ? "craft-rcs.json" : scenario == "jet" ? "craft-jet.json" : scenario == "rover" ? "craft-rover.json" : scenario == "legs" ? "craft-legs.json" : scenario == "fuelline" ? "craft-fuelline.json" : scenario == "crossfeed" ? "craft-crossfeed.json" : scenario == "transfer" ? "craft-crossfeed.json" : (scenario == "stack" || scenario == "thrustwarp") ? "craft-stack.json" : scenario == "reentry" ? "craft-reentry.json" : scenario == "docking" ? "craft-docking.json" : scenario.StartsWith("drop ") ? "craft-drop-" + scenario.Split(' ')[1] + ".json" : scenario.StartsWith("sepland ") ? "craft-sepland.json" : orbital ? "craft-rotation.json" : (scenario == "k1" || scenario == "partdump") ? "craft-k1.json" : "craft.json"));
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
            if (scenario == "jet")
            {
                // a plane: the hangar orientation and the runway, as the stock assembly's own launch site
                G.OAB.Current.eventsUI.OnSetOABOrientationMode?.Invoke(OABOrientation.AIRPLANE);
                G.OAB.SetLaunchSite(OABProvider.LaunchLocationNameToEnum("Runway_1"));
                yield return new WaitForSeconds(2f);
            }
            G.OAB.Current.eventsUI.OnLaunch?.Invoke();
            yield return WaitState(GameState.FlightView, 600);
            VesselComponent vessel = null;
            t = 0;
            while ((vessel = G.ViewController.GetActiveSimVessel()) == null && t < 120) { t += Time.unscaledDeltaTime; yield return null; }
            yield return new WaitForSeconds(12f);
            Mark("in flight, situation " + vessel.Situation + ", mass " + vessel.totalMass + ", alt " + vessel.AltitudeFromSeaLevel);
            yield return Shot(orbital ? "04-orbit" : "04-pad");
            if (scenario == "rcs") { yield return Rcs(vessel); yield break; }
            if (scenario == "docking") { yield return DockingScenario.Run(vessel, dir, Mark); yield return Finish("docking"); yield break; }
            if (scenario == "thrustwarp") { yield return ThrustWarp(vessel); yield break; }
            if (scenario == "jet")
            {
                yield return JetScenario.Run(this, vessel, dir, Mark);
                Mark("finished jet");
                File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
                yield return new WaitForSeconds(3f);
                Application.Quit();
                yield break;
            }
            if (scenario == "warp") { yield return OrbitScenarios.Warp(vessel, dir, Mark); yield return Finish("warp"); yield break; }
            if (scenario == "burn") { yield return OrbitScenarios.Burn(vessel, dir, Mark); yield return Finish("burn"); yield break; }
            if (scenario == "jetpack") { yield return OrbitScenarios.Jetpack(vessel, dir, Mark, (v, f) => Begin(v, f), Stop); yield return Finish("jetpack"); yield break; }
            if (orbital) { yield return Rotation(vessel); yield break; }
            if (scenario == "rover") { yield return Rover(vessel); yield break; }
            if (scenario == "eva") { yield return Eva(vessel); yield break; }
            if (scenario == "partdump") { yield return PartDump(vessel); yield break; }
            if (scenario == "legs") { yield return Legs(vessel); yield break; }
            if (scenario == "stack") { yield return Stack(vessel); yield break; }
            if (scenario == "fuelline" || scenario == "crossfeed") { yield return FuelLine(vessel); yield break; }
            if (scenario == "transfer") { yield return Transfer(vessel); yield break; }
            if (scenario == "reentry") { yield return Reentry(vessel); yield break; }
            if (scenario.StartsWith("drop ")) { yield return Drop(vessel, double.Parse(scenario.Split(' ')[2], CultureInfo.InvariantCulture)); yield break; }
            if (scenario.StartsWith("sepland ")) { yield return SepLand(vessel, double.Parse(scenario.Split(' ')[1], CultureInfo.InvariantCulture)); yield break; }
            if (scenario == "visuals")
            {
                SetGroup(KSPActionGroup.Lights, true);
                yield return new WaitForSecondsRealtime(3f);
                yield return Shot("v3-pad-night-lights");
                // morning for the flight
                double day = brightUT; while (day < G.UniverseModel.UniverseTime + 60) day += 21600;
                yield return WarpUntil(day);
                yield return new WaitForSecondsRealtime(4f);
                yield return Shot("v3-pad-day");
            }
            if (scenario == "hop") { yield return Hop(vessel); yield break; }
            if (scenario == "k1") { yield return Staged(vessel); yield break; }

            vessel.SetAutopilotEnableDisable(true);
            G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(0.5f);
            Begin(vessel);
            // T3: the vessel's centre of mass in the root part's frame every tick, as the tanks drain
            if (scenario == "com")
            {
                comLog = new StreamWriter(Path.Combine(dir, "out", "com-trace.csv"), false) { AutoFlush = true };
                comLog.WriteLine("t,mass_t,parts,com_x,com_y,com_z,resource_t");
            }
            vessel.ActivateNextStage();
            Mark("staged");

            double[] marks = scenario == "visuals" ? new double[] { 15000, 45000 } : new double[] { 150, 1500, 8000, 25000 };
            string[] names = scenario == "visuals" ? new[] { "v4-flight-15km", "v4-flight-45km" } : new[] { "05-liftoff", "06-ascent-1km", "07-ascent-8km", "08-ascent-25km" };
            bool re40 = false, re25 = false, dumped10 = false;
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
                if (apo && !dumped10 && alt < 10000) { dumped10 = true; DumpParts(vessel, "parts-descent-10km.csv"); DumpForces(vessel, "forces-descent-10km.csv"); Mark("parts dumped at " + alt.ToString("F0", CultureInfo.InvariantCulture)); }
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
            SetGroup(KSPActionGroup.RCS, true);
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
            SetGroup(KSPActionGroup.Brakes, true);
            yield return Drive(8f, 0f, 0f);
            SetGroup(KSPActionGroup.Brakes, false);
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
            Mark("let go of the ladder");
            InputHolder.EvaJump = true; yield return new WaitForSeconds(0.3f); InputHolder.EvaJump = false;
            yield return new WaitForSeconds(4f);
            yield return Shot("13b-eva-on-ground");
            Mark("walk forward"); InputHolder.EvaFrontBack = 1f; yield return new WaitForSeconds(6f);
            InputHolder.EvaFrontBack = 0f; Mark("stand"); yield return new WaitForSeconds(3f);
            Mark("run forward"); InputHolder.EvaRun = true; InputHolder.EvaFrontBack = 1f; yield return new WaitForSeconds(6f);
            InputHolder.EvaFrontBack = 0f; InputHolder.EvaRun = false; Mark("stand"); yield return new WaitForSeconds(3f);
            Mark("jump"); InputHolder.EvaJump = true; yield return new WaitForSeconds(0.2f); InputHolder.EvaJump = false;
            yield return new WaitForSeconds(4f);
            yield return Shot("13c-eva-close");
            Mark("walk back"); InputHolder.EvaFrontBack = -1f; yield return new WaitForSeconds(4f);
            InputHolder.EvaFrontBack = 0f; yield return new WaitForSeconds(2f);
            yield return Shot("14-eva-end");
            Stop();
            Mark("finished eva");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(3f);
            Application.Quit();
        }

        // Per-part mass and drag of the craft standing on the pad, for comparing KSP2's mass model with OSE's.
        // Time warp by rate index (WarpTo crawls when no vessel is flying): fast until close, then 10x, then stop.
        IEnumerator WarpUntil(double target)
        {
            var tw = G.ViewController.TimeWarp;
            float w = 0;
            while (G.UniverseModel.UniverseTime < target && w < 300)
            {
                double left = target - G.UniverseModel.UniverseTime;
                tw.SetRateIndex(left > 3000 ? 7 : left > 300 ? 5 : left > 20 ? 3 : 1, true);
                w += Time.unscaledDeltaTime;
                yield return null;
            }
            tw.StopTimeWarp(true);
            Mark("warped to " + G.UniverseModel.UniverseTime.ToString("F0", CultureInfo.InvariantCulture) + " (target " + target.ToString("F0", CultureInfo.InvariantCulture) + ")");
        }

        // Every force on every part at this tick: the rigidbody's persistent and single-frame IForce lists, plus
        // Module_Drag's scalar and the global drag multipliers it uses.
        void DumpForces(VesselComponent vessel, string file)
        {
            var inv = CultureInfo.InvariantCulture;
            using (var w = new StreamWriter(Path.Combine(dir, "out", file), false))
            {
                double rho = vessel.AtmDensity, v = vessel.SrfSpeedMagnitude;
                w.WriteLine("# mach " + vessel.MachNumber.ToString("R", inv) + " q_kpa " + vessel.DynamicPressure_kPa.ToString("R", inv) + " alt " + vessel.AltitudeFromSeaLevel.ToString("R", inv)
                    + " density " + rho.ToString("R", inv) + " speed " + v.ToString("R", inv) + " mass_t " + vessel.totalMass.ToString("R", inv)
                    + " DragMultiplier " + PhysicsSettings.DragMultiplier.ToString("R", inv) + " DragCubeMultiplier " + PhysicsSettings.DragCubeMultiplier.ToString("R", inv)
                    + " LiftDragMultiplier " + PhysicsSettings.LiftDragMultiplier.ToString("R", inv) + " PseudoReynolds(rho*v) " + PhysicsSettings.DragCurvePseudoReynolds.Evaluate((float)(rho * v)).ToString("R", inv));
                w.WriteLine("part,list,force_type,mode,fx,fy,fz,|f|_kN,px,py,pz,drag_scalar");
                foreach (var part in vessel.SimulationObject.PartOwner.Parts)
                {
                    if (!G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) continue;
                    var pb = view.GetComponent<PartBehavior>();
                    var md = view.GetComponent<KSP.Modules.Module_Drag>();
                    string ds = md != null ? md.dragScalar.ToString("R", inv) : "";
                    var rb = pb?.Rigidbody?.Model;
                    if (rb == null) { w.WriteLine(part.Name + ",none,,,,,,,,,," + ds); continue; }
                    foreach (var (list, name) in new[] { (rb.Forces, "persistent"), (rb.SingleFrameForces, "single") })
                        foreach (var f in list)
                        {
                            var F = f.RelativeForce; var P = f.RelativePosition;
                            w.WriteLine(part.Name + "," + name + "," + f.GetType().FullName + "," + f.ForceMode + "," + F.x.ToString("R", inv) + "," + F.y.ToString("R", inv) + "," + F.z.ToString("R", inv) + ","
                                + F.magnitude.ToString("R", inv) + "," + P.x.ToString("R", inv) + "," + P.y.ToString("R", inv) + "," + P.z.ToString("R", inv) + "," + ds);
                        }
                }
            }
        }

        void DumpParts(VesselComponent vessel, string file)
        {
            var flow = vessel.SurfaceVelocity.vector;
            using (var w = new StreamWriter(Path.Combine(dir, "out", file), false))
            {
                w.WriteLine("index,name,physics_mode,dry_t,resource_t,green_t,total_t,physics_mass_t,drag_area_xp,drag_area_xn,drag_area_yp,drag_area_yn,drag_area_zp,drag_area_zn,drag_cd_xp,drag_cd_xn,drag_cd_yp,drag_cd_yn,drag_cd_zp,drag_cd_zn,drag_enabled,exposed_xp,exposed_xn,exposed_yp,exposed_yn,exposed_zp,exposed_zn,drag_mult_yp,drag_mult_yn,mach,q_kpa,alt_m");
                int i = 0;
                foreach (var part in vessel.SimulationObject.PartOwner.Parts)
                {
                    string drag = string.Join(",", new string[24]);
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
                            foreach (var e in d.weightedExposedArea) vals.Add(e.ToString("R", CultureInfo.InvariantCulture));
                            vals.Add(d.weightedDragMultiplier[2].ToString("R", CultureInfo.InvariantCulture));
                            vals.Add(d.weightedDragMultiplier[3].ToString("R", CultureInfo.InvariantCulture));
                            vals.Add(vessel.MachNumber.ToString("R", CultureInfo.InvariantCulture));
                            vals.Add(vessel.DynamicPressure_kPa.ToString("R", CultureInfo.InvariantCulture));
                            vals.Add(vessel.AltitudeFromSeaLevel.ToString("R", CultureInfo.InvariantCulture));
                            drag = string.Join(",", vals);
                        }
                    }
                    double[] mass = { part.DryMass, part.ResourceMass, part.GreenMass, part.TotalMass, part.PhysicsMass };
                    w.WriteLine(i++ + "," + part.Name + "," + part.PhysicsMode + "," + string.Join(",", Array.ConvertAll(mass, x => x.ToString("R", CultureInfo.InvariantCulture))) + "," + drag);
                }
            }
        }

        IEnumerator PartDump(VesselComponent vessel)
        {
            yield return new WaitForSeconds(5f);
            DumpParts(vessel, "parts.csv");
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
            SetGroup(KSPActionGroup.Gear, true);
            vessel.SetAutopilotEnableDisable(true);
            Begin(vessel, "legs-trace.csv");
            yield return new WaitForSeconds(8f);
            yield return Shot("15-legs-deployed");
            yield return new WaitForSeconds(2f);
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

        // Action groups through the active vehicle's controllable groups, as the flight keys do
        // (VesselComponent.SetActionGroup does not reach them for a freshly launched craft).
        // T14 crash tolerance: put the craft 0.5 m above where it rests, upright, falling at `speed`, and log
        // the parts aboard every tick until 8 s after the drop.
        IEnumerator Drop(VesselComponent vessel, double speed)
        {
            var before = new System.Collections.Generic.List<string>();
            foreach (var part in vessel.SimulationObject.PartOwner.Parts) before.Add(part.Name);
            Mark("parts before " + string.Join(" ", before));
            var state = new KSP.Sim.State.SurfaceTeleportState { ReferenceBodyGuid = vessel.mainBody.Name, Altitude = 1.0 /* above the terrain, not sea level */, Latitude = vessel.Latitude, Longitude = vessel.Longitude, VerticalSpeed = -speed };
            Begin(vessel, "drop-trace.csv");
            using (var log = new StreamWriter(Path.Combine(dir, "out", "drop-parts.csv"), false))
            {
                log.WriteLine("t,parts,names");
                G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject, state);
                Mark("teleported, falling at " + speed.ToString(CultureInfo.InvariantCulture));
                float t = 0; int last = -1;
                while (t < 12)
                {
                    yield return new WaitForFixedUpdate();
                    t = (float)(G.UniverseModel.UniverseTime - start); // game time since Begin: WaitForFixedUpdate resumes once a frame, not once a tick
                    var v = G.ViewController.GetActiveSimVessel();
                    if (v == null) { log.WriteLine(t.ToString("F2", CultureInfo.InvariantCulture) + ",0,"); Mark("vessel gone at " + t.ToString("F2", CultureInfo.InvariantCulture)); break; }
                    var names = new System.Collections.Generic.List<string>();
                    foreach (var part in v.SimulationObject.PartOwner.Parts) names.Add(part.Name);
                    if (names.Count != last) { log.WriteLine(t.ToString("F2", CultureInfo.InvariantCulture) + "," + names.Count + "," + string.Join(" ", names)); last = names.Count; }
                }
            }
            yield return Shot("16-after-drop");
            Stop();
            Mark("finished drop");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(2f);
            Application.Quit();
        }

        // T13 joints: pod + six 1x4 tanks + Swivel; SAS on, full throttle 5 s, then full roll 3 s, then 3 s free.
        // Logs, every physics tick, each part's position in the pod's frame (pod = root) and the part count.
        // T40 warp under thrust: the stack craft (pod, six 1x4 tanks, Swivel) in orbit, SAS on, full throttle;
        // 10 s at 1x, then 30 game seconds at 10x and 100 at 100x, then 1x for 5 s, throttle off. Logs every tick.
        IEnumerator ThrustWarp(VesselComponent vessel)
        {
            var inv = CultureInfo.InvariantCulture;
            vessel.SetAutopilotEnableDisable(true);
            G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(1f);
            Begin(vessel, "thrustwarp-trace.csv");
            vessel.ActivateNextStage();
            Mark("staged, mass " + vessel.totalMass.ToString("R", inv) + " speed " + vessel.OrbitalSpeed.ToString("R", inv));
            var tw = G.ViewController.TimeWarp;
            double t0 = G.UniverseModel.UniverseTime;
            float w = 0;
            while (G.UniverseModel.UniverseTime < t0 + 10 && w < 30) { w += Time.unscaledDeltaTime; yield return null; }
            foreach (var step in new[] { (3, 30.0), (5, 100.0) })
            {
                double s = G.UniverseModel.UniverseTime;
                tw.SetRateIndex(step.Item1, true);
                w = 0;
                bool told = false;
                while (G.UniverseModel.UniverseTime < s + step.Item2 && w < 120)
                {
                    w += Time.unscaledDeltaTime;
                    if (!told && w > 2) { told = true; Mark("rate index " + tw.CurrentRateIndex + " rate " + tw.CurrentRate.ToString("R", inv) + " physics warp " + tw.IsPhysicsTimeWarp + " under-thrust orbital " + vessel.IsOrbitalPhysicsUnderThrustActive); }
                    yield return null;
                }
                Mark("after rate index " + step.Item1 + ": ut " + (G.UniverseModel.UniverseTime - t0).ToString("F2", inv) + " mass " + vessel.totalMass.ToString("R", inv) + " speed " + vessel.OrbitalSpeed.ToString("R", inv) + " rate " + tw.CurrentRate.ToString("R", inv));
            }
            tw.StopTimeWarp(true);
            yield return new WaitForSeconds(5f);
            G.ViewController.flightInputHandler.OverrideInputThrottleCutoff(true);
            yield return new WaitForSeconds(2f);
            Mark("end: mass " + vessel.totalMass.ToString("R", inv) + " speed " + vessel.OrbitalSpeed.ToString("R", inv));
            Stop();
            yield return Finish("thrustwarp");
        }

        IEnumerator Stack(VesselComponent vessel)
        {
            vessel.SetAutopilotEnableDisable(true);
            G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(0.5f);
            Begin(vessel, "stack-trace.csv");
            var inv = CultureInfo.InvariantCulture;
            using (var w = new StreamWriter(Path.Combine(dir, "out", "stack-parts.csv"), false))
            {
                w.WriteLine("t,phase,parts,index,name,x,y,z");
                vessel.ActivateNextStage();
                Mark("staged");
                float t = 0; string phase = "thrust";
                while (t < 11.5f)
                {
                    yield return new WaitForFixedUpdate();
                    t = (float)(G.UniverseModel.UniverseTime - start); // game time since Begin: WaitForFixedUpdate resumes once a frame, not once a tick
                    if (phase == "thrust" && t >= 5f) { phase = "roll"; InputHolder.Roll = 1f; Mark("roll"); }
                    if (phase == "roll" && t >= 8f) { phase = "free"; InputHolder.Roll = 0f; Mark("roll off"); }
                    var v = G.ViewController.GetActiveSimVessel();
                    if (v == null) { Mark("vessel gone"); break; }
                    Transform root = null; var views = new System.Collections.Generic.List<(string, Transform)>();
                    foreach (var part in v.SimulationObject.PartOwner.Parts)
                        if (G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) { views.Add((part.Name, view.transform)); if (root == null) root = view.transform; }
                    if (root == null) continue;
                    for (int i = 0; i < views.Count; i++)
                    {
                        var rel = Quaternion.Inverse(root.rotation) * (views[i].Item2.position - root.position);
                        w.WriteLine(t.ToString("F2", inv) + "," + phase + "," + views.Count + "," + i + "," + views[i].Item1 + "," + rel.x.ToString("R", inv) + "," + rel.y.ToString("R", inv) + "," + rel.z.ToString("R", inv));
                    }
                }
            }
            InputHolder.Roll = 0f;
            yield return Shot("17-stack");
            Stop();
            Mark("finished stack");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(2f);
            Application.Quit();
        }

        // Craft-on-craft contact: the pod over a decoupler over a 1x2 tank, put about 1 m above the pad falling at
        // `speed`, decoupled at once; the tank lands and the pod comes down on it. Every tick, every vessel within
        // 50 m: its parts, height, vertical and horizontal speed and situation (sepland-vessels.csv), for 10 s.
        IEnumerator SepLand(VesselComponent vessel, double speed)
        {
            var inv = CultureInfo.InvariantCulture;
            var state = new KSP.Sim.State.SurfaceTeleportState { ReferenceBodyGuid = vessel.mainBody.Name, Altitude = 1.0, Latitude = vessel.Latitude, Longitude = vessel.Longitude, VerticalSpeed = -speed };
            Begin(vessel, "drop-trace.csv");
            using (var w = new StreamWriter(Path.Combine(dir, "out", "sepland-vessels.csv"), false))
            {
                w.WriteLine("t,vessel,parts,names,alt_m,vspeed,hspeed,situation,ut");
                G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject, state);
                Mark("teleported, falling at " + speed.ToString(inv));
                bool staged = false;
                float t = 0;
                while (t < 10f)
                {
                    yield return new WaitForFixedUpdate();
                    t = (float)(G.UniverseModel.UniverseTime - start);
                    var active = G.ViewController.GetActiveSimVessel();
                    if (!staged && active != null && active.Situation == VesselSituations.Flying && active.VerticalSrfSpeed < -0.5)
                    {
                        active.ActivateNextStage();
                        staged = true;
                        Mark("decoupled at " + t.ToString("F2", inv));
                    }
                    var here = active != null ? active.SimulationObject.Position : vessel.SimulationObject.Position;
                    foreach (var v in G.UniverseModel.GetAllVesselsInRange(here, 50.0))
                    {
                        var names = new System.Collections.Generic.List<string>();
                        foreach (var part in v.SimulationObject.PartOwner.Parts) names.Add(part.Name);
                        w.WriteLine(t.ToString("F3", inv) + "," + v.GlobalId.ToString().Substring(0, 8) + "," + names.Count + "," + string.Join(" ", names) + ","
                            + v.AltitudeFromSeaLevel.ToString("R", inv) + "," + v.VerticalSrfSpeed.ToString("R", inv) + "," + v.HorizontalSrfSpeed.ToString("R", inv) + "," + v.Situation + ","
                            + G.UniverseModel.UniverseTime.ToString("R", inv));
                    }
                }
            }
            yield return Shot("20-sepland");
            Stop();
            yield return Finish("sepland");
        }

        // the end of a scenario: a last screenshot, the done marker, quit
        IEnumerator Finish(string name)
        {
            yield return Shot("99-" + name + "-end");
            Mark("finished " + name);
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(3f);
            Application.Quit();
        }

        // T39 fuel lines: pod, tank A, inline decoupler (crossfeed off), tank B, Swivel, and a fuel line from A
        // to B (craft-fuelline.json). SAS on, full throttle, engine staged; every physics tick logs each part's
        // stored units per resource, until the engine has had nothing to burn for 3 s or 240 s pass.
        IEnumerator FuelLine(VesselComponent vessel)
        {
            vessel.SetAutopilotEnableDisable(true);
            G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(0.5f);
            Begin(vessel, "fuelline-trace.csv");
            var inv = CultureInfo.InvariantCulture;
            var db = G.ResourceDefinitionDatabase;
            using (var w = new StreamWriter(Path.Combine(dir, "out", "fuelline-tanks.csv"), false))
            {
                w.WriteLine("t,index,name,resource,stored_units,capacity_units,resource_mass_t");
                vessel.ActivateNextStage();
                Mark("staged");
                float t = 0, idle = 0; double lastTotal = double.NaN;
                while (t < 240f && idle < 3f)
                {
                    yield return new WaitForFixedUpdate();
                    t += Time.fixedDeltaTime;
                    var v = G.ViewController.GetActiveSimVessel();
                    if (v == null) { Mark("vessel gone"); break; }
                    int i = 0; double total = 0;
                    foreach (var part in v.SimulationObject.PartOwner.Parts)
                    {
                        var c = part.PartResourceContainer;
                        if (c != null)
                            foreach (var r in c.GetAllResourcesContainedData())
                            {
                                if (r.CapacityUnits <= 0) continue;
                                w.WriteLine(t.ToString("F2", inv) + "," + i + "," + part.Name + "," + db.GetResourceNameFromID(r.ResourceID) + ","
                                    + r.StoredUnits.ToString("R", inv) + "," + r.CapacityUnits.ToString("R", inv) + "," + part.ResourceMass.ToString("R", inv));
                                if (part.Name.StartsWith("fueltank_")) total += r.StoredUnits;
                            }
                        i++;
                    }
                    idle = !double.IsNaN(lastTotal) && Math.Abs(total - lastTotal) < 1e-9 ? idle + Time.fixedDeltaTime : 0f;
                    lastTotal = total;
                }
                Mark("fuel run ended at t " + t.ToString("F1", inv));
            }
            yield return Shot("18-fuelline");
            Stop();
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(2f);
            Application.Quit();
        }

        // G06 Resource Manager: on the pad, the crossfeed craft's two tanks (A above the decoupler, B below).
        // Phase "inout": A set to a quarter, B full; B Out, A In. Phase "balance": A a tenth, B nine tenths;
        // both Balance. Each phase issues the requests KSP2's ResourceManagerUI.StartResourceTransfer makes
        // (its split for two tanks, NO_FLOW per part, FlowUnits = TargetUnits) through each part's own
        // ResourceFlowRequestBroker, and logs both tanks' stored units every physics tick for 4 s.
        IEnumerator Transfer(VesselComponent vessel)
        {
            yield return new WaitForSeconds(2f);
            var inv = CultureInfo.InvariantCulture;
            var db = G.ResourceDefinitionDatabase;
            var tanks = new System.Collections.Generic.List<PartComponent>();
            foreach (var part in vessel.SimulationObject.PartOwner.Parts)
                if (part.Name.StartsWith("fueltank_")) tanks.Add(part);
            if (tanks.Count != 2) { Mark("expected 2 tanks, found " + tanks.Count); Application.Quit(); yield break; }
            var a = tanks[0]; var b = tanks[1];
            var resources = new System.Collections.Generic.List<KSP.Sim.ResourceSystem.ResourceDefinitionID>();
            foreach (var r in a.PartResourceContainer.GetAllResourcesContainedData()) resources.Add(r.ResourceID);
            using (var w = new StreamWriter(Path.Combine(dir, "out", "transfer-tanks.csv"), false))
            {
                w.WriteLine("phase,t,tank,resource,stored_units,capacity_units");
                foreach (var phase in new[] { "inout", "balance" })
                {
                    double fa = phase == "inout" ? 0.25 : 0.1, fb = phase == "inout" ? 1.0 : 0.9;
                    foreach (var id in resources)
                    {
                        a.PartResourceContainer.SetResourceStoredUnits(id, a.PartResourceContainer.GetResourceCapacityUnits(id) * fa);
                        b.PartResourceContainer.SetResourceStoredUnits(id, b.PartResourceContainer.GetResourceCapacityUnits(id) * fb);
                    }
                    yield return new WaitForFixedUpdate();
                    var cmdsA = new System.Collections.Generic.List<KSP.Sim.ResourceSystem.ResourceFlowRequestCommandConfig>();
                    var cmdsB = new System.Collections.Generic.List<KSP.Sim.ResourceSystem.ResourceFlowRequestCommandConfig>();
                    foreach (var id in resources)
                    {
                        double sa = a.PartResourceContainer.GetResourceStoredUnits(id), ca = a.PartResourceContainer.GetResourceCapacityUnits(id);
                        double sb = b.PartResourceContainer.GetResourceStoredUnits(id);
                        double moveA, moveB; // + in, - out
                        if (phase == "inout") { double room = ca - sa; double m = Math.Min(room, sb); moveA = m; moveB = -m; }
                        else { double avg = (sa + sb) / 2; moveA = avg - sa; moveB = avg - sb; }
                        foreach (var (list, mv) in new[] { (cmdsA, moveA), (cmdsB, moveB) })
                            if (Math.Abs(mv) > 0)
                                list.Add(new KSP.Sim.ResourceSystem.ResourceFlowRequestCommandConfig
                                {
                                    FlowResource = id,
                                    FlowDirection = mv > 0 ? KSP.Sim.ResourceSystem.FlowDirection.FLOW_INBOUND : KSP.Sim.ResourceSystem.FlowDirection.FLOW_OUTBOUND,
                                    FlowUnits = Math.Abs(mv),
                                    TargetUnits = Math.Abs(mv),
                                    FlowModeOverride = KSP.Sim.ResourceSystem.ResourceFlowMode.NO_FLOW
                                });
                    }
                    var handles = new System.Collections.Generic.List<(KSP.Sim.ResourceSystem.ResourceFlowRequestBroker, KSP.Sim.ResourceSystem.ResourceFlowRequestHandle)>();
                    foreach (var (part, cmds) in new[] { (a, cmdsA), (b, cmdsB) })
                    {
                        if (cmds.Count == 0) continue;
                        var broker = part.PartResourceFlowRequestBroker;
                        var h = broker.AllocateOrGetRequest();
                        broker.SetCommands(h, 1.0, cmds.ToArray());
                        broker.SetRequestActive(h);
                        handles.Add((broker, h));
                    }
                    Mark("transfer " + phase + " started, " + (cmdsA.Count + cmdsB.Count) + " commands");
                    float t = 0;
                    while (t < 4f)
                    {
                        foreach (var (name, part) in new[] { ("A", a), ("B", b) })
                            foreach (var id in resources)
                                w.WriteLine(phase + "," + t.ToString("F2", inv) + "," + name + "," + db.GetResourceNameFromID(id) + ","
                                    + part.PartResourceContainer.GetResourceStoredUnits(id).ToString("R", inv) + ","
                                    + part.PartResourceContainer.GetResourceCapacityUnits(id).ToString("R", inv));
                        yield return new WaitForFixedUpdate();
                        t += Time.fixedDeltaTime;
                    }
                    foreach (var (broker, h) in handles) broker.SetRequestInactive(h);
                    Mark("transfer " + phase + " stopped");
                }
            }
            yield return Shot("19-transfer");
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            yield return new WaitForSeconds(2f);
            Application.Quit();
        }

        static void SetGroup(KSPActionGroup group, bool on)
        {
            if (G.ViewController.TryGetActiveVehicle(out var v)) v.ControllableActionGroups?.SetGroup(group, on);
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
                if (comLog != null) LogCom(vessel, ut - start);
            }
            catch (Exception e) { Logger.LogError(e); Stop(); }
        }

        // the centre of mass (PartOwnerComponent.CenterOfMass) in the root part's frame, Unity axes
        void LogCom(VesselComponent vessel, double t)
        {
            var inv = CultureInfo.InvariantCulture;
            Transform root = null; double res = 0; int n = 0;
            foreach (var part in vessel.SimulationObject.PartOwner.Parts)
            {
                n++;
                res += part.ResourceMass;
                if (root == null && G.SpaceSimulation.TryGetViewObject(part.SimulationObject, out var view)) root = view.transform;
            }
            if (root == null) return;
            Vector3 c = G.UniverseView.PhysicsSpace.PositionToPhysics(vessel.CenterOfMass);
            var rel = Quaternion.Inverse(root.rotation) * (c - root.position);
            comLog.WriteLine(t.ToString("R", inv) + "," + vessel.totalMass.ToString("R", inv) + "," + n + "," + rel.x.ToString("R", inv) + "," + rel.y.ToString("R", inv) + "," + rel.z.ToString("R", inv) + "," + res.ToString("R", inv));
        }

        // T11/T15 heat shield: the Mk1 pod over a 1.25 m heat shield, held upright by SAS, put 70 km above the
        // ground falling at 2,300 m/s; every tick each part's temperature, re-entry flux and ablator, until the
        // craft is down or gone (or 6 minutes).
        IEnumerator Reentry(VesselComponent vessel)
        {
            vessel.SetAutopilotEnableDisable(true);
            yield return new WaitForSeconds(0.5f);
            var state = new KSP.Sim.State.SurfaceTeleportState { ReferenceBodyGuid = vessel.mainBody.Name, Altitude = 70000.0, Latitude = vessel.Latitude, Longitude = vessel.Longitude, VerticalSpeed = -2300.0 };
            Begin(vessel, "reentry-trace.csv");
            var inv = CultureInfo.InvariantCulture;
            var db = G.ResourceDefinitionDatabase;
            using (var w = new StreamWriter(Path.Combine(dir, "out", "reentry-parts.csv"), false))
            {
                w.WriteLine("t,alt_m,mach,density,part,temp_k,max_temp_k,reentry_flux,ablator_units,ablator_capacity");
                G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject, state);
                Mark("teleported to 70 km, falling at 2300");
                float t = 0; bool shot60 = false, shot30 = false;
                while (t < 360)
                {
                    yield return new WaitForFixedUpdate();
                    t = (float)(G.UniverseModel.UniverseTime - start); // game time since Begin: WaitForFixedUpdate resumes once a frame, not once a tick
                    var v = G.ViewController.GetActiveSimVessel();
                    if (v == null || v.GlobalId.ToString() != vesselId) { Mark("vessel gone at " + t.ToString("F2", inv)); break; }
                    foreach (var part in v.SimulationObject.PartOwner.Parts)
                    {
                        double units = 0, cap = 0;
                        foreach (var r in part.PartResourceContainer.GetAllResourcesContainedData())
                            if (db.GetResourceNameFromID(r.ResourceID) == "Ablator") { units += r.StoredUnits; cap += r.CapacityUnits; }
                        w.WriteLine(t.ToString("F2", inv) + "," + v.AltitudeFromSeaLevel.ToString("R", inv) + "," + v.MachNumber.ToString("R", inv) + "," + v.AtmDensity.ToString("R", inv) + "," + part.Name + "," + part.Temperature.ToString("R", inv) + "," + part.MaxTemp.ToString("R", inv) + "," + part.ThermalData.ReentryFlux.ToString("R", inv) + "," + units.ToString("R", inv) + "," + cap.ToString("R", inv));
                    }
                    if (!shot60 && v.AltitudeFromSeaLevel < 60000) { shot60 = true; yield return Shot("18-reentry-60km"); }
                    if (!shot30 && v.AltitudeFromSeaLevel < 30000) { shot30 = true; yield return Shot("19-reentry-30km"); }
                    if (v.Situation == VesselSituations.Landed || v.Situation == VesselSituations.Splashed) { Mark("down " + v.Situation + " at " + t.ToString("F2", inv)); break; }
                }
            }
            Stop();
            yield return Finish("reentry");
        }

        void Stop()
        {
            if (comLog != null) { comLog.Dispose(); comLog = null; }
            if (trace == null) return;
            trace.Dispose(); trace = null;
        }
        void OnDestroy() { Stop(); }
    }
}
