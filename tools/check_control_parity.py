"""Generate numerical fixtures by compiling the local reference PID source.

This is a source-level comparison, not a capture of the running KSP2 simulation.
Requires .NET 9 SDK and the locally extracted reference tree (never redistributed).
"""
from pathlib import Path
import hashlib
import json
import os
import subprocess

ROOT=Path(__file__).resolve().parents[1]
SOURCE=ROOT/'reference/ksp2/Assembly-CSharp/KSP.Sim'
BUILD=ROOT/'.cache/control-reference'
OUTPUT=ROOT/'crates/ose-sim/testdata/control-reference.json'

def method(text, signature):
    start=text.index(signature)
    brace=text.index('{',start)
    depth=1;end=brace+1
    while depth:
        depth+=(text[end]=='{')-(text[end]=='}');end+=1
    return text[brace:end]

def main():
    BUILD.mkdir(parents=True,exist_ok=True);OUTPUT.parent.mkdir(parents=True,exist_ok=True)
    hashes={}
    for name in ['PIDclamp.cs','IPid.cs']:
        src=(SOURCE/name).read_text(encoding='utf-8')
        (BUILD/name).write_text(src,encoding='utf-8')
        hashes[name]=hashlib.sha256(src.encode()).hexdigest()
    sas=(SOURCE/'VesselSAS.cs').read_text(encoding='utf-8')
    body=method(sas,'private double EvaluateScalar(double ratio)')
    hashes['EvaluateScalar']=hashlib.sha256(body.encode()).hexdigest()
    (BUILD/'Scalar.cs').write_text('public static class ReferenceScalar { public static double Evaluate(double ratio) '+body+' }',encoding='utf-8')
    (BUILD/'control-reference.csproj').write_text('<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net9.0</TargetFramework></PropertyGroup></Project>',encoding='utf-8')
    (BUILD/'Program.cs').write_text(r'''using System;
using System.IO;
using System.Collections.Generic;
using System.Text.Json;
using KSP.Sim;
public static class UtilMath {public static double Clamp(double v,double lo,double hi)=>Math.Clamp(v,lo,hi);}
public static class Program {
 public static void Main(string[] args) {
  var traces=new List<object>();
  foreach(var axis in new[]{0,1,2}) foreach(var dt in new[]{0.02,0.04,0.08}) {
   var pid=new PIDclamp("trace",axis==1?9000:15000,0,axis==1?600:2000,axis==1?2000:1500);
   var rows=new List<double[]>();
   for(int i=0;i<96;i++) {
    double error=i<20?0.025:i<40?0.0002*Math.Sin(i):i<64?-0.012:0;
    pid.TuningScalar=i<48?0.7:0.125;pid.ClampScalar=i<48?1:4;
    if(i==72)pid.Reset();
    rows.Add(new[]{error,pid.TuningScalar,pid.ClampScalar,pid.Update(error,dt)/pid.Clamp});
   }
   traces.Add(new{axis,dt,rows});
  }
  var scalars=new List<double[]>();
  foreach(var r in new[]{-1.0,0,0.0001,0.05,0.1,0.1001,1,10,10.0001,25,50,50.0001,100})scalars.Add(new[]{r,ReferenceScalar.Evaluate(r)});
  File.WriteAllText(args[0],JsonSerializer.Serialize(new{traces,scalars}));
 }
}''',encoding='utf-8')
    env=dict(os.environ,DOTNET_CLI_HOME=str(BUILD),DOTNET_SKIP_FIRST_TIME_EXPERIENCE='1',DOTNET_CLI_TELEMETRY_OPTOUT='1')
    subprocess.run(['dotnet','run','--project',str(BUILD/'control-reference.csproj'),'-c','Release','--',str(OUTPUT)],check=True,env=env)
    data=json.loads(OUTPUT.read_text());data['provenance']={'kind':'compiled_reference_source','hashes':hashes}
    OUTPUT.write_text(json.dumps(data,separators=(',',':')),encoding='utf-8')
    print('Recorded',sum(len(t['rows']) for t in data['traces']),'PID steps and',len(data['scalars']),'autotune breakpoints')

if __name__=='__main__':main()
