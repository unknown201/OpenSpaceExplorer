using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Reflection;
using KSP.Game;
using KSP.IO;
using KSP.Messages;
using KSP.Modules;
using KSP.OAB;
using UnityEngine;

namespace OpenSpaceExplorer.Validation
{
    // Procedural tubes and engine mounts (docs/ksp2-side-by-side-gaps.md, part rendering): KSP2 gives them a
    // floating attach node at the end of a procedural wall. Built here in the VAB the way a player places parts,
    // with KSP2's own calls (ObjectAssemblyBuilder.CreateBasicShip's: CreatePartFreeFloating, SnapNodes,
    // StackParts, then the PartPlacedMessage the placement tool publishes), onto the Mk1 pod of craft-drop-pod.json:
    //
    //   tube_1v_procedural (bottom) on the pod's top, then a 2.5 m tank on that tube's floating node
    //   pod, 1x2 tank, tube_1v_procedural hung by its floating node, 1x2 tank, enginemount_1v_procedural,
    //   Terrier on the mount's bottom node, inline decoupler on the mount's floating node
    //
    // After each placement every Module_Fairing's Data_Fairing (Length, FairingStartHeight, CrossSections), its
    // floating node and its panels' extent in the part frame go to procparts.csv; every part's pose to
    // procparts-parts.csv; the workspace (KSP2's own save format) to procparts-workspace.json.
    public static class ProcPartsScenario
    {
        static GameInstance G => GameManager.Instance?.Game;
        static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        static readonly FieldInfo DataField = typeof(Module_Fairing).GetField("_dataFairing", BindingFlags.NonPublic | BindingFlags.Instance);

        public static IEnumerator Run(string dir, Action<string> mark, Func<string, IEnumerator> shot)
        {
            var oab = G.OAB.Current;
            var tracker = oab.ActivePartTracker;
            var avail = new Dictionary<string, IObjectAssemblyAvailablePart>();
            foreach (var a in oab.Stats.AvailableParts) avail[a.Name] = a;
            IObjectAssemblyPart pod = null;
            foreach (var p in tracker.PartsMasterList) if (p.AvailablePart != null && p.AvailablePart.Name == "pod_1v_conical_crew") pod = p;
            if (pod == null) { mark("procparts: no pod"); yield break; }
            var csv = new StreamWriter(Path.Combine(dir, "out", "procparts.csv"), false);
            csv.WriteLine("step,part,length,start_height,capped,deploy,floating_node_x,floating_node_y,floating_node_z,floating_connected,panels,panel_min_y,panel_max_y,radius_at_min_y,radius_at_max_y,sections");
            var made = new List<IObjectAssemblyPart> { pod };
            IObjectAssemblyPart created = null;

            IEnumerator Create(string name)
            {
                created = null;
                if (!avail.TryGetValue(name, out var a)) { mark("procparts: no available part " + name); yield break; }
                tracker.CreatePartFreeFloating(a, pod.WorldPosition + Vector3.right * 6f, ref created);
                yield return new WaitForSeconds(1.5f);
                if (created != null) made.Add(created);
                mark("procparts: created " + name + (created == null ? " (failed)" : ""));
            }

            IEnumerator Attach(string step, IObjectAssemblyPart parent, string parentTag, IObjectAssemblyPart child, string childTag)
            {
                if (parent == null || child == null) { mark("procparts: " + step + " skipped (missing part)"); yield break; }
                var pn = parent.FindNodeWithTag(parentTag);
                var cn = child.FindNodeWithTag(childTag);
                if (pn == null || cn == null) { mark("procparts: " + step + " no node " + (pn == null ? parentTag : childTag)); yield break; }
                ObjectAssemblyPlacementTool.SnapNodes(pn, cn);
                bool ok = tracker.StackParts(pn, cn, out _, out _);
                if (G.Messages.TryCreateMessage<PartPlacedMessage>(out var msg))
                {
                    msg.messagePart = child;
                    msg.attachmentReceiver = parent;
                    msg.receiverPartNode = pn;
                    msg.messagePartNode = cn;
                    G.Messages.Publish(msg);
                }
                yield return new WaitForSeconds(2f);
                mark("procparts: " + step + (ok ? "" : " (StackParts false)"));
                foreach (var p in made) Dump(csv, step, p);
                csv.Flush();
            }

            // a tube standing on the pod with nothing on it, then a 2.5 m tank on its floating node
            yield return Create("tube_1v_procedural");
            var tubeTop = created;
            yield return Attach("tube-on-pod", pod, "top", tubeTop, "bottom");
            yield return Create("fueltank_2v_inline_methalox_1x2");
            yield return Attach("wide-tank-on-tube", tubeTop, "dynamic_floating", created, "bottom");
            // down the stack: tank, tube hung by its floating node, tank, engine mount, Terrier, decoupler
            yield return Create("fueltank_1v_inline_methalox_1x2");
            var t1 = created;
            yield return Attach("tank-under-pod", pod, "bottom", t1, "top");
            yield return Create("tube_1v_procedural");
            var tube = created;
            yield return Attach("tube-under-tank", t1, "bottom", tube, "dynamic_floating");
            yield return Create("fueltank_1v_inline_methalox_1x2");
            var t2 = created;
            yield return Attach("tank-under-tube", tube, "bottom", t2, "top");
            yield return Create("enginemount_1v_procedural");
            var mount = created;
            yield return Attach("mount-under-tank", t2, "bottom", mount, "top");
            yield return Create("engine_1v_methalox_terrier");
            yield return Attach("engine-under-mount", mount, "bottom", created, "top");
            yield return Create("decoupler_1v_inline");
            yield return Attach("decoupler-on-mount-floating", mount, "dynamic_floating", created, "top");
            csv.Close();

            // run 1 (0b5c556) stopped here: a row threw and the coroutine died, so each row is guarded
            try
            {
                using (var w = new StreamWriter(Path.Combine(dir, "out", "procparts-parts.csv"), false))
                {
                    w.WriteLine("part,x,y,z,qx,qy,qz,qw,parent,parent_node,own_node");
                    foreach (var p in made)
                    {
                        try
                        {
                            var rel = pod.PartTransform.InverseTransformPoint(p.PartTransform.position);
                            var q = Quaternion.Inverse(pod.PartTransform.rotation) * p.PartTransform.rotation;
                            w.WriteLine(string.Join(",", new[] { p.AvailablePart?.Name, F(rel.x), F(rel.y), F(rel.z), F(q.x), F(q.y), F(q.z), F(q.w),
                                p.ParentPart?.AvailablePart?.Name ?? "", p.ParentNodeConnectedToMe?.NodeTag ?? "", p.MyNodeConnectedToParent?.NodeTag ?? "" }));
                        }
                        catch (Exception e) { mark("procparts: pose of " + (p?.AvailablePart?.Name ?? "?") + " failed: " + e.GetType().Name + " " + e.Message); }
                    }
                }
            }
            catch (Exception e) { mark("procparts: parts dump failed: " + e.GetType().Name + " " + e.Message); }
            try
            {
                IOProvider.ToJsonFile(Path.Combine(dir, "out", "procparts-workspace.json"), oab.UtilFileIO.GenerateSnapshot(tracker));
                mark("procparts: workspace written");
            }
            catch (Exception e) { mark("procparts: workspace not written: " + e.Message); }
            yield return shot("05-procparts-vab");
        }

        static string F(double x) => x.ToString("R", Inv);

        static void Dump(StreamWriter w, string step, IObjectAssemblyPart part)
        {
            if (!part.TryGetModule<Module_Fairing>(out var m) || m == null) return;
            var d = DataField?.GetValue(m) as Data_Fairing;
            if (d == null) return;
            var secs = new List<string>();
            if (d.CrossSections != null)
                foreach (var c in d.CrossSections) secs.Add(F(c.Height) + ":" + F(c.Radius) + ":" + F(c.DefaultHeight));
            var node = part.FindNodeWithTag(d.FloatingAttachNodeTag);
            Vector3 np = node != null ? node.PartRelativePosition : new Vector3(float.NaN, float.NaN, float.NaN);
            float minY = float.PositiveInfinity, maxY = float.NegativeInfinity, rMin = 0, rMax = 0;
            int panels = 0;
            if (m.Panels != null)
                foreach (var panel in m.Panels)
                {
                    if (panel?.PanelGameObject == null || panel.Mesh == null) continue;
                    panels++;
                    foreach (var v in panel.Mesh.vertices)
                    {
                        var lp = part.PartTransform.InverseTransformPoint(panel.PanelGameObject.transform.TransformPoint(v));
                        float r = new Vector2(lp.x, lp.z).magnitude;
                        if (lp.y < minY - 1e-4f) { minY = lp.y; rMin = r; } else if (Mathf.Abs(lp.y - minY) <= 1e-4f) rMin = Mathf.Max(rMin, r);
                        if (lp.y > maxY + 1e-4f) { maxY = lp.y; rMax = r; } else if (Mathf.Abs(lp.y - maxY) <= 1e-4f) rMax = Mathf.Max(rMax, r);
                    }
                }
            w.WriteLine(string.Join(",", new[] { step, part.AvailablePart?.Name, F(d.Length.GetValue()), F(d.FairingStartHeight), d.IsCapped ? "1" : "0",
                d.DeployType.GetValue().ToString(), F(np.x), F(np.y), F(np.z), node?.ConnectedPart?.AvailablePart?.Name ?? "",
                panels.ToString(Inv), F(minY), F(maxY), F(rMin), F(rMax), string.Join(" ", secs) }));
        }
    }
}
