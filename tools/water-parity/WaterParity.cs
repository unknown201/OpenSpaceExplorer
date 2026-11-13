using System;
using System.Collections;
using System.IO;
using System.Linq;
using System.Reflection;
using BepInEx;
using KSP.Game;
using KSP.IO;
using KSP.OAB;
using KSP.Sim;
using KSP.Sim.impl;
using KSP.Sim.State;
using Newtonsoft.Json;
using UnityEngine;

namespace OSE.Validation.Water
{
    // Active live-game benchmark. Never loads a player campaign or substitutes water forces.
    [BepInPlugin("org.openspaceexplorer.waterparity", "OSE Water Parity", "0.1.0")]
    [DefaultExecutionOrder(32000)]
    public sealed class WaterParity : BaseUnityPlugin
    {
        const string Campaign = "OSE Water Validation";
        string dir, scenario;
        VesselComponent vessel;
        PartComponent part;
        PartBehavior behavior;
        Rigidbody rb;
        StreamWriter samples, events;
        double start, last = -1;
        bool recording;
        Vector3 up, east;
        static GameInstance G => GameManager.Instance?.Game;
        static GameState State => G?.GlobalGameState?.GetState() ?? GameState.Invalid;
        static float[] A(Vector3 v) => new[] {v.x,v.y,v.z};
        static object Field(object o, string name) => o.GetType().GetField(name, BindingFlags.NonPublic|BindingFlags.Instance)?.GetValue(o);
        void Awake()
        {
            dir = Path.Combine(Paths.GameRootPath,"BepInEx","ose-water-validation");
            if (!File.Exists(Path.Combine(dir,"run.txt"))) return;
            Directory.CreateDirectory(Path.Combine(dir,"out"));
            samples = new StreamWriter(Path.Combine(dir,"out","samples.jsonl")) {AutoFlush=true};
            events = new StreamWriter(Path.Combine(dir,"out","events.jsonl")) {AutoFlush=true};
            Application.runInBackground=true;
            StartCoroutine(Guard(Run()));
        }
        void Mark(string action, object detail=null)
        {
            events.WriteLine(JsonConvert.SerializeObject(new {action,scenario,ut=G?.UniverseModel?.UniverseTime,real_s=Time.realtimeSinceStartup,detail}, Formatting.None));
            Logger.LogInfo(action+" "+scenario);
        }
        IEnumerator Guard(IEnumerator r)
        {
            while(true) {
                object c;
                try {if(!r.MoveNext()) break; c=r.Current;}
                catch(Exception e) {Mark("FAILED",e.ToString()); File.WriteAllText(Path.Combine(dir,"out","failed.txt"),e.ToString()); recording=false; Application.Quit(); yield break;}
                if(c is IEnumerator child) yield return Guard(child); else yield return c;
                if(File.Exists(Path.Combine(dir,"out","failed.txt"))) yield break;
            }
        }
        IEnumerator Wait(GameState target)
        {
            float until=Time.realtimeSinceStartup+600;
            while(State!=target && Time.realtimeSinceStartup<until) yield return null;
            if(State!=target) throw new Exception("State timeout: "+State+" expected "+target);
        }
        IEnumerator Hold(double seconds)
        {
            double until=G.UniverseModel.UniverseTime+seconds;
            float timeout=Time.realtimeSinceStartup+(float)Math.Max(120,seconds*12);
            while(G.UniverseModel.UniverseTime<until && Time.realtimeSinceStartup<timeout) {
                if(recording && part!=null && part.IsDestroyedOrBeingDestroyed) yield break;
                yield return null;
            }
            if(G.UniverseModel.UniverseTime<until) throw new Exception("UT stalled");
        }
        object Sample()
        {
            bool alive=part!=null && !part.IsDestroyedOrBeingDestroyed && rb!=null;
            if(!alive) return new {scenario,t=G.UniverseModel.UniverseTime-start,alive=false};
            var flow=G.UniverseView.PhysicsSpace.VectorToPhysics(vessel.SurfaceVelocity);
            var force=(PartBehavior.BuoyancyForce)Field(behavior,"buoyantForce");
            return new {scenario,t=G.UniverseModel.UniverseTime-start,ut=G.UniverseModel.UniverseTime,alive=true,
                origin_alt_m=vessel.mainBody.GetAltitudeFromRadius(G.UniverseView.PhysicsSpace.PhysicsToPosition(rb.position)),
                com_alt_m=vessel.mainBody.GetAltitudeFromRadius(G.UniverseView.PhysicsSpace.PhysicsToPosition(rb.worldCenterOfMass)),
                vertical_mps=Vector3.Dot(flow,up),horizontal_mps=(flow-up*Vector3.Dot(flow,up)).magnitude,
                bank_deg=Vector3.Angle(rb.rotation*Vector3.up,up),omega=A(rb.angularVelocity),
                splashed=part.Splashed,submerged=Field(behavior,"_submergedPercent"),drag=rb.drag,angular_drag=rb.angularDrag,
                drag_scalar=Field(behavior,"_dragScalar"),
                buoyant_force_local=new[]{force.RelativeForce.x,force.RelativeForce.y,force.RelativeForce.z},
                force_point_local=new[]{force.RelativePosition.x,force.RelativePosition.y,force.RelativePosition.z}};
        }
        void FixedUpdate()
        {
            if(!recording || G.UniverseModel.UniverseTime<=last) return;
            last=G.UniverseModel.UniverseTime;
            samples.WriteLine(JsonConvert.SerializeObject(Sample(),Formatting.None));
        }
        void Update()
        {
            if (samples!=null && (Screen.width<640 || Screen.height<360)) Screen.SetResolution(960,540,false);
        }
        IEnumerator Run()
        {
            yield return Wait(GameState.MainMenu);
            Mark("main_menu");
            Screen.SetResolution(960,540,false);
            var cases=new[] {
                new {name="drop",height=3.0,vy=-5.0,vx=0.0,bank=0.0,duration=45.0},
                new {name="bob",height=0.0,vy=0.0,vx=0.0,bank=0.0,duration=45.0},
                new {name="bank",height=0.0,vy=0.0,vx=0.0,bank=25.0,duration=45.0},
                new {name="drag",height=0.0,vy=0.0,vx=5.0,bank=0.0,duration=20.0},
                new {name="vertical_safe",height=1.0,vy=-16.0,vx=0.0,bank=0.0,duration=6.0},
                new {name="vertical_destroy",height=1.0,vy=-22.0,vx=0.0,bank=0.0,duration=3.0},
                new {name="horizontal_safe",height=0.0,vy=0.0,vx=100.0,bank=0.0,duration=3.0},
                new {name="horizontal_destroy",height=0.0,vy=0.0,vx=220.0,bank=0.0,duration=3.0}
            };
            foreach(var c in cases) {
                scenario=c.name; recording=false; vessel=null; part=null; rb=null;
                string seed=Path.Combine(G.SaveLoadManager.GetCampaignFolderPath(CampaignType.SinglePlayer,Campaign),"seed.json");
                G.SaveLoadManager.LoadGameFromFile(seed,(_,ok)=>{if(!ok) Logger.LogError("seed load failed");});
                yield return Wait(GameState.KerbalSpaceCenter);
                Mark("seed_loaded");
                yield return new WaitForSecondsRealtime(2);
                var snapshot=IOProvider.FromJsonFile<OABHistoricalSnapshot>(Path.Combine(dir,"craft.json"));
                bool shown=false;
                G.OAB.ShowOAB(new OABConfig(OABVariant.VAB,OABEnvironmentType.Orbital,OABConstructionType.User,null),snapshot,null,()=>shown=true);
                float timeout=Time.realtimeSinceStartup+180;
                while(!shown && Time.realtimeSinceStartup<timeout) yield return null;
                if(!shown) throw new Exception("OAB timeout");
                Mark("oab_shown");
                yield return new WaitForSecondsRealtime(5);
                Mark("launch_requested");
                G.OAB.Current.eventsUI.OnLaunch?.Invoke();
                yield return Wait(GameState.FlightView);
                Mark("flight_view");
                yield return Hold(2);
                vessel=G.ViewController.GetActiveSimVessel();
                vessel.SetAutopilotEnableDisable(false);
                G.SpaceSimulation.TeleportSimObjectToSurface(vessel.SimulationObject,new SurfaceTeleportState {
                    ReferenceBodyGuid=vessel.mainBody.Name,Latitude=0,Longitude=-60,Altitude=1000,VerticalSpeed=0});
                yield return Hold(8);
                part=vessel.SimulationObject.PartOwner.Parts.Single();
                if(!G.SpaceSimulation.TryGetViewObject(part.SimulationObject,out var view)) throw new Exception("No part view");
                behavior=view.GetComponent<PartBehavior>(); rb=view.GetComponent<Rigidbody>();
                up=rb.rotation*Vector3.up; east=rb.rotation*Vector3.right;
                double alt=vessel.mainBody.GetAltitudeFromRadius(G.UniverseView.PhysicsSpace.PhysicsToPosition(rb.position));
                if(alt < 100 || alt > 1200) throw new Exception("Teleport did not reach open water staging height: "+alt);
                var waterVelocity=rb.velocity-G.UniverseView.PhysicsSpace.VectorToPhysics(vessel.SurfaceVelocity);
                rb.rotation=Quaternion.AngleAxis((float)c.bank,rb.rotation*Vector3.forward)*rb.rotation;
                rb.position+=up*(float)(c.height-alt);
                rb.velocity=waterVelocity+up*(float)c.vy+east*(float)c.vx;
                rb.angularVelocity=Vector3.zero; rb.WakeUp();
                behavior.SyncToSimulationObject(true);
                // Refresh native telemetry after changing only the initial rigidbody state. Otherwise
                // its previous high-altitude descent speed can contaminate the first water-entry tick.
                vessel.SimulationObject.Telemetry.OnUpdate(G.UniverseModel.UniverseTime,0);
                scenario=c.name; start=G.UniverseModel.UniverseTime; last=-1;
                Mark("release",new {input=c,part=part.Name,mass_t=rb.mass,com=A(rb.centerOfMass),
                    inertia=A(rb.inertiaTensor),inertia_rotation=new[]{rb.inertiaTensorRotation.x,rb.inertiaTensorRotation.y,rb.inertiaTensorRotation.z,rb.inertiaTensorRotation.w},
                    displacement=Field(behavior,"_partVolume"),buoyancy=part.PartData.buoyancy,
                    co_buoyancy=new[]{part.PartData.coBuoyancy.x,part.PartData.coBuoyancy.y,part.PartData.coBuoyancy.z},crash_tolerance=part.PartData.crashTolerance,
                    fixed_dt_s=Time.fixedDeltaTime,game_version=Application.version,unity_version=Application.unityVersion,
                    radius_m=vessel.mainBody.radius,ocean_density=vessel.mainBody.oceanDensity,
                    infinite_power=G.SessionManager.IsDifficultyOptionEnabled("InfinitePower"),sample=Sample()});
                recording=true;
                samples.WriteLine(JsonConvert.SerializeObject(Sample(),Formatting.None));last=start;
                yield return Hold(c.duration);
                recording=false;
                Mark("end",Sample());
                yield return new WaitForEndOfFrame();
                ScreenCapture.CaptureScreenshot(Path.Combine(dir,"out",c.name+".png"));
                yield return new WaitForSecondsRealtime(0.3f);
            }
            Mark("complete"); samples.Dispose();events.Dispose();
            File.WriteAllText(Path.Combine(dir,"out","done.txt"),"ok");
            Application.Quit();
        }
    }
}
