using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;
using BepInEx;
using KSP;
using KSP.Game;
using KSP.IO;
using KSP.Modules;
using KSP.OAB;
using KSP.Sim;
using KSP.Sim.Definitions;
using KSP.Sim.impl;
using KSP.Sim.State;
using Newtonsoft.Json;
using UnityEngine;

namespace OSE.Validation.Services
{
    // Active benchmark, NOT a passive recorder. Runs only in its dedicated empty campaign.
    [BepInPlugin("org.openspaceexplorer.servicesparity", "OSE Services Parity", "0.1.0")]
    [DefaultExecutionOrder(32000)]
    public sealed class ServicesParity : BaseUnityPlugin
    {
        const string Campaign = "OSE Services Validation";
        string dir, phase = "startup";
        StreamWriter samples, events;
        VesselComponent vessel;
        PartComponentModule_DataTransmitter tx;
        readonly List<Module_SolarPanel> panels = new List<Module_SolarPanel>();
        static GameInstance G => GameManager.Instance?.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        static GameState State => G?.GlobalGameState?.GetState() ?? GameState.Invalid;
        static string N(double x) => x.ToString("R", Inv);
        void Awake()
        {
            dir = Path.Combine(Paths.GameRootPath, "BepInEx", "ose-services-validation");
            if (!File.Exists(Path.Combine(dir, "run.txt"))) return;
            Directory.CreateDirectory(Path.Combine(dir, "out"));
            samples = new StreamWriter(Path.Combine(dir, "out", "samples.jsonl")) { AutoFlush = true };
            events = new StreamWriter(Path.Combine(dir, "out", "events.jsonl")) { AutoFlush = true };
            Application.runInBackground = true;
            StartCoroutine(Guard(Run()));
        }
        void Mark(string action, object detail = null)
        {
            events.WriteLine(JsonConvert.SerializeObject(new { action, detail, phase, real_s = Time.realtimeSinceStartup, ut = G?.UniverseModel?.UniverseTime }));
            Logger.LogInfo(action + " " + JsonConvert.SerializeObject(detail));
        }
        IEnumerator Guard(IEnumerator r)
        {
            while (true)
            {
                object c;
                try { if (!r.MoveNext()) break; c = r.Current; }
                catch (Exception e) { Mark("FAILED", e.ToString()); File.WriteAllText(Path.Combine(dir, "out", "failed.txt"), e.ToString()); yield break; }
                if (c is IEnumerator child) yield return Guard(child); else yield return c;
            }
        }
        IEnumerator Wait(GameState target, float seconds = 600)
        {
            float end = Time.realtimeSinceStartup + seconds;
            while (State != target && Time.realtimeSinceStartup < end) yield return null;
            if (State != target) throw new Exception("Waiting for " + target + ", found " + State);
        }
        IEnumerator Hold(string name, float seconds)
        {
            phase = name; Mark("phase");
            double until = G.UniverseModel.UniverseTime + seconds;
            float timeout = Time.realtimeSinceStartup + Math.Max(120, seconds * 30);
            while (G.UniverseModel.UniverseTime < until && Time.realtimeSinceStartup < timeout) yield return null;
            if (G.UniverseModel.UniverseTime < until) throw new Exception("Simulation time stalled in " + name);
        }
        double Charge()
        {
            var id = G.ResourceDefinitionDatabase.GetResourceIDFromName("ElectricCharge");
            return vessel.SimulationObject.PartOwner.Parts.Sum(p => p.PartResourceContainer.GetResourceStoredUnits(id));
        }
        void Fill(double ratio)
        {
            var id = G.ResourceDefinitionDatabase.GetResourceIDFromName("ElectricCharge");
            foreach (var p in vessel.SimulationObject.PartOwner.Parts)
                p.PartResourceContainer.SetResourceStoredUnits(id, p.PartResourceContainer.GetResourceCapacityUnits(id) * ratio);
            Mark("set_charge_fraction", ratio);
        }
        Data_SolarPanel Solar(Module_SolarPanel p) => (Data_SolarPanel)p.DataModules[typeof(Data_SolarPanel)];
        Data_Deployable Deploy(Module_SolarPanel p) => (Data_Deployable)p.DataModules[typeof(Data_Deployable)];
        void Update()
        {
            if (vessel == null || samples == null) return;
            var t = vessel.SimulationObject.Telemetry;
            var solar = panels.Where(p => p != null).Select(p => {
                var d = Solar(p); var a = Deploy(p);
                var pivot = p.panelRotationTransform;
                return new { part = p.name, flow = d.EnergyFlow.GetValue(), scale = d.SimStarEnergyScale,
                    exposure = d.StellarExposure.GetValue(), blocker = d.SimBlockingBody, tracking = a.isTracking,
                    state = a.CurrentDeployState.GetValue().ToString(), anim = a.AnimationNormalizedTime.GetValue(),
                    pivot_rotation = pivot == null ? null : new[] {pivot.localRotation.x,pivot.localRotation.y,pivot.localRotation.z,pivot.localRotation.w} };
            }).ToArray();
            samples.WriteLine(JsonConvert.SerializeObject(new { phase, real_s = Time.realtimeSinceStartup,
                ut = G.UniverseModel.UniverseTime, ec = Charge(), link = t.CommNetConnectionStatus.ToString(),
                range = t.CommNetNode.MaxRange, solar,
                tx_completed = tx?.ActiveTransmissionCompleted, tx_size = tx?.ActiveTransmissionSize }));
        }
        IEnumerator Shot(string name)
        {
            while (G.UI.GetCurtainVisibility()) yield return null;
            yield return new WaitForEndOfFrame();
            ScreenCapture.CaptureScreenshot(Path.Combine(dir, "out", name + ".png"));
            yield return new WaitForSecondsRealtime(0.2f);
        }
        IEnumerator Run()
        {
            bool flagsOnly = File.Exists(Path.Combine(dir, "flags-only.txt"));
            bool solarOnly = File.Exists(Path.Combine(dir, "solar-only.txt"));
            yield return Wait(GameState.MainMenu);
            Screen.SetResolution(960, 540, false);
            yield return new WaitForSecondsRealtime(3);
            string seed = Path.Combine(G.SaveLoadManager.GetCampaignFolderPath(CampaignType.SinglePlayer, Campaign), "seed.json");
            G.SaveLoadManager.LoadGameFromFile(seed, (_, ok) => Mark("seed_loaded", ok));
            yield return Wait(GameState.KerbalSpaceCenter);
            yield return new WaitForSecondsRealtime(4);
            var snapshot = IOProvider.FromJsonFile<OABHistoricalSnapshot>(Path.Combine(dir, "craft.json"));
            bool shown = false;
            G.OAB.ShowOAB(new OABConfig(OABVariant.VAB, flagsOnly ? OABEnvironmentType.Terrestrial : OABEnvironmentType.Orbital, OABConstructionType.User, null), snapshot, null, () => shown = true);
            float deadline = Time.realtimeSinceStartup + 180;
            while (!shown && Time.realtimeSinceStartup < deadline) yield return null;
            if (!shown) throw new Exception("OAB timeout");
            yield return new WaitForSecondsRealtime(3);
            G.OAB.Current.eventsUI.OnLaunch?.Invoke();
            yield return Wait(GameState.FlightView);
            yield return new WaitForSecondsRealtime(8);
            vessel = G.ViewController.GetActiveSimVessel();
            vessel.SetAutopilotEnableDisable(false);
            var transmitters = new List<Data_Transmitter>();
            foreach (var p in vessel.SimulationObject.PartOwner.Parts)
            {
                if (p.TryGetModuleData<PartComponentModule_DataTransmitter, Data_Transmitter>(out var data))
                {
                    transmitters.Add(data);
                    if (p.Name.StartsWith("antenna_")) p.TryGetModule(out tx);
                }
                if (G.SpaceSimulation.TryGetViewObject(p.SimulationObject, out var view))
                    panels.AddRange(view.GetComponents<Module_SolarPanel>());
                if (p.TryGetModuleData<PartComponentModule_Deployable, Data_Deployable>(out var dep)) dep.toggleExtend.SetValue(true);
            }
            Mark("fixture_inventory", new { panels=panels.Count, transmitter=tx!=null, parts=vessel.SimulationObject.PartOwner.Parts.Select(p=>new {p.Name,p.PartName}).ToArray() });
            if (tx == null || panels.Count != 2) throw new Exception("Fixture missing transmitter/panels");
            foreach (var panel in panels) Deploy(panel).toggleExtend.SetValue(true);
            tx.OnTransmissionUpdated += (id, state, fraction) => Mark("transmission_update", new { id, state = state.ToString(), fraction, ec = Charge() });
            Mark("conditions", new { game = typeof(VesselComponent).Assembly.GetName().Version.ToString(), unity = Application.unityVersion,
                infinitePower = G.SessionManager.IsDifficultyOptionEnabled("InfinitePower"), vessel = vessel.GlobalId.ToString(),
                lat = vessel.Latitude, lon = vessel.Longitude,
                tx_interval = tx.DataInterval, packet = tx.DataPacketSize, tx_ec_rate = tx.EcPerSecond,
                parts = vessel.SimulationObject.PartOwner.Parts.Select(p => p.Name).ToArray() });
            if (solarOnly) { yield return SolarRun(); yield break; }
            if (flagsOnly) { yield return FlagRun(); yield break; }
            Fill(0.2);
            yield return Hold("solar_deploy_tracking", 8);
            yield return Shot("01-deployed");
            foreach (var p in panels) Deploy(p).toggleExtend.SetValue(false);
            yield return Hold("solar_retract", 6);
            // The synthetic payload exercises the stock transmitter, broker, callbacks and timer.
            // It is not a submitted research report and cannot award science.
            Fill(1);
            phase = "tx_complete";
            Mark("transmit_start", new { size = tx.DataPacketSize, accepted = tx.TransmitData("ose-measure-complete", tx.DataPacketSize) });
            yield return Hold("tx_complete", tx.DataInterval + 1);
            if (tx.ActiveTransmissionSize > 0) throw new Exception("Baseline transmission has not stopped");
            Fill(1);
            Mark("transmit_start", new { accepted = tx.TransmitData("ose-measure-power", tx.DataPacketSize * 100) });
            yield return Hold("tx_before_power_loss", 1);
            Fill(0);
            yield return Hold("tx_power_loss", 3);
            Fill(1);
            yield return Hold("tx_power_return_no_restart", 2);
            Mark("transmit_restart", new { accepted = tx.TransmitData("ose-measure-resume", tx.DataPacketSize * 100, tx.DataPacketSize * 2) });
            yield return Hold("tx_power_restart", 1);
            tx.StopTransmission();
            var ranges = transmitters.Select(d => d.CommunicationRange).ToArray();
            for (int i = 0; i < 3; i++)
            {
                yield return Hold("link_phase_offset", 0.37f + i * 0.41f);
                phase = "link_loss_" + i;
                foreach (var d in transmitters) d.CommunicationRange = 1;
                vessel.SimulationObject.Telemetry.OnTransmitterDeactivated();
                Mark("range_cut", 1);
                yield return Hold(phase, 4);
                phase = "link_return_" + i;
                for (int j = 0; j < ranges.Length; j++) transmitters[j].CommunicationRange = ranges[j];
                vessel.SimulationObject.Telemetry.OnTransmitterActivated();
                Mark("range_restore");
                yield return Hold(phase, 4);
            }
            Fill(1);
            Mark("transmit_start", new { accepted = tx.TransmitData("ose-measure-link", tx.DataPacketSize * 100) });
            yield return Hold("tx_before_link_loss", 1);
            foreach (var d in transmitters) d.CommunicationRange = 1;
            vessel.SimulationObject.Telemetry.OnTransmitterDeactivated();
            Mark("range_cut", 1);
            yield return Hold("tx_link_loss", 4);
            for (int j = 0; j < ranges.Length; j++) transmitters[j].CommunicationRange = ranges[j];
            vessel.SimulationObject.Telemetry.OnTransmitterActivated();
            Mark("range_restore");
            yield return Hold("tx_link_return", 4);
            foreach (var p in panels) Deploy(p).toggleExtend.SetValue(true);
            Fill(0.2);
            yield return Hold("solar_redeployed", 6);
            // Move the same real craft to opposite longitudes; no irradiance/output is overridden.
            foreach (double lon in new[] { 0.0, 180.0 })
            {
                G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject, new SurfaceTeleportState {
                    ReferenceBodyGuid = vessel.mainBody.Name, Latitude = 0, Longitude = lon, Altitude = 100000, VerticalSpeed = 0 });
                Mark("surface_teleport", new { longitude = lon, altitude = 100000 });
                yield return Hold("solar_longitude_" + lon, 3);
                yield return Shot("02-longitude-" + lon);
            }
            // Return near the pad for an actual EVA/plant/remove sequence.
            G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject, new SurfaceTeleportState {
                ReferenceBodyGuid = vessel.mainBody.Name, Latitude = -0.1, Longitude = -74.5, Altitude = 0.2, VerticalSpeed = 0 });
            yield return Hold("return_surface", 6);
            var pod = vessel.SimulationObject.PartOwner.Parts.First(p => p.Name.StartsWith("pod_"));
            G.SpaceSimulation.TryGetViewObject(pod.SimulationObject, out var podView);
            Mark("eva_start", EVAUtils.TryStartEVA(podView.GetComponent<PartBehavior>(), 0));
            yield return Hold("eva_wait", 6);
            var eva = G.ViewController.GetActiveSimVessel();
            if (eva == null || !eva.IsKerbalEVA) { Mark("flag_unmeasured", "EVA did not become active"); }
            else
            {
                G.SpaceSimulation.TeleportSimObjectToSurface(eva.SimulationObject, new SurfaceTeleportState {
                    ReferenceBodyGuid = eva.mainBody.Name, Latitude = -0.1, Longitude = -74.499, Altitude = 0.1, VerticalSpeed = 0 });
                yield return Hold("eva_settle", 6);
                var kb = UnityEngine.Object.FindObjectsOfType<KerbalBehavior>().FirstOrDefault(k => k.Model != null && k.Model.SimulationObject == eva.SimulationObject);
                if (kb == null) kb = UnityEngine.Object.FindObjectsOfType<KerbalBehavior>().FirstOrDefault();
                if (kb == null) Mark("flag_unmeasured", "No KerbalBehavior");
                else
                {
                    Mark("flag_plant_request", kb.TryPlantFlag(new FlagComponentDefinition {Name="Services validation", LocationName="Telemetry", LocationDescription="Disposable test"}));
                    yield return Hold("flag_plant_animation", 10);
                    var flag = UnityEngine.Object.FindObjectsOfType<FlagBehavior>().FirstOrDefault();
                    if (flag == null) Mark("flag_unmeasured", "Plant did not create a flag");
                    else
                    {
                        var delta = (flag.Model.SimulationObject.Position - eva.SimulationObject.Position).vector;
                        Mark("flag_placed", new { distance_m = delta.magnitude, x=delta.x,y=delta.y,z=delta.z, carrying=kb.Model.IsCarryingFlag });
                        yield return Shot("03-flag-planted");
                        // Invoke the same interaction callback; this does not test pointer reach/selection.
                        typeof(KerbalBehavior).GetMethod("OnFlagRemovalUsed", BindingFlags.Instance | BindingFlags.NonPublic)
                            .Invoke(kb, new object[] {flag.GetComponentInChildren<InteractBehavior>(), null, null});
                        yield return Hold("flag_removed", 2);
                        Mark("flag_recovered", new { remaining = UnityEngine.Object.FindObjectsOfType<FlagBehavior>().Length, carrying = kb.Model.IsCarryingFlag });
                        yield return Shot("04-flag-recovered");
                    }
                }
            }
            Mark("complete");
            samples.Dispose(); samples = null; events.Dispose(); events = null;
            File.WriteAllText(Path.Combine(dir, "out", "done.txt"), "ok");
            Application.Quit();
        }

        IEnumerator FlagRun()
        {
            Mark("flag_conditions", new {setup="normal terrestrial OAB launch, no teleport",lat=vessel.Latitude,lon=vessel.Longitude});
            yield return Shot("flag-pad");
            var pod = vessel.SimulationObject.PartOwner.Parts.First(p => p.Name.StartsWith("pod_"));
            G.SpaceSimulation.TryGetViewObject(pod.SimulationObject, out var podView);
            Mark("eva_start", EVAUtils.TryStartEVA(podView.GetComponent<PartBehavior>(), 0));
            yield return Hold("eva_wait", 2);
            // The normal input handler releases the ladder on jump. Repeat over several frames
            // because the handler consumes the flag during its update.
            var jump = typeof(EVAInputHandler).GetField("_jump", BindingFlags.Instance | BindingFlags.NonPublic);
            double until = G.UniverseModel.UniverseTime + 0.4;
            while (G.UniverseModel.UniverseTime < until) { jump.SetValue(G.ViewController.evaInputHandler, true); yield return null; }
            jump.SetValue(G.ViewController.evaInputHandler, false);
            Mark("eva_jump_release");
            yield return Hold("eva_ground_settle", 8);
            var kb = UnityEngine.Object.FindObjectsOfType<KerbalBehavior>().FirstOrDefault();
            if (kb == null) throw new Exception("No EVA KerbalBehavior");
            bool planted = kb.TryPlantFlag(new FlagComponentDefinition {Name="Services validation",LocationName="Launchpad",LocationDescription="Disposable flag audit"});
            Mark("flag_plant_request", planted);
            if (!planted) throw new Exception("Ground flag request rejected");
            yield return Hold("flag_plant_animation", 10);
            var flag = UnityEngine.Object.FindObjectsOfType<FlagBehavior>().FirstOrDefault();
            if (flag == null) throw new Exception("No flag object after planting");
            var delta = (flag.Model.SimulationObject.Position - kb.Model.SimulationObject.Position).vector;
            Mark("flag_placed", new {distance_m=delta.magnitude,x=delta.x,y=delta.y,z=delta.z,carrying=kb.Model.IsCarryingFlag});
            Mark("flag_second_plant_request", kb.TryPlantFlag(new FlagComponentDefinition {Name="Second flag"}));
            yield return Shot("flag-planted");
            typeof(KerbalBehavior).GetMethod("OnFlagRemovalUsed", BindingFlags.Instance | BindingFlags.NonPublic)
                .Invoke(kb, new object[] {flag.GetComponentInChildren<InteractBehavior>(), null, null});
            yield return Hold("flag_removed", 2);
            Mark("flag_recovered", new {remaining=UnityEngine.Object.FindObjectsOfType<FlagBehavior>().Length,carrying=kb.Model.IsCarryingFlag});
            yield return Shot("flag-recovered");
            Mark("complete");
            samples.Dispose(); samples=null;events.Dispose();events=null;
            File.WriteAllText(Path.Combine(dir,"out","done.txt"),"ok");
            Application.Quit();
        }

        IEnumerator SolarRun()
        {
            yield return Hold("solar_initial_deploy", 8);
            for (int i=0;i<4;i++)
            {
                var body=vessel.mainBody;
                G.SpaceSimulation.TeleportSimObjectToOrbit(vessel.SimulationObject,new KeplerOrbitState {
                    referenceBodyGuid=body.Name,semiMajorAxis=body.radius+100000,eccentricity=0,inclination=0,
                    longitudeOfAscendingNode=0,argumentOfPeriapsis=0,meanAnomalyAtEpoch=i*Math.PI/2,epoch=G.UniverseModel.UniverseTime},true);
                yield return null;
                while(G.UI.GetCurtainVisibility()) yield return null;
                Fill(0.2);
                yield return Hold("solar_orbit_phase_"+i,4);
                var star=panels[0].trackingTransform;
                var r=body.transform.celestialFrame.ToLocalPosition(vessel.SimulationObject.Position);
                var sun=star==null?Vector3d.zero:body.transform.celestialFrame.ToLocalPosition(star.Position);
                Mark("solar_environment",new {case_index=i,altitude_m=vessel.AltitudeFromSeaLevel,lat=vessel.Latitude,lon=vessel.Longitude,
                    radius_m=body.radius,r=new[]{r.x,r.y,r.z},star=new[]{sun.x,sun.y,sun.z},ec=Charge()});
                yield return Shot("solar-orbit-"+i);
            }
            Mark("complete");samples.Dispose();samples=null;events.Dispose();events=null;
            File.WriteAllText(Path.Combine(dir,"out","done.txt"),"ok");Application.Quit();
        }
    }
}
