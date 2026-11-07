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
            bool orbital = scenario == "rotation";
            var snapshot = IOProvider.FromJsonFile<OABHistoricalSnapshot>(Path.Combine(dir, orbital ? "craft-rotation.json" : "craft.json"));
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
            if (orbital) { yield return Rotation(vessel); yield break; }

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
                // the handler consumes an override once per frame, so hold it every frame
                float held = 0;
                while (held < 1f) { Set(axis, 1f); held += Time.deltaTime; yield return null; }
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

        static void Set(string axis, float v)
        {
            if (axis == "pitch") FlightInputHandler.OverrideInputPitch(v);
            else if (axis == "yaw") FlightInputHandler.OverrideInputYaw(v);
            else FlightInputHandler.OverrideInputRoll(v);
        }

        void Begin(VesselComponent vessel, string file = "drag-trace.csv")
        {
            vesselId = vessel.GlobalId.ToString();
            start = G.UniverseModel.UniverseTime;
            last = double.NegativeInfinity;
            trace = new StreamWriter(Path.Combine(dir, "out", file), false) { AutoFlush = true };
            trace.WriteLine("t,alt_m,vspeed,srf_speed,mach,q_kpa,density,static_kpa,mass_t,throttle,qx,qy,qz,qw,wx,wy,wz,sas,pitch,yaw,roll,situation,ut");
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
                double[] v = { ut - start, vessel.AltitudeFromSeaLevel, vessel.VerticalSrfSpeed, vessel.SrfSpeedMagnitude, vessel.MachNumber,
                    vessel.DynamicPressure_kPa, vessel.AtmDensity, vessel.StaticPressure_kPa, vessel.totalMass, vessel.flightCtrlState.mainThrottle,
                    q.x, q.y, q.z, q.w, w.x, w.y, w.z, vessel.Autopilot != null && vessel.Autopilot.Enabled ? 1 : 0,
                    vessel.flightCtrlState.pitch, vessel.flightCtrlState.yaw, vessel.flightCtrlState.roll };
                trace.WriteLine(string.Join(",", Array.ConvertAll(v, x => x.ToString("R", CultureInfo.InvariantCulture))) + "," + vessel.Situation + "," + ut.ToString("R", CultureInfo.InvariantCulture));
            }
            catch (Exception e) { Logger.LogError(e); Stop(); }
        }

        void Stop() { if (trace == null) return; trace.Dispose(); trace = null; }
        void OnDestroy() { Stop(); }
    }
}
