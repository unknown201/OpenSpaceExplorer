using System;
using KSP.Sim.impl;

namespace OpenSpaceExplorer.Validation
{
    // Boarding a Kerbal on EVA back into a crewed part, through the game's own boarding call. TO FILL from the
    // decompiled code (reference/ksp2/Assembly-CSharp: search "Board" in EVAUtils, KerbalBehavior and the
    // interact/airlock classes) on a machine with the KSP2 install.
    public static class EvaBoard
    {
        public static bool TryBoard(VesselComponent kerbal, PartBehavior pod, Action<string> mark)
        {
            mark("TryBoard not implemented");
            return false;
        }
    }
}
