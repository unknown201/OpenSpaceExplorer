using System;
using System.Reflection;
using KSP;
using KSP.Game;
using KSP.Sim.impl;

namespace OpenSpaceExplorer.Validation
{
    // Boarding a Kerbal on EVA back into a crewed part, through the game's own boarding call. This is what
    // KerbalBehavior.OnAirlockUsed (the airlock interact callback, private) does: stop any ladder climb
    // (private EnsureLadderClimbCleanup, called by reflection), look the Kerbal up in the roster by its main
    // part's sim object at seat 0, then EVAUtils.TryStopEVA(kerbal vessel sim object id, kerbal id, pod part),
    // which moves the Kerbal into the pod's first free seat, makes the pod's vessel active and destroys the
    // Kerbal's sim object.
    public static class EvaBoard
    {
        public static bool TryBoard(VesselComponent kerbal, PartBehavior pod, Action<string> mark)
        {
            var game = GameManager.Instance.Game;
            if (kerbal == null || pod == null) { mark("board: no kerbal or pod"); return false; }
            if (!EVAUtils.CanBoardPart(pod)) { mark("board: pod full"); return false; }
            var behaviour = game.ViewController.GetBehaviorIfLoaded(kerbal.SimulationObject.Kerbal);
            if (behaviour != null)
            {
                var cleanup = typeof(KerbalBehavior).GetMethod("EnsureLadderClimbCleanup", BindingFlags.Instance | BindingFlags.NonPublic);
                if (cleanup != null) { cleanup.Invoke(behaviour, null); mark("board: EnsureLadderClimbCleanup"); }
                else mark("board: EnsureLadderClimbCleanup not found");
            }
            var mainPartId = behaviour != null && behaviour.MainPart != null
                ? behaviour.MainPart.Model.SimulationObject.GlobalId
                : kerbal.SimulationObject.PartOwner.RootPart.SimulationObject.GlobalId;
            if (!game.SessionManager.KerbalRosterManager.TryGetKerbalBySimObjectAndIdx(mainPartId, 0, out var info))
            {
                mark("board: kerbal not in roster under " + mainPartId);
                return false;
            }
            bool ok = EVAUtils.TryStopEVA(kerbal.SimulationObject.GlobalId, info.Id, pod);
            mark("board: EVAUtils.TryStopEVA " + info.Id + " -> " + pod.Model.PartData.partName + " = " + ok);
            return ok;
        }
    }
}
