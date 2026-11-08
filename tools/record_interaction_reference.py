"""Record installed KSP2 settings and execute extracted interaction equations.

This is a source/configuration reference, NOT a live input/video capture. Private
game source remains in .cache; only numeric results and hashes are committed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

from check_control_parity import method

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / 'crates/ose-client/testdata/ksp2-interactions.json'
SOURCE = ROOT / 'reference/ksp2/Assembly-CSharp'
BUILD = ROOT / '.cache/interaction-reference'


def record(global_dir):
    hashes = {}

    def source(name):
        path = SOURCE / name
        raw = path.read_bytes()
        hashes[name] = hashlib.sha256(raw).hexdigest()
        return raw.decode('utf-8-sig')

    def config(name):
        path = global_dir / name
        raw = path.read_bytes()
        hashes['Global/' + name] = hashlib.sha256(raw).hexdigest()
        return json.loads(raw)

    def expression(text, pattern):
        match = re.search(pattern, text)
        if not match:
            raise ValueError('Reference source changed: ' + pattern)
        return match.group(1)

    flight = source('KSP.Sim.impl/FlightInputHandler.cs')
    placement = source('KSP.OAB/ObjectAssemblyPlacementTool.cs')
    widget = source('KSP.OAB/ManipulationWidget.cs')
    basic = source('KSP.Rendering.impl/BasicCameraRigSolution.cs')
    map_input = source('KSP.Map/MapCameraInputHandler.cs')
    source('KSP.Sim/CameraInputState.cs')
    source('Mouse.cs')
    source('KSP.OAB/ObjectAssemblyInputHandler.cs')
    source('KSP.OAB/ObjectAssemblyCameraManager.cs')
    source('KSP.OAB/SymmetrySetPositionerRadial.cs')
    source('KSP.Map/Map3DManeuvers.cs')
    source('KSP.Sim.impl/ManeuverPlanComponent.cs')
    gizmo = source('KSP.Map/MapManeuverGizmo.cs')
    warp = source('KSP.Sim.impl/TimeWarp.cs')
    symmetry = [int(n) for n in re.findall(r'\bX(\d+)\b', source('KSP.OAB/BuilderSymmetryMode.cs'))]
    settings = {name: config(name + 'CameraTweakables.json') for name in ['Ground', 'Orbital', 'Map']}
    oab = config('oab-config.json')['OABBehavioralValues']
    levels = config('PhysicsSettings.json')['TimeWarpLevels']
    # Compile extracted bodies/expressions; do not substitute an OSE implementation.
    throttle = expression(flight, r'float value = ([^;]+);\s*value = Mathf.Clamp01\(value\)')
    rotation = expression(placement, r'float num = (\(_cameraFastModifierTriggered \? [^;]+\));\s*RotateSelection')
    snap = expression(widget, r'float num4 = (\(input.IsSpeedModifierActive \? [^;]+\))')
    yaw = expression(basic, r'double num = (zero.x \* unscaledDeltaTime)')
    zoom = expression(basic, r'gimbalState.distance \*= ([^;]+)')
    map_gain = expression(map_input, r'num = (Mathf.Max\(_minSensitivity[^;]+)')
    vb_gain = expression(placement, r'float num = (\(_modifierTriggered \? \(vector.x[^;]+)')
    vb_degrees = expression(placement, r'float deltaYaw = ([^;]+)')
    vb_zoom = expression(placement, r'float scrollWheelDifference = (\(_modifierTriggered[^;]+)')
    BUILD.mkdir(parents=True, exist_ok=True)
    bodies = '\n'.join([
        'static double GetAccelerationTime(double initialWarpFactor,double targetWarpFactor,double lerpSpeed) ' + method(warp, 'private static double GetAccelerationTime('),
        'static double GetWarpRateAcceleration(double startSpeed,double targetSpeed,double lerpTime) ' + method(warp, 'private static double GetWarpRateAcceleration('),
        'static double ScaleInputsForDistance(float x,float numInches) ' + method(gizmo, 'private double ScaleInputsForDistance('),
        'static double GetMaxWarpRateForTravel(double warpDeltaTime,double minTimeInWarp,double maxTimeInWarp,out int rateIdx) ' + method(warp, 'private double GetMaxWarpRateForTravel('),
    ])
    program = r'''
using System;
using System.IO;
using System.Collections.Generic;
using System.Text.Json;
static class Screen { public static float dpi = 96; }
static class Mathf { public static float Clamp01(float x)=>Math.Clamp(x,0,1); public static float Abs(float x)=>Math.Abs(x); public static float Max(float a,float b)=>Math.Max(a,b); }
static class GlobalLog {public static void Error(object a,object b) {throw new Exception(b.ToString());}}
enum LogFilter {Debug}
static class PhysicsSettings {public struct Level {public float TimeScaleFactor;} public static Level[] TimeWarpLevels;}
class Program {
 static int _maxWarpRateIdx => PhysicsSettings.TimeWarpLevels.Length - 2;
 BODIES
 static void Main(string[] args) {
 var cases = new List<object>();
 void Add(string id,string category,string op,object input,object expected,string unit) => cases.Add(new{id,category,op,input,expected,unit});
 foreach (float mainThrottle in new[]{0f,0.4f,1f}) foreach(float _inputThrottle in new[]{-1f,0f,1f}) foreach(float unscaledDeltaTime in new[]{0.02f,0.1f}) {
  float value = THROTTLE;
  Add($"throttle-{cases.Count}","throttle","throttle",new{current=mainThrottle,axis=_inputThrottle,dt=unscaledDeltaTime},Mathf.Clamp01(value),"fraction");
 }
 Add("Z","throttle","preset",new{full=true},MAX_THROTTLE,"fraction");
 Add("X","throttle","preset",new{full=false},CUT_THROTTLE,"fraction");
 foreach(bool snap in new[]{false,true}) foreach(bool fine in new[]{false,true}) {
  bool _cameraFastModifierTriggered=fine;
  Add($"rotation-{snap}-{fine}","vab_rotation","rotation",new{snap,fine},ROTATION,"degree");
 }
 var settings=JsonDocument.Parse(File.ReadAllText(args[0])).RootElement;
 foreach(string context in new[]{"Ground","Orbital"}) foreach(bool fine in new[]{false,true}) foreach(float dt in new[]{1f/30,1f/60,1f/144}) {
  var s=settings.GetProperty("cameras").GetProperty(context);
  double gain=s.GetProperty("orbitSensitivity").GetDouble()*(fine?s.GetProperty("fineMovementModifier").GetDouble():1);
  var zero=new{x=100*gain};float unscaledDeltaTime=dt;
  Add($"flight-orbit-{context}-{fine}-{dt}","flight_camera","orbit",new{context="flight",pixels=100,fine,dt},YAW,"degree");
 }
 foreach(string context in new[]{"Ground","Orbital"}) foreach(bool fine in new[]{false,true}) foreach(double axis in new[]{-0.1,0.0,0.1}) {
  var s=settings.GetProperty("cameras").GetProperty(context);
  double num2=axis*s.GetProperty("scrollSensitivity").GetDouble()*(fine?s.GetProperty("fineMovementModifier").GetDouble():1);
  Add($"flight-zoom-{context}-{fine}-{axis}","flight_camera","zoom",new{context="flight",axis,fine,shift=false,distance=100.0},100*(ZOOM),"metre");
 }
 var map=settings.GetProperty("cameras").GetProperty("Map");
 foreach(bool fine in new[]{false,true}) foreach(float pitch in new[]{0f,40f,80f,85f}) {
  float num=(float)(map.GetProperty("orbitSensitivity").GetDouble()*(fine?map.GetProperty("fineMovementModifier").GetDouble():1));
  float _minSensitivity=0.001f,_pitchLimitInDegrees=85f,_cumulativePitch=pitch,_inversePitchLimit=1f/85;
  num=MAP_GAIN;
  Add($"map-orbit-{fine}-{pitch}","map_camera","orbit",new{context="map",pixels=100,fine,pitch},100*map.GetProperty("panSensitivity").GetDouble()*num,"degree");
 }
 foreach(double axis in new[]{-1.0,1.0}) {
  double f=map.GetProperty("ExponentialZoomFactor").GetDouble();
  Add($"map-zoom-{axis}","map_camera","zoom",new{context="map",axis,fine=false,shift=false,distance=1.0},Math.Exp(-Math.Sign(axis)*f),"ratio");
 }
 var oab=settings.GetProperty("oab");
 var stats=new{BehavioralValues=new{OrbitalCameraSpeedRotatePrecise=(float)oab.GetProperty("OrbitalCameraSpeedRotatePrecise").GetDouble(),OrbitalCameraSpeedRotate=(float)oab.GetProperty("OrbitalCameraSpeedRotate").GetDouble(),CameraSpeedZoomPrecise=(float)oab.GetProperty("CameraSpeedZoomPrecise").GetDouble(),CameraSpeedZoom=(float)oab.GetProperty("CameraSpeedZoom").GetDouble()}};
 foreach(bool _modifierTriggered in new[]{false,true}) {
  var vector=new{x=100f};float num5=VAB_GAIN;
  Add($"vab-orbit-{_modifierTriggered}","vab_camera","orbit",new{context="vab",pixels=100,fine=_modifierTriggered},VAB_DEGREES,"degree");
  foreach(bool shift in new[]{false,true}) foreach(float axis in new[]{-0.1f,0f,0.1f}) {
   float currentZoomDelta=axis*(shift?(float)oab.GetProperty("CameraSpeedTurboScalar").GetDouble():1);
   float scrollWheelDifference=VAB_ZOOM;
   Add($"vab-zoom-{_modifierTriggered}-{shift}-{axis}","vab_camera","zoom",new{context="vab",axis,fine=_modifierTriggered,shift,distance=100.0},100-scrollWheelDifference,"metre");
  }
 }
 foreach(float dpi in new[]{96f,192f}) foreach(float pixels in new[]{0f,24f,48f,96f}) foreach(int frames in new[]{1,30,60}) {
  Screen.dpi=dpi;
  Add($"maneuver-{dpi}-{pixels}-{frames}","maneuver","maneuver",new{pixels,dpi,frames,strength=1.0,fine=false},frames*(float)ScaleInputsForDistance(pixels,1),"m/s");
 }
 var ladder=settings.GetProperty("ladder");
 PhysicsSettings.TimeWarpLevels=new PhysicsSettings.Level[ladder.GetArrayLength()];
 for(int i=0;i<ladder.GetArrayLength();i++)PhysicsSettings.TimeWarpLevels[i].TimeScaleFactor=ladder[i].GetSingle();
 var regular=new List<float>();for(int i=0;i<=_maxWarpRateIdx;i++)regular.Add(PhysicsSettings.TimeWarpLevels[i].TimeScaleFactor);
 Add("warp-ladder","warp","ladder",new{},regular,"factor");
 foreach(double remaining in new[]{0.0,1,5,10,30,500,10000}) {
  double rate=GetMaxWarpRateForTravel(remaining,1,10,out int idx);
  Add($"warp-to-{remaining}","warp","warp_to",new{remaining},rate,"factor");
 }
 foreach(double rate in new[]{1.0,2,4,10,50,100,1000,1000000}) {
  double lead=GetAccelerationTime(rate,1,GetWarpRateAcceleration(rate,1,1));
  foreach(double offset in new[]{-0.001,0,0.001}) {
   double remaining=lead+offset;
   Add($"auto-drop-{rate}-{offset}","auto_drop","drop",new{rate,remaining},rate<=1 || remaining<=lead,"boolean");
  }
 }
 File.WriteAllText(args[1],JsonSerializer.Serialize(new{cases}));
 }
}'''
    replacements = {
        'BODIES': bodies, 'THROTTLE': throttle, 'ROTATION': rotation,
        'MAX_THROTTLE': expression(method(flight, 'private void OnThrottleMax()'), r'_flightCtrlState.mainThrottle = ([^;]+)'),
        'CUT_THROTTLE': expression(method(flight, 'private void OnThrottleCutoff()'), r'_flightCtrlState.mainThrottle = ([^;]+)'),
        'YAW': yaw, 'ZOOM': zoom, 'MAP_GAIN': map_gain,
        'VAB_GAIN': vb_gain, 'VAB_DEGREES': vb_degrees, 'VAB_ZOOM': vb_zoom,
    }
    program = re.sub(r'\b(' + '|'.join(replacements) + r')\b', lambda m: replacements[m.group()], program)
    (BUILD / 'Program.cs').write_text(program, encoding='utf-8')
    (BUILD / 'reference.csproj').write_text('<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net9.0</TargetFramework></PropertyGroup></Project>')
    inputs = {'cameras': settings, 'oab': oab, 'ladder': [v['TimeScaleFactor'] for v in levels]}
    (BUILD / 'inputs.json').write_text(json.dumps(inputs))
    env = dict(os.environ, DOTNET_CLI_HOME=str(BUILD), DOTNET_SKIP_FIRST_TIME_EXPERIENCE='1', DOTNET_CLI_TELEMETRY_OPTOUT='1')
    subprocess.run(['dotnet', 'run', '--project', str(BUILD / 'reference.csproj'), '-c', 'Release', '--', str(BUILD / 'inputs.json'), str(BUILD / 'results.json')], env=env, check=True)
    result = json.loads((BUILD / 'results.json').read_text())
    result['cases'].extend([
        {'id': 'symmetry-counts', 'category': 'symmetry', 'op': 'symmetry_counts', 'input': {}, 'expected': symmetry, 'unit': 'count'},
        {'id': 'symmetry-angles', 'category': 'symmetry', 'op': 'symmetry_angles', 'input': {}, 'expected': [360 / n for n in symmetry], 'unit': 'degree'},
    ])
    physics_max_index = int(expression(warp, r'MAX_PHYSICS_WARP_RATE_INDEX = (\d+)'))
    physics = source('PhysicsSettings.cs')
    event_max_index = int(expression(physics, r'MAX_TIME_WARP_RATE_INDEX_SOI_TRANSITIONS = (\d+)'))
    fine_snap, normal_snap = [float(v) for v in re.findall(r'(\d+(?:\.\d+)?)f', snap)]
    result.update(schema=1, provenance={'kind': 'compiled_reference_equations_and_installed_configuration', 'game_version': '0.2.2.0.32913', 'hashes': hashes, 'live_input_capture': False}, settings=inputs,
                  symmetry={'counts': symmetry, 'angles_deg': [360 / n for n in symmetry]},
                  rotation_gizmo={'snap_deg': [normal_snap, fine_snap], 'snap_off': 'continuous', 'source_expression_sha256': hashlib.sha256(snap.encode()).hexdigest()},
                  warp_policy={'max_physics_rate': inputs['ladder'][physics_max_index], 'interstellar_rate': inputs['ladder'][-1], 'orbital_event_cap_rate': inputs['ladder'][event_max_index], 'soi_lookahead_sim_seconds': int(expression(physics, r'SOI_TRANSITION_LOOK_AHEAD_TIME_SECONDS = (\d+)')), 'fuel_exhaustion_lookahead_sim_seconds': int(expression(physics, r'OUT_OF_FUEL_LOOK_AHEAD_TIME_SECONDS = (\d+)')), 'nominal_rate_transition_unity_seconds': 1, 'maneuver_margin_sim_seconds': float(expression(physics, r'WARP_TO_MANNODE_MARGIN = ([\d.]+)'))},
                  boundaries=['Camera cases use Unity input units; browser CSS pixels and wheel events need live calibration.', 'Maneuver cases use an assumed serialized handle strength of 1 and explicit DPI, projected handle displacement, and number of Update frames; not a constant m/s per pixel.', 'Warp-to models unrestricted default selection and deceleration trigger; collision, altitude, SOI and multiplayer caps are separate.', 'No live Shift/Ctrl hold, camera drag, or maneuver drag was measured in this recording.'])
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(f'Recorded {len(result["cases"])} source/configuration cases in {OUTPUT}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--global-dir', type=Path, default=Path(os.environ.get('USERPROFILE', '~')) / 'AppData/LocalLow/Intercept Games/Kerbal Space Program 2/Global')
    record(parser.parse_args().global_dir)
