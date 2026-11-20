using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Reflection;
using KSP.Game;
using KSP.Modules;
using KSP.OAB;
using KSP.Sim.impl;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // T38 fairing drag cubes (docs/ksp2-side-by-side-gaps.md): the Mk1 pod with an AE-FF125 fairing base
    // (craft-fairing.json, its wall written by make_craft_fairing.py) on the pad. KSP2 builds the wall when the
    // craft is loaded (Module_Fairing.WaitForMaterialsAndSpawn: SpawnMeshes, GenerateDragCubes); every drag cube
    // the part then holds is written to fairing-cubes.csv: per face area, drag and depth, centre and size (part
    // frame, Unity axes), and the cube's weight.
    public static class FairingScenario
    {
        static GameInstance G => GameManager.Instance?.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;

        public static IEnumerator Run(VesselComponent vessel, string dir, Action<string> mark)
        {
            Module_Fairing fairing = null;
            PartComponent part = null;
            foreach (var p in vessel.SimulationObject.PartOwner.Parts)
            {
                if (!p.Name.StartsWith("fairing_")) continue;
                if (G.SpaceSimulation.TryGetViewObject(p.SimulationObject, out var view)) { fairing = view.GetComponent<Module_Fairing>(); part = p; }
            }
            if (fairing == null) { mark("fairings: no fairing module"); yield break; }
            if (!part.TryGetModuleData<PartComponentModule_Fairing, Data_Fairing>(out var data)) { mark("fairings: no Data_Fairing"); yield break; }
            if (!part.TryGetModuleData<PartComponentModule_Drag, Data_Drag>(out var drag)) { mark("fairings: no Data_Drag"); yield break; }
            mark("fairings: start height " + data.FairingStartHeight.ToString("R", Inv) + ", side count " + data.FairingSideCount + ", thickness " + data.FairingThickness.ToString("R", Inv));
            using (var w = new StreamWriter(Path.Combine(dir, "out", "fairing-cubes.csv"), false))
            {
                w.WriteLine("wall,capped,sections,panels,cube,weight,a0,a1,a2,a3,a4,a5,d0,d1,d2,d3,d4,d5,p0,p1,p2,p3,p4,p5,cx,cy,cz,sx,sy,sz");
                // the wall as KSP2 built it when the craft was loaded: rebuilding it in flight leaves the old
                // panels in place and the next cube renders both, so each wall is its own run
                Dump(w, vessel.SimulationObject.Name, data, fairing, drag);
                mark("fairings: " + vessel.SimulationObject.Name + ", panels " + (fairing.Panels?.Count ?? 0));
                yield return new WaitForSeconds(1f);
            }
        }

        static void Dump(StreamWriter w, string name, Data_Fairing data, Module_Fairing fairing, Data_Drag drag)
        {
            string F(double x) => x.ToString("R", Inv);
            var secs = new List<string>();
            foreach (var c in data.CrossSections) secs.Add(F(c.Height) + ":" + F(c.Radius));
            foreach (var cube in drag.cubes)
            {
                var row = new List<string> { name, data.IsCapped ? "1" : "0", string.Join(" ", secs), (fairing.Panels?.Count ?? 0).ToString(Inv), cube.Name, F(cube.Weight) };
                for (int k = 0; k < 6; k++) row.Add(F(cube.Area[k]));
                for (int k = 0; k < 6; k++) row.Add(F(cube.Drag[k]));
                for (int k = 0; k < 6; k++) row.Add(F(cube.Depth[k]));
                row.Add(F(cube.Center.x)); row.Add(F(cube.Center.y)); row.Add(F(cube.Center.z));
                row.Add(F(cube.Size.x)); row.Add(F(cube.Size.y)); row.Add(F(cube.Size.z));
                w.WriteLine(string.Join(",", row));
            }
        }
    }
}
