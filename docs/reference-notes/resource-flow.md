# Reference notes: resource flow

From reading `KSP.Sim.ResourceSystem` (`FlowGraph`, `Edge`, `ResourceFlowRequestManager`,
`ResourceFlowPriorityQuerySolver`, `ResourceContainerGroup`, `ResourceContainerGroupSequence`) and
`KSP.Sim.impl.PartFlowGraph`.

## Resources

From `resources-json`: a basic resource has `massPerUnit` (t), `volumePerUnit`, `flowMode` (code 1..5, see
`FlowMode` in `ose-data`), `transferMode` (1 = pumpable), `costPerUnit`, `NonStageable`. A recipe (`isRecipe`)
such as Methalox lists ingredients with `unitsPerRecipeUnit`; an engine burning Methalox requests Methane and
Oxidizer in that ratio. Containers on parts (`resourceContainers` in the part JSON) store basic resources.

## Flow graph

Nodes are parts (and, in the editor, assembly parts). Directed edges are created per attachment, in both
directions, with an `Edge` kind decided by the part that owns the node:

| Part kind | Edge | Flags |
|---|---|---|
| Decoupler (stageType `DecouplerHorizontal` or `DecouplerVertical`) | `NewDecoupler(part.fuelCrossFeed)` | `IsStructureBoundary = true` |
| Compound part, not a fuel line (strut) | `NewStrut()` | not searchable, no crossfeed, compound |
| Fuel line | `NewFuelLine(crossfeed, isCompound)` | directional (only outbound edge added), compound on the far end |
| Anything else | `NewAttachment(part.fuelCrossFeed)` | crossfeed from the part flag |

Node tags named `ReinforcedConnectionJoint` are skipped (they are structural only). The older
`PartFlowGraph` (part-level fast lookup) builds the same thing with the `AllowCrossfeed` flag of the attach node
(`isResourceCrossfeed` in the JSON) deciding stack edges and both parts' `fuelCrossFeed` deciding surface edges.

## Priorities (`ResourceFlowPriorityQuerySolver`)

For a requesting node (the engine's part) the solver flood-fills the graph from that node:

1. **Root distance**: a flood fill over searchable, non-compound edges where crossing a structure boundary
   (decoupler) increments the fill value. The result groups parts by "number of decouplers between here and
   the requester": `rootMaxDistancePriority` orders far groups first, `rootMinDistancePriority` the reverse.
2. **Stack distance** per node: a flood fill over searchable, non-compound edges that require crossfeed, where
   leaving a compound node (end of a fuel line) increments the value. Groups are "fuel-line hops away".
3. Per flow mode the container priority list is:

| Flow mode | Outbound (drain) order | Inbound (fill) order |
|---|---|---|
| `NO_FLOW` | the part itself only | same |
| `ALL_VESSEL` | every container on the vessel, one group | same |
| `STAGE_PRIORITY_FLOW` | groups by decoupler distance, nearest first (`rootMin`) | farthest first (`rootMax`) |
| `STACK_PRIORITY_SEARCH` | groups by crossfeed/fuel-line distance | same |
| `STAGE_STACK_FLOW_BALANCE` | same groups as stack priority search | same |

Methane and Oxidizer use `STACK_PRIORITY_SEARCH` (code 4) so tanks drain through the stack from the far end of a
fuel line chain first, bounded by crossfeed-disabled nodes and decouplers that block crossfeed.

## Requests (`ResourceFlowRequestManager.UpdateFlowRequests`)

A consumer (engine, generator, converter) owns a request with one or more instructions:
`FlowResource`, `FlowDirection` (inbound or outbound), `FlowMode`, `FlowUnitsOptimal`, `FlowUnitsMinimum`,
`FlowUnitsTarget` (a total budget, or negative for unlimited), `FlowUpdateMode` (per second or absolute).
Requests carry a `flowPriorityOffset`; each tick active requests are sorted by it descending and processed in
that order, so higher-offset consumers get first pick.

Per tick, for each request in order:

1. For each instruction, build the container group sequence from the solver's priority groups.
2. First pass ("pre-process"): `optimal = FlowUnitsOptimal * dt`, `minimum = FlowUnitsMinimum * dt`.
   - Outbound: available = stored units (including already pre-processed). If `optimal > available`, the
     request cannot be fully met; if `minimum > available` it fails (`fraction = 0`), else
     `fraction = min(fraction, available / optimal)`. The amount `optimal * fraction` is reserved.
   - Inbound: symmetric against empty capacity.
   The `fraction` is shared across all instructions of the request, so a Methalox engine starved of Oxidizer also
   throttles its Methane draw.
3. Second pass: if the request is at least partly met, each instruction moves
   `optimal * fraction` units (capped by the remaining `FlowUnitsTarget`): outbound `RemoveResourceUnits`,
   inbound `AddResourceUnits`.
4. The request's resolution state records accepted or rejected, the normalized delivery (`fraction`) and the
   list of failed resources; the engine reads it next update to scale thrust or flame out.

Draining across a sequence of priority groups (`ResourceContainerGroupSequence.ConsumePreProcessedResourceUnits`)
takes everything from the first group before touching the next. Filling (`AddResourceUnits`) goes in the
reverse order. Within one group the amount is split **proportionally to each container's current contents**
(`ResourceContainerGroup.ConsumePreProcessedResourceUnits`: `share = total / groupStored`, each container gives
`share * itsStored`), so tanks in the same group empty at the same fill ratio.

"InfiniteFuel" and "InfinitePower" difficulty options short-circuit the outbound checks.

## What we implement

- `ose-sim::resources`: resource definitions, containers as ECS components, a flow graph rebuilt on vessel
  structure change (attach/detach/decouple), the two flood fills, and the request processor above as a pure
  function over `(graph, containers, requests, dt)`.
- Keep the same group semantics (sequence, proportional split, shared fraction per request, priority offset
  ordering) so burn times and staging fuel distribution match the original.
- Parity tests: two stacked tanks plus an engine drain proportionally; a decoupler with crossfeed off isolates
  the upper stage; a fuel line pulls from the far tank first; an engine starved of one ingredient scales thrust
  by the delivered fraction.
