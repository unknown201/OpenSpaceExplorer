using System;
using System.Collections;
using System.Globalization;
using System.IO;
using BepInEx;
using KSP.Game;
using KSP.Modules;
using KSP.Sim;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // F8 is passive. F9 explicitly runs a temporary orbit/input fixture; never writes a save.
    [BepInPlugin("org.openspaceexplorer.controlcapture", "OSE Control Capture", "0.2.0")]
    [DefaultExecutionOrder(-32000)]
    public sealed class ControlCapture : BaseUnityPlugin
    {
        StreamWriter capture;
        string vesselId;
        double start, last;
        bool pulse; double pulseStart; string pulseVessel;
        LiveState pending; double pendingUt;
        sealed class LiveState {
            public QuaternionD q;
            public Vector3d w, position, velocity, torque;
            public double physicsMass, wheelInput, pendingPower, acceptedPower;
        }
        void Awake() {
            StartCoroutine(AfterPhysics());
            Logger.LogInfo("Live rigid-body interval recorder ready. F8 starts/stops; F9 runs the orbit pulse.");
        }
        void Update()
        {
            if (Input.GetKeyDown(KeyCode.F9)) {
                var game = GameManager.Instance.Game;
                var vessel = game.ViewController.GetActiveSimVessel();
                if (vessel == null || !vessel.mainBody.bodyName.Equals("Kerbin")) return;
                Stop(); vessel.SetAutopilotEnableDisable(false);
                game.ViewController.flightInputHandler.OverrideInputThrottleCutoff(true);
                game.SpaceSimulation.TeleportSimObjectToOrbit(vessel.GlobalId.ToString(), new KSP.Sim.State.KeplerOrbitState {
                    referenceBodyGuid = "Kerbin", semiMajorAxis = 700000, eccentricity = 0,
                    inclination = 0, epoch = game.UniverseModel.UniverseTime
                }, false);
                pulse = true; pulseStart = game.UniverseModel.UniverseTime + 2;
                pulseVessel = vessel.GlobalId.ToString();
                Logger.LogInfo("Orbit/pulse fixture armed: two seconds settling, then pitch/release/reverse/roll/yaw for 12 seconds.");
            }
            bool trigger = Input.GetKeyDown(KeyCode.F8);
            if (!trigger) return;
            if (capture != null) { Stop(); return; }
            BeginCapture();
        }
        void BeginCapture()
        {
            try
            {
                var game=GameManager.Instance.Game;
                var vessel=game.ViewController.GetActiveSimVessel();
                if (vessel==null) {Logger.LogWarning("Select a flight vessel first.");return;}
                var folder=Path.Combine(Paths.GameRootPath,"BepInEx","ose-control-traces");
                Directory.CreateDirectory(folder);
                var path=Path.Combine(folder,DateTime.UtcNow.ToString("yyyyMMdd-HHmmss",CultureInfo.InvariantCulture)+".csv");
                start=game.UniverseModel.UniverseTime;last=double.NegativeInfinity;vesselId=vessel.GlobalId.ToString();
                capture=new StreamWriter(path,false);capture.AutoFlush=true;
                capture.WriteLine("t,pitch,yaw,roll,throttle,qx,qy,qz,qw,wx,wy,wz,mass_t,px,py,pz,vx,vy,vz,ix,iy,iz,command_pitch,command_yaw,command_roll,translate_x,translate_y,translate_z,sas,ut,dt,unity_frame,fixed_count,end_qx,end_qy,end_qz,end_qw,end_wx,end_wy,end_wz,physics_mass_t,cached_wx,cached_wy,cached_wz,torque_x,torque_y,torque_z,wheel_input,pending_power,accepted_power");
                File.WriteAllText(path+".metadata.txt","source=running_KSP2\nrecorder_version=0.2.0\nfixture="+(pulse?"orbit_input_pulse":"passive")+"\nphase=pre_physics_live_rigidbody\ninputs=interval_applied\nend_phase=wait_for_fixed_update_live_rigidbody\nframe=main_body_motion_frame\nhandedness=Unity\nvessel="+vesselId+"\nassembly="+typeof(VesselComponent).Assembly.GetName().Version+"\n");
                using(var parts=new StreamWriter(path+".parts.csv",false)) {
                    parts.WriteLine("part_id,guid,px,py,pz,qx,qy,qz,qw,com_x,com_y,com_z,dry_t,resource_t,crew_t,physics_t,ix,iy,iz,iqx,iqy,iqz,iqw");
                    var craftFrame=vessel.SimulationObject.transform.bodyFrame;
                    foreach(var part in vessel.SimulationObject.PartOwner.Parts) {
                        var rb=part.SimulationObject.Rigidbody;if(rb==null)continue;
                        var pos=craftFrame.ToLocalPosition(part.transform.Position);
                        var rot=craftFrame.ToLocalRotation(part.transform.Rotation);
                        var com=part.transform.bodyFrame.ToLocalPosition(part.CenterOfMass);
                        var iq=part.transform.bodyFrame.ToLocalRotation(rb.inertiaTensorRotation);
                        var inertia=rb.inertiaTensor.vector;
                        double[] values={pos.x,pos.y,pos.z,rot.x,rot.y,rot.z,rot.w,com.x,com.y,com.z,
                            part.DryMass,part.ResourceMass,part.GreenMass,part.PhysicsMass,
                            inertia.x,inertia.y,inertia.z,iq.x,iq.y,iq.z,iq.w};
                        parts.WriteLine(part.Name+","+part.Guid.ToString()+","+string.Join(",",Array.ConvertAll(values,v=>v.ToString("R",CultureInfo.InvariantCulture))));
                    }
                }
                using(var parts=new StreamWriter(path+".surfaces.csv",false)) {
                    parts.WriteLine("part_id,moving_coeff,fixed_coeff,range_deg,speed,pressure_x,pressure_y,pressure_z,pivot_x,pivot_y,pivot_z");
                    foreach(var part in vessel.SimulationObject.PartOwner.Parts) {
                        if(!game.SpaceSimulation.TryGetViewObject(part.SimulationObject,out var view))continue;
                        foreach(var module in view.GetComponents<Module_ControlSurface>()) {
                            if(!module.DataModules.Contains(typeof(Data_ControlSurface)))continue;
                            var data=(Data_ControlSurface)module.DataModules[typeof(Data_ControlSurface)];
                            double[] values={data.CtrlSurfaceArea,data.LiftingCtrlSurfaceArea,data.CtrlSurfaceRange,data.ActuatorSpeedNormalScale,
                                data.CoPressureCtrlSurfaceOffset.x,data.CoPressureCtrlSurfaceOffset.y,data.CoPressureCtrlSurfaceOffset.z,
                                data.CtrlSurfacePivotOffset.x,data.CtrlSurfacePivotOffset.y,data.CtrlSurfacePivotOffset.z};
                            parts.WriteLine(part.Name+","+string.Join(",",Array.ConvertAll(values,v=>v.ToString("R",CultureInfo.InvariantCulture))));
                        }
                    }
                }
                Logger.LogInfo("Recording "+path);
            }
            catch(Exception error) {Logger.LogError(error);Stop();}
        }
        void FixedUpdate()
        {
            if(capture==null && !pulse)return;
            try
            {
                var game=GameManager.Instance.Game;var vessel=game.ViewController.GetActiveSimVessel();
                if (pulse) {
                    double t=game.UniverseModel.UniverseTime-pulseStart;
                    if(vessel==null || vessel.GlobalId.ToString()!=pulseVessel || t>=12){Stop();return;}
                    // The stock override resets after every input FixedUpdate, so supply it
                    // before every physics tick, including multiple ticks in one render frame.
                    FlightInputHandler.OverrideInputPitch(t>=1 && t<2?1f:t>=3 && t<4?-1f:0f);
                    FlightInputHandler.OverrideInputRoll(t>=5 && t<6?1f:0f);
                    FlightInputHandler.OverrideInputYaw(t>=8 && t<9?1f:0f);
                    if(t>=0 && capture==null)BeginCapture();
                }
                if(capture==null)return;
                if(vessel==null || vessel.GlobalId.ToString()!=vesselId){Stop();return;}
                double ut=game.UniverseModel.UniverseTime;
                if(ut<=last)return;last=ut;
                if(pending!=null)throw new InvalidOperationException("Missing end-of-physics sample");
                pending=ReadLive(vessel);pendingUt=ut;
            }
            catch(Exception error){Logger.LogError(error);Stop();}
        }
        LiveState ReadLive(VesselComponent vessel)
        {
            var game=GameManager.Instance.Game;
            var physics=game.UniverseView.PhysicsSpace;
            var frame=game.UniverseModel.inertialReferenceFrame.bodyMotionFrame;
            var state=new LiveState();
            Vector3d pos=Vector3d.zero,vel=Vector3d.zero,w=Vector3d.zero,torque=Vector3d.zero;
            foreach(var part in vessel.SimulationObject.PartOwner.Parts) {
                if(!game.SpaceSimulation.TryGetViewObject(part.SimulationObject,out var view))continue;
                var body=view.Rigidbody?.activeRigidBody;
                if(body==null || body.isKinematic)continue;
                double mass=body.mass;state.physicsMass+=mass;
                pos+=(Vector3d)body.worldCenterOfMass*mass;
                vel+=(Vector3d)body.velocity*mass;w+=(Vector3d)body.angularVelocity*mass;
                foreach(var t in part.SimulationObject.Rigidbody.Torques)
                    torque+=view.transform.localToWorldMatrix.MultiplyVector(t.RelativeTorque);
                foreach(var module in view.GetComponents<Module_ReactionWheel>()) {
                    if(!module.DataModules.Contains(typeof(Data_ReactionWheel)))continue;
                    var data=(Data_ReactionWheel)module.DataModules[typeof(Data_ReactionWheel)];
                    state.wheelInput+=data.InputSumForResource;
                    state.pendingPower+=data.HasPendingResourceActivity?1:0;
                    state.acceptedPower+=data.HasResourcesToOperate?1:0;
                }
            }
            if(state.physicsMass<=0)throw new InvalidOperationException("No live dynamic rigid bodies");
            var position=physics.PhysicsToPosition(pos/state.physicsMass);
            state.position=frame.localFrame.ToLocalPosition(position);
            state.velocity=Vector.Reframed(frame.ToRelativeVelocity(physics.PhysicsToVelocity(vel/state.physicsMass),position),frame.localFrame).vector;
            state.w=Vector.Reframed(frame.ToRelativeAngularVelocity(physics.PhysicsToAngularVelocity(w/state.physicsMass)),frame.localFrame).vector;
            state.torque=Vector.Reframed(physics.PhysicsToVector(torque),frame.localFrame).vector;
            var owner=vessel.GetControlOwner()??vessel.SimulationObject.PartOwner.RootPart;
            if(!game.SpaceSimulation.TryGetViewObject(owner.SimulationObject,out var controlView) || controlView.Rigidbody?.activeRigidBody==null)
                throw new InvalidOperationException("No live control-owner rigid body");
            var local=owner.transform.bodyFrame.ToLocalRotation(vessel.ControlTransform.Rotation);
            state.q=frame.localFrame.ToLocalRotation(physics.PhysicsToRotation(controlView.Rigidbody.activeRigidBody.rotation))*local;
            return state;
        }
        IEnumerator AfterPhysics()
        {
            while(true) {
                yield return new WaitForFixedUpdate();
                if(capture==null || pending==null)continue;
                try {
                var game=GameManager.Instance.Game;var vessel=game.ViewController.GetActiveSimVessel();
                if(vessel==null || vessel.GlobalId.ToString()!=vesselId){Stop();continue;}
                var before=pending;pending=null;
                var end=ReadLive(vessel);
                var f=vessel.flightCtrlState;
                var frame=game.UniverseModel.inertialReferenceFrame.bodyMotionFrame;
                var cached=Vector.Reframed(frame.ToRelativeAngularVelocity(vessel.AngularVelocityMassAvg),frame.localFrame).vector;
                var moi = vessel.SimulationObject.PartOwner.MOI.vector;
                double scale=game.ViewController.flightInputHandler!=null && game.ViewController.flightInputHandler.IsPrecisionMode?0.1:1.0;
                double dt=game.UniverseModel.UniverseTime-pendingUt;
                if(dt<=0)continue;
                double[] values={pendingUt-start,f.inputPitch*scale+f.pitchTrim,f.inputYaw*scale+f.yawTrim,f.inputRoll*scale+f.rollTrim,f.mainThrottle,
                    before.q.x,before.q.y,before.q.z,before.q.w,before.w.x,before.w.y,before.w.z,vessel.totalMass,
                    before.position.x,before.position.y,before.position.z,before.velocity.x,before.velocity.y,before.velocity.z,moi.x,moi.y,moi.z,
                    f.pitch,f.yaw,f.roll,f.X,f.Y,f.Z,vessel.Autopilot!=null && vessel.Autopilot.Enabled?1:0,pendingUt,
                    dt,Time.frameCount,game.FixedUpdateCount,end.q.x,end.q.y,end.q.z,end.q.w,end.w.x,end.w.y,end.w.z,
                    before.physicsMass,cached.x,cached.y,cached.z,end.torque.x,end.torque.y,end.torque.z,
                    end.wheelInput,end.pendingPower,end.acceptedPower};
                capture.WriteLine(string.Join(",",Array.ConvertAll(values,v=>v.ToString("R",CultureInfo.InvariantCulture))));
                } catch(Exception error){Logger.LogError(error);Stop();}
            }
        }
        void Stop(){pending=null;if(pulse){pulse=false;FlightInputHandler.OverrideInputPitch(0);FlightInputHandler.OverrideInputYaw(0);FlightInputHandler.OverrideInputRoll(0);}if(capture==null)return;capture.Dispose();capture=null;Logger.LogInfo("Control capture stopped.");}
        void OnDestroy(){Stop();}
    }
}
