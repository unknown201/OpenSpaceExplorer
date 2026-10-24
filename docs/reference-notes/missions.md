# Reference notes: missions

From reading `KSP.Game.Missions` (`ConditionSet`, `PropertyCondition`, `EventCondition`, `MissionResolutionAction`)
and `KSP.Game.Science.ScienceManager.GetPotentialMissionSciencePointsScaled`, and the 57 mission JSONs
(`explorationmissions`, `missioncontrol`).

## Shape

A mission has ordered `missionStages`; each has a `scriptableCondition` tree and a `MissionReward` (only
`SciencePoints` rewards occur, 25 to 10000). The stage without a condition is the closing one. Conditions:

- `ConditionSet` with `ConditionMode` AND / OR / XOR / NOT over children.
- `PropertyCondition`: a property watcher (`PropertyTypeAQN`) compared with `propOperator` EQUAL / GREATER /
  LESSER against `TestWatchedValue` (numbers), `TestWatchedInt` (counts) or `TestWatchedstring` (text), or an
  `Inputstring` the watcher takes (a part id, a part family, a report key, a body) with `TestWatchedBool`.
  `RequireCurrentValue = false` latches once met.
- `EventCondition`: a message type (`EventTypeAQN`) observed since the stage became current.

Watchers in the stock missions (count): VesselSOI 38, EscapeTrajectorySOIPredicted 15, VesselResearchLocation 13,
periapsis 11, HasPart 11, apoapsis 8, HasPartInGroup 7, PartActivated 7, HasStoredResearchReport 6,
HasScoredResearchReport 6, and a few each of max / ground / sea-level altitude, mass, wheel and crew counts,
orbit speed, landed-on-body, last destroyed vessel, EVA and distance from a position. Events: landed on ground
or water at rest, landed after flight, flag planted, vessel destroyed, report acquired, and tutorial steps.

## What we implement

`ose_data::missions` evaluates the trees against per-vessel facts from `ose_sim::missions` (a flight log of
highest altitude, landings, crashes and activated parts per vessel; debris is left out). The server checks the
offered missions once a second against every campaign member's vessels; a stage passes when one vessel meets it,
and its reward counts towards the agency's science points (times `MissionRewards`). Missions are offered as:
every secondary agency mission with a granter at once; the primaries `Main_Kerbin_01`, `_02`, `_03` in order,
then the remaining primaries. 40 of the 57 use only checks we support; the rest (tutorial helpers, distance
from a mission position) are not offered.

Simplifications: events count from when their stage began rather than from the evaluation that consumes them;
latched checks persist until the stage passes; missions are accepted automatically and rewards granted on
completion rather than turned in at Mission Control; flags and EVA do not exist yet, so those missions cannot
finish.
