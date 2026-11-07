using System;
using System.Collections;
using System.Globalization;
using System.IO;
using BepInEx;
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
            bool orbital = scenario == "rotation" || scenario == "rcs";
            var snapshot = IOProvider.FromJsonFile<OABHistoricalSnapshot>(Path.Combine(dir, scenario == "rcs" ? "craft-rcs.json" : scenario == "rover" ? "craft-rover.json" : orbital ? "craft-rotation.json" : scenario == "k1" ? "craft-k1.json" : "craft.json"));
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
            if (scenario == "hop") { yield return Hop(vessel); yield break; }
            if (scenario == "k1") { yield return Staged(vessel); yield break; }

            vessel.SetAutopilotEnableDisable(true);
            G.ViewController.flightInputHandler.OverrideInputMaxThrottle(true);
            yield return new WaitForSeconds(0.5f);
            Begin(vessel);
            vessel.ActivateNextStage();
            Mark("staged");

            double[] marks = { 150, 1500, 8000, 25000 };
            string[] names = { "05-liftoff", "06-ascent-1km", "07-ascent-8km", "08-ascent-25km" };
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
                if (apo && !mapped && alt < 30000 && top > 31000)
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
            trace.WriteLine("t,alt_m,vspeed,srf_speed,mach,q_kpa,density,static_kpa,mass_t,throttle,qx,qy,qz,qw,wx,wy,wz,sas,pitch,yaw,roll,vx,vy,vz,gx,gy,gz,tx,ty,tz,wheel_throttle,wheel_steer,situation,ut");
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
                var gv = Vector.Reframed(vessel.gravityForPos, frame.localFrame).vector;
                double[] v = { ut - start, vessel.AltitudeFromSeaLevel, vessel.VerticalSrfSpeed, vessel.SrfSpeedMagnitude, vessel.MachNumber,
                    vessel.DynamicPressure_kPa, vessel.AtmDensity, vessel.StaticPressure_kPa, vessel.totalMass, vessel.flightCtrlState.mainThrottle,
                    q.x, q.y, q.z, q.w, w.x, w.y, w.z, vessel.Autopilot != null && vessel.Autopilot.Enabled ? 1 : 0,
                    vessel.flightCtrlState.pitch, vessel.flightCtrlState.yaw, vessel.flightCtrlState.roll,
                    ov.x, ov.y, ov.z, gv.x, gv.y, gv.z, vessel.flightCtrlState.X, vessel.flightCtrlState.Y, vessel.flightCtrlState.Z,
                    vessel.flightCtrlState.wheelThrottle, vessel.flightCtrlState.wheelSteer };
                trace.WriteLine(string.Join(",", Array.ConvertAll(v, x => x.ToString("R", CultureInfo.InvariantCulture))) + "," + vessel.Situation + "," + ut.ToString("R", CultureInfo.InvariantCulture));
            }
            catch (Exception e) { Logger.LogError(e); Stop(); }
        }

        void Stop() { if (trace == null) return; trace.Dispose(); trace = null; }
        void OnDestroy() { Stop(); }
    }
}
