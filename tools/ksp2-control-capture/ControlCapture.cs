using System;
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
    // Passive telemetry only: never injects inputs, alters vessel state, or writes a save.
    [BepInPlugin("org.openspaceexplorer.controlcapture", "OSE Control Capture", "0.1.0")]
    [DefaultExecutionOrder(32000)]
    public sealed class ControlCapture : BaseUnityPlugin
    {
        StreamWriter capture;
        string vesselId;
        double start, last;
        void Awake() { Logger.LogInfo("Passive control recorder ready. F8 starts/stops a CSV capture of the active flight vessel."); }
        void Update()
        {
            if (!Input.GetKeyDown(KeyCode.F8)) return;
            if (capture != null) { Stop(); return; }
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
                capture.WriteLine("t,pitch,yaw,roll,throttle,qx,qy,qz,qw,wx,wy,wz,mass_t,command_pitch,command_yaw,command_roll,translate_x,translate_y,translate_z,sas,ut");
                File.WriteAllText(path+".metadata.txt","source=running_KSP2\nphase=post_FixedUpdate\nframe=main_body_motion_frame\nhandedness=Unity\nvessel="+vesselId+"\nassembly="+typeof(VesselComponent).Assembly.GetName().Version+"\n");
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
            if(capture==null)return;
            try
            {
                var game=GameManager.Instance.Game;var vessel=game.ViewController.GetActiveSimVessel();
                if(vessel==null || vessel.GlobalId.ToString()!=vesselId){Stop();return;}
                double ut=game.UniverseModel.UniverseTime;
                if(ut<=last)return;last=ut;
                var f=vessel.flightCtrlState;
                var frame=game.UniverseModel.inertialReferenceFrame.bodyMotionFrame;
                var q=frame.localFrame.ToLocalRotation(vessel.ControlTransform.Rotation);
                var w=Vector.Reframed(frame.ToRelativeAngularVelocity(vessel.AngularVelocityMassAvg),frame.localFrame).vector;
                double scale=game.ViewController.flightInputHandler!=null && game.ViewController.flightInputHandler.IsPrecisionMode?0.1:1.0;
                double[] values={ut-start,f.inputPitch*scale+f.pitchTrim,f.inputYaw*scale+f.yawTrim,f.inputRoll*scale+f.rollTrim,f.mainThrottle,
                    q.x,q.y,q.z,q.w,w.x,w.y,w.z,vessel.totalMass,
                    f.pitch,f.yaw,f.roll,f.X,f.Y,f.Z,vessel.Autopilot!=null && vessel.Autopilot.Enabled?1:0,ut};
                capture.WriteLine(string.Join(",",Array.ConvertAll(values,v=>v.ToString("R",CultureInfo.InvariantCulture))));
            }
            catch(Exception error){Logger.LogError(error);Stop();}
        }
        void Stop(){if(capture==null)return;capture.Dispose();capture=null;Logger.LogInfo("Control capture stopped.");}
        void OnDestroy(){Stop();}
    }
}
