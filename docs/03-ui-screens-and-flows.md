# KSP2 user interface: screens, states and flows (reference survey)

Surveyed 2026-10-23 from the ILSpy decompile in `reference/ksp2/Assembly-CSharp/` (read-only) and the extracted JSON in `assets/ksp2/data/`. All paths below are relative to `reference/ksp2/` unless marked `data/`. Note that the decompiler wrote the `KSP.Game` namespace folder as `Assembly-CSharp/KSP.game` (lower-case g); sub-namespaces such as `KSP.Game.Missions` kept their case.

Purpose: a planning document for OpenSpaceExplorer (OSE). Section 1 is the state machine, 2 the screen inventory, 3 the player journeys, 4 the build list mapped onto the phases of `02-architecture-and-plan.md`, 5 the side-by-side checklist. Where something is inferred rather than read from code it is marked *(inferred)*.

How the original UI is wired, in one paragraph: every screen is a Unity prefab bound to a `DataContext` of named properties and actions (`KSP.UI.Binding.*`, `AddProperty("Throttle", ...)`, `AddAction("TrackMission", ...)`). Game systems publish `KSP.Messages.*` events through one `MessageCenter`; UI subscribes. The visible set of canvases per state is a `UIView` list in `Assembly-CSharp/KSP.game/UIStateViews.cs`. The property and action names quoted below are therefore the exact data each screen consumes; they double as the OSE protocol/HUD field list.

---

## 1. Game states and transitions

### 1.1 The state enum

`Assembly-CSharp/KSP.game/GameState.cs`:

| Value | State | Shipped use |
|---|---|---|
| 0 | Invalid | |
| 1 | WarmUpLoading | boot, splash, legal |
| 2 | MainMenu | landing menu, campaign menus, settings |
| 3 | KerbalSpaceCenter | KSC overview, building picker |
| 10 | VehicleAssemblyBuilder | VAB (the OAB in VAB variant) |
| 11 | BaseAssemblyEditor | BAE, colony editor variant of the OAB (not reachable in 0.2.2; colonies unshipped) |
| 20 | FlightView | flight HUD |
| 21 | ColonyView | unshipped (`KSP.Game.Colonies` exists, no UI entry) |
| 22 | Map3DView | map view, in flight or as Tracking Station |
| 30 | PhotoMode | UI hidden |
| 31 | MetricsMode | debug |
| 32 | PlanetViewer | debug planet viewer (`Assembly-CSharp/KSP.PlanetViewer/PlanetViewer.cs`) |
| 33 | Loading | curtain while KSC assets load |
| 40 | TrainingCenter | tutorials menu |
| 41 | MissionControl | missions screen |
| 42 | TrackingStation | enum exists; the real tracking station runs as `Map3DView` with `MapMode.TrackingStation` |
| 43 | ResearchAndDevelopment | tech tree |
| 44, 45, 46 | Launchpad, Runway, Flag | enum only; no enter/exit case |

`GameStateConfiguration` (`KSP.game/GameStateConfiguration.cs`) tags each state with `IsObjectAssembly`, `IsFlightMode`, `InGameSession`; the list lives in a ScriptableObject (`GameStatesConfiguration.cs`), not in code.

### 1.2 The state machine

`Assembly-CSharp/KSP.game/GameStateMachine.cs` (extends `SimpleStateMachine<GameState>`). On every `SetState`:

1. publishes `GameStateLeftMessage`, `GameStateChangedMessage {PreviousState, CurrentState}`, `GameStateEnteredMessage` (`Assembly-CSharp/KSP.Messages/GameStateChangedMessage.cs` and siblings);
2. `OnStateChange` publishes `DismissAllNotificationsMessage`, hides the pause menu, records the state on the player's `CampaignPlayerEntry` (so a save remembers where each player was), runs `OnStateExit(prev)` then `OnStateEnter(next)`, and refreshes the global header contextual menu (`GlobalHeaderManager.UpdateContextualMenu`).

Enter/exit side effects (same file):

| State | On enter | On exit |
|---|---|---|
| MainMenu | primary camera none; `PropertyIsMultiplayer=false` | disable "Previous" navigation input |
| KerbalSpaceCenter | virtual cursor on; flight input locked, KSC input unlocked; camera `FocusFlightCamOnKSCGameLandingObject`; push `UIStateViews.KSCView` | lock KSC input |
| VehicleAssemblyBuilder | camera group OAB; push `OABView`; OAB input on; time warp stopped and disabled; gamepad navigation enabled | `OAB.HideOAB()`, release flight camera, stop warp, re-enable warp |
| FlightView | camera group Flight, camera Flight, cursor off, flight input on (forced), push `FlightView`, `ValidateNavballIsVisible`, publish `FlightViewEnteredMessage` | publish `FlightViewLeftMessage`, lock flight input |
| Map3DView | flight and map input on, cursor on, camera group Map, primary camera none, publish `MapViewEnteredMessage` | publish `MapViewLeftMessage`, lock map input, `Map.DisableMap()` |
| ResearchAndDevelopment | cursor on, RD input on, push `RDCenterView`, publish `ResearchAndDevelopmentLoadedMessage` | lock RD input |
| MetricsMode / PlanetViewer | push `PhotoModeView` / `PlanetViewerView` (both are the hidden-UI view) | |
| TrainingCenter, MissionControl, Loading, PhotoMode | nothing here; handled by the callers below | |

### 1.3 Who triggers which transition

Every `GlobalGameState.SetState(...)` call site in the decompile:

| From | To | Trigger | Where |
|---|---|---|---|
| boot | WarmUpLoading -> MainMenu | `GameManager.StartGame` after splash screens and legal acceptance; `CreateMainMenuFlowAction` loads `MainMenu.prefab` | `KSP.game/GameManager.cs`, `KSP.Game.StartupFlow/CreateMainMenuFlowAction.cs`, `LegalMenu.cs` |
| MainMenu | KerbalSpaceCenter | New campaign: `CreateCampaignMenu.StartCampaignCreationFlow` builds a `LoadOrSaveCampaignTicket` with `Load_StartNewCampaign`, runs the load flow (section 1.4), then `SetActiveVesselFlowAction` calls `ViewController.ObserveKSC()`; autosave `CampaignCreationAtMainMenu` | `KSP.game/CreateCampaignMenu.cs`, `KSP.Game.Load/SetActiveVesselFlowAction.cs`, `KSP.Sim.impl/ViewController.cs` |
| MainMenu | FlightView or KSC | Load save: `CampaignLoadMenu.LoadSelectedFile` -> `SaveLoadManager.LoadGameFromFile`; the save's active vessel decides (`SetActiveVesselFlowAction`) | `KSP.game/CampaignLoadMenu.cs`, `KSP.game/SaveLoadManager.cs` |
| KSC | Loading -> KSC | `ViewController.ObserveKSC`: curtain `ToKSC`, state `Loading`, load Kerbin scaled and local space, then `OnKSCDependenciesLoaded` sets `KerbalSpaceCenter` | `KSP.Sim.impl/ViewController.cs` lines 338-586 |
| KSC | VehicleAssemblyBuilder | building click `KSCNavEnum.VAB` -> `KSCMenuManager.UserOAB(VAB, Terrestrial)` -> `OAB.ShowOAB(config)` -> `ObjectAssemblyBuilder` sets state | `KSP.UI/KSCMenuManager.cs`, `KSP.OAB/ObjectAssemblyBuilder.cs` |
| KSC | Map3DView (TrackingStation mode) | `KSCNavEnum.TrackingCenter` -> `Map.TransitionToMapTrackingStationFlight` -> `MapProvider.TransitionToMapTrackingStation` | `KSP.game/MapProvider.cs` 81-104 |
| KSC | ResearchAndDevelopment | `KSCNavEnum.ResearchCenter` (only if game-mode feature `SciencePoints`) -> `UIManager.TransitionToResearchAndDevelopment`: curtain `ToResearchAndDevelopment`, then SetState | `KSP.game/UIManager.cs` 611 |
| KSC | MissionControl | `KSCNavEnum.AstronautComplex` (only if feature `ExplorationMissions`) -> `GameInstance.TransitionToMissionControl` -> `MissionControlManager.ShowMissionControl` | `KSP.game/GameInstance.cs` 1179, `Assembly-CSharp/MissionControlManager.cs` 139-189 |
| KSC | TrainingCenter | `KSCNavEnum.TrainingCenter` -> `GameInstance.TransitionToTrainingCenter`: publishes `TrainingCenterLoadingMessage`, curtain `ToTrainingCenter`, `ShowTrainingCenter`, `TrainingCenterLoadedMessage` | `KSP.game/GameInstance.cs` 1056 |
| KSC | (Launchpad dialog, no state change) | `KSCNavEnum.Launchpad_1..4`, `Runway_1..2`, `Boat_Launch`, `AirTraffic` -> `OAB.SetLaunchSite`, `LaunchpadDialog.SetVisible(true)` | `KSP.UI/KSCMenuManager.cs` 139-186 |
| Launchpad dialog | FlightView | `launchVehicle` action spawns the saved craft at the site | `KSP.game/LaunchpadDialog.cs` 452 |
| VAB | FlightView | Launch button: `BeginAssemblyLaunch` -> `DetermineLaunchLocation`, `TryDeductResourceCostAtLaunch`, publish `VABLaunchButtonPressedMessage`, `AsyncLoadAllParts`, `LaunchAtLocation` -> `OABProvider.SpawnAssembly` creates the vessel sim object, SetState(FlightView), `SetActiveVehicle` | `KSP.OAB/ObjectAssemblyBuilder.cs` 600-640, `KSP.game/OABProvider.cs` 712-745 |
| VAB | KSC | header "back"/escape menu KSC action; `ObjectAssemblyBuilder` sets `KerbalSpaceCenter` | `KSP.OAB/ObjectAssemblyBuilder.cs` |
| FlightView | Map3DView | key `ShowMap` (M) or app-bar map toggle -> `UIManager.LoadMap`: SetState, `Map.EnableFlightMap`, push `FlightMapView` | `KSP.game/UIManager.cs` 411, `KSP.UI.Flight/GameView.cs` |
| Map3DView | FlightView or KSC | `UIManager.TransitionAwayFromMapState`: back to FlightView if that was the previous state or a vessel is active, else KSC | `KSP.game/UIManager.cs` 431 |
| Map3DView | FlightView | double-click/"Control" on a map item or tracking-station entry (`Map3DFocusItem`, `MapUISelectableEntry.OnControl`) if `CanObserverLeaveTheActiveVessel` | `KSP.Map/Map3DFocusItem.cs` 202, `KSP.Map/MapUISelectableEntry.cs` 814-822 |
| Map3DView | VehicleAssemblyBuilder | `OnVAB` action on a tracking-station entry (edit a landed craft) | `KSP.Map/MapUI.cs` |
| Flight / any | KSC | escape menu `ksc`, vessel status menu with no active vessel (`BackToKSCMessage`), tracking station `OnGoToKSC`, breadcrumbs KSC crumb; all call `ViewController.ObserveKSC` | `KSP.game/GlobalEscapeMenu.cs` 521, `KSP.game/VesselDataProvider.cs` 897, `KSP.game/BreadcrumbsController.cs` 374 |
| any in-session | MainMenu | escape menu `quitToMainMenu` -> `QuitToMainMenuStartedMessage`, `GameInstance.Shutdown`, `GameManager` SetState(MainMenu) | `KSP.game/GlobalEscapeMenu.cs`, `KSP.game/GameManager.cs` |
| Flight | Flight (revert) | escape menu `revertToLaunchpad` / `revertToVab` using `stateRevTracker.HasValidReversionState`; publishes `RevertToLaunchMessage` / `VehicleRevertedToOABMessage`, `VehicleRevertCompleteMessage` | `KSP.game/GlobalEscapeMenu.cs`, `KSP.Messages/RevertToLaunchMessage.cs` |
| Flight | Flight (EVA) | `EVAUtils` publishes `EVAEnteredMessage` / `EVALeftMessage`; the kerbal becomes the active vessel, no GameState change | `Assembly-CSharp/KSP/EVAUtils.cs` 38, 94 |
| any | PlanetViewer / back | debug only | `KSP.PlanetViewer/PlanetViewer.cs` |

Pause is not a state: `GameStateMachine.Paused` plus `UIManager.SetPauseVisible`/`ToggleEscapeVisible`, `GamePauseToggledMessage`, `EscapeMenuOpenedMessage`/`EscapeMenuClosedMessage`, `PauseStateChangedMessage` (`KSP.Messages/`). Universe time pause is separate (`UniverseDataProvider.IsUniverseTimePaused`, `PressPlayAction`).

### 1.4 Load and save flows

`Assembly-CSharp/KSP.game/SaveLoadManager.cs` runs a `SequentialFlow` of `FlowAction`s (`KSP.Game.Flow/`, `KSP.Game.Load/`). Operations (`LoadOrSaveCampaignOperation`): `Load_StartNewCampaign`, `Load_StartExistingCampaignFromJsonString`, `Load_LoadGameFromAsset/Addressable/File/Buffer`, `Save_SaveGameToFile/Memory`, `Save_SaveSpecificPlayerToFile/Memory`, `DeleteSaveFile`, `DeleteDirectory`. Every operation is a `LoadOrSaveCampaignTicket` with a status (`Working/Success/Failure`) and a failure code (`Error_LoadFileDoesNotExist`, `Error_SaveFileIsReadOnly`, `Error_BusyLoading`, ...).

Load order (`PrivateLoadCommon`, lines 1287-1400): fade curtain -> game-mode data -> session manager -> local player -> session GUID, campaign players, agencies -> unload main menu -> validate file version -> mission base -> preload assets -> resource and procedural-part databases -> kerbal prefab and roster -> colony data -> properties -> create simulation -> celestial body files -> interstellar neighbourhood -> part data -> create celestial bodies -> universe view, view controller -> starting body -> map systems -> UI systems -> mission data -> science manager -> universe state -> create vessels -> set active vessel (this is where KSC vs flight is decided) -> travel log -> game dashboard -> camera state -> start sim -> flight control state -> legacy module data -> planted flags -> `LoadDoneFlowAction` -> `GameLoadFinishedMessage`.

Save order (`PrivateSaveCommon`, 1204-1262): agencies, campaign players, flight data (vessels + control state), session manager, version, session GUID, metadata (incl. thumbnail via `SaveGameThumbnailCreator`), kerbal roster, colonies, travel log, mission progress, planted flags, serialize to file or memory. Save kinds: Manual, Quick (F5, `ForceQuickSave`), Auto (`AutoSaveEvents`, e.g. `CampaignCreationAtMainMenu`). Saves live per campaign folder (`GetCampaignFolderPath(CampaignType.SinglePlayer, name)`); OAB workspaces in a campaign `Workspaces` folder.

Loading screens: `data/loadingscreens/LoadingScreens.json` lists images per curtain context (`EnterGamePlay`, `LeaveGamePlay`, `ToVABKSC`, `ToTrainingCenter`, `ToKSC`, `ToResearchAndDevelopment`, ...) with 0.5 s fades; `UIManager.SetCurtainContext/SetCurtainVisibility/SetLoadingBarPercentComplete`.

### 1.5 UI views per state

`KSP.game/UIStateViews.cs` defines canvases `FlightHudParent`, `FlightHudGameView`, `FlightHudTimeWarp`, `FlightHUDNonStageableResources`, `FlightHudWorldLabels`, `MapUIParent`, `KSCMenuParent`, `OABParent`, `RDCenterParent`, `RDTechTree`, `TrainingCenter`, `MissionControl`, `GlobalHeaderParent`, `GlobalHeaderTimeWarp`, `NotificationManager`, `PopupCanvas`, `ResearchReportInventory`, and views `MainMenu`, `FlightView`, `FlightMapView`, `FlightTrackingStationView`, `TrackingStationView`, `KSCView`, `OABView`, `RDCenterView`, `TrainingCenterView`, `MissionControl`, `UIHiddenView` (also used for Replay, PhotoMode, PlanetViewer). `NotificationManager` and `PopupCanvas` are active in every view, including the main menu.

---

## 2. Screen inventory

### 2.1 Boot, main menu and campaign creation

**Splash and legal.** `GameManager.CreateSplashScreens` (skippable with any `SkipSplashScreen` binding: click, space, escape, enter). `LegalMenu` (`KSP.Game.StartupFlow/LegalMenu.cs`) shows EULA, privacy policy, terms of service and a photosensitivity warning, each versioned and stored in `PersistentProfileManager`; actions `LegalAccept`, `LegalBack`, `Open*Link`. OSE needs only a first-run notice.

**Main menu (`LandingHUD`, `KSP.Game.StartupFlow/LandingHUD.cs`).** Actions: `SinglePlayer`, `Settings`, `LogIn` (Private Division account; compiled out by `MultiplayerCompileFlagsUtil.ShouldIncludePDLogInMenuOption() => false`), `Exit` (confirm box "Menu/MainMenu/ExitWarning"), `Extras`, `Credits` (`CreditsMenu.cs`, `CreditScreenEnteredMessage`), `EULA`, `PrivacyPolicy`, `TermsOfService`, `MainMenuBack`. A `MultiplayerMenu.cs` exists with an empty data context; `ShouldIncludeMultiplayerMenuOption()` returns false. So the shipped game has no multiplayer screen; OSE must design its own (section 4).

**Campaign menu (`KSP.game/CampaignMenu.cs`).** Tiles of existing campaigns (`CampaignTileEntry`) from `SaveLoadManager.GetCampaignSaveFiles(SinglePlayer)`, each showing thumbnail and metadata; delete button (`DeleteCampaignFolder`, warning "Menu/SaveLoad/DeleteGameWarning"); "new campaign" opens `CreateCampaignMenu`.

**Create campaign (`KSP.game/CreateCampaignMenu.cs`).** Sub-menus: `OpenGameModesMenu` (`GameModePicker`: one toggle per mode from `data/gamemodes/*.json`, title and description loc keys `GameMode/<Id>`), `OpenDifficultyMenu` (`DifficultyOptionsMenu`), `OpenCampaignNameMenu`, `OpenAgencyNameMenu`, `OpenAgencyFlagAndColorsMenu` (`AgencyFlagManager`: `selectedFlagIcon`, `baseColorValue`, `accentColorValue`, `setAgencyDefaultColors`; `AgencyColorPicker`), `ToggleFTUEConfirmationPanel` (first-time-user hints on/off, `NewPlayerSetupInfo.IsFTUEEnabled`), name collision check against existing folders, `StartCampaignCreationFlow`. Data carried: `NewGameSetupInfo` (difficulty level + option states) and `NewPlayerSetupInfo` (agency choice new/existing, name, base and accent colour, flag sprite, FTUE flag).

Game modes (`data/gamemodes/`): `SandboxMode` (no features, no difficulty options) and `ExplorationMode` with features `SciencePoints`, `ScienceExperiments`, `SciencePartUnlock`, `ExplorationMissions`, options `StartingScience` (0-500), `ScienceRewards` (x0.5-2), `MissionRewards` (x0.5-2), presets Easy/Normal/Hard/RocketScientist. `KSCMenuManager` and `GlobalEscapeMenu` hide R&D and Mission Control when the features are off.

Difficulty options (`data/playeroptions/DifficultyOptionDefinitions.json`, presets `Easy/Normal/Hard/RocketScientist.json`, heat variants): `AllowRevert`, `AllowQuickLoad`, `IncludeStockVessels` (General); `InfiniteFuel`, `InfinitePower` (Resource); `DockingTolerance` %, `CommNetRequired`, `UnbreakableJoints` (Flight). Enum `DifficultyLevel {Easy, Normal, Hard, RocketScientist, Custom}`; `DifficultyOptionStatesChangedMessage` on change. These are the server-side rules OSE will expose in session creation.

**Load campaign (`KSP.game/CampaignLoadMenu.cs`).** Save list (`SaveLoadDialogFileEntry`), sort toggle (`swapFileSaveSortingMethod`), details panel `playerFlag, campaignName, saveName, science, flights, lastPlayed, universeTime, gameMode`, actions `loadSelectedFile`, `deleteSelectedFile`, confirm/cancel modals.

### 2.2 Global chrome present in every in-session state

**Global header (`KSP.game/GlobalHeaderManager.cs`, `GlobalHeaderTimewarpManager.cs`, `KSP.UI/GH*.cs`).** Top bar: escape-menu button (`escapeMenu`), contextual menu per state (`GHFlightView`, `GHMapView`, `GHVABView` with `NewButton/SaveButton/LoadButton` and `currentWorkspaceVehicleDisplayName`, `GHTrackingStation`, `GHMissionControl`, `GHResearchDevelopment`, `GHTutorialView`, `GHColony`), time-warp widget bound to `UniverseDataProvider` (`TimeRateIndex`, `MaxTimeRateIndex`, `TimewarpRateDisplay`, `IsUniverseTimePaused`, `TimeMode`, `TimeDisplay` UT, `HaveManeuvers`, `ManeuverMarker`, `PressPlayAction`), player flag/name/agency/science (`UIManager` global properties `PlayerFlag, PlayerName, PlayerAgency, PlayerScience, PlayerPlayTime, GameMode, IsMultiplayer, IsExplorationGame, ServerName, Admin`). The unused `ServerName`/`Admin`/`IsMultiplayer` fields show a multiplayer header was planned.

**Breadcrumbs (`KSP.game/BreadcrumbsController.cs`, `KSP.UI/Breadcrumbs*.cs`).** Star > body > vessel/EVA/colony chain (`BreadcrumbsType {Star, Celestial, Vessel, EVA, Colony}`); clicking a crumb returns to the last state or KSC.

**Escape menu (`KSP.game/GlobalEscapeMenu.cs`, `KSP.UI/ESCMenu*.cs`).** Actions `resumeGame, saveGame, loadGame, revertToLaunchpad, revertToVab, recoverVessel, colony, ksc, trackingStation, missionControl (if feature), trainingCenter, researchDevelopment, vehicleAssemblyBuilding, launchpad, runway, settings, quitToMainMenu, quitGame`; properties `isMissionControlAvailable`, `CanLeaveCurrentVessel`; availability toggles in `ESCMenuActionElementsController` (`SetRevertAvailability`, `SetTrackingStationAvailability`, `HideActionsForMPOnlyModes`, `HideActionsForReadOnlyModes`, `SetQuittingActionButtonNames(isHost)`), a quicksave icon (`ESCMenuQuicksaveIconController`) and player info panel (`ESCMenuPlayerInformation`). The MP-only and host-aware hooks are the original team's multiplayer intent.

**Save/load dialog (`KSP.game/SaveLoadDialog.cs`).** In-session: `saveNewFile, loadSelectedFile, confirmWarningModal, swapFileSaveSortingMethod, returnToEscapeMenu`; shows `activeVessel, currentLocation, timeDisplay` in addition to the main-menu fields. Hotkeys F5 quicksave, F9 quickload (hold variant controlled by accessibility setting `ToggleQuickLoad`).

**Settings (`KSP.UI/SettingsMenuManager.cs` + one manager per tab).** Tabs: General (`Language`, `DifficultyLevel` with `SetToCustomDifficulty`), Gameplay (`MaxPersistantDebris`, `MaxPatchesToDisplay`, `SkipLaunchSequence`, `ShowFPS`, `DefaultThrottleInPrelaunch`, `ActivateTooltips`, `IsFTUEenabled`, `IsEnhancedJointsEnabled`, `EnhancedJointsMode`, `IsMultiJointsEnabled`), Input (`InputRebindingController`, `KSP.Input/`), Audio (`MasterLevel, AmbienceLevel, SFXLevel, VOLevel, MusicLevel, KerbalCommunicationChatter, UISounds, AudioListeningMode, TabAwayAudio, IsKerbalIVAEnabled`, `MuteAudioSettings`), UI (`ShowVesselLabels`, `TemperatureGauges`, `ShowNavballInMapView`, `GlobalUIScale`), Accessibility (`SubTitleTextSize`, `TextSpeed`, `TextAutoContinue`, `ToggleQuickLoad`), Graphics (`GameScreenMode, VSync, Resolution, AntiAliasing, AnisotropicFiltering, PC quality preset, EnvironmentProp, ScatterDensity, ScatterClutterDrawDistance, WaterQuality, ShadowDetails, ShadowQuality, CloudQuality, GroundShadingQuality, TextureQuality`), Multiplayer (hidden; `ShouldIncludeMultiplayerMenu`). Shared: `ApplySettings`, `ResetSettings`, `ResetAllSettings`, `BackButton`, description panel, confirm prompt. Persisted by `PersistentSettings`/`PersistentProfileManager`.

**Notifications (`KSP.game/NotificationManager.cs`, `NotificationEvents.cs`, `NotificationUI*.cs`).** Tiers `Alert, Triumph, Passive, Admin`; importance High/Medium/Low; delivery `Multiline/SingleLine`; `DismissAllNotificationsMessage` on every state change. `NotificationEvents` turns these messages into toasts: vessel recoverable, observer cannot leave vessel, vessel lost/insufficient control, out of electricity, left CommNet range, non-active vessel on crash course / near maneuver / entered SOI, deployable availability, OAB user and error messages, invalid symmetry/anchor/paste, mouselook on/off, camera mode, docking vs normal controls, ablator depleted, solar panels ineffective, throttle locked by warp, warp cannot increase (altitude), warp decreasing for orbital event, forced warp decrease, pause/unpause, engine air-breathing mode, audio/music muted. Loc keys `Flight/NotificationEvent/*`.

**Message boxes and dialogs (`KSP.UI/KSP2MessageBox.cs`, `UIManager.LoadAndShowMessageBox(title, text, icon, buttons, modal, cancel...)`, `TutorialMessageBox`, `TutorialFullScreenVideo`, `CharacterDialogWindow` with `characterName, characterSprite, autoContinue, playNextDialog`).** Windows are `KSP2UIWindow` with drag and resize (`KSP2DraggableWindow`, `KSP2ResizableWindow`). Tooltips: `KSP2ComplexTooltip` (tabbed, expand with Shift, freeze with right click), `Basic2DTooltip`, part info overlay (`PartInfoOverlay`, `PartInfoContext`).

**Cheats / debug (`KSP.game/CheatsMenu.cs`, Alt+F8).** Pages General, Teleport (Orbit: `SMA, Ecc, Inc, LAN, ArgPe, MeanAnomaly, Epoch, Pe/Ap altitude`; Surface: `lat, lon, alt, speed`; Rendezvous: offsets and attitude relative to a target), `AddScience`, Aero GUI toggle (`KSP.UI.Flight/AeroGUI.cs`, F12), show loc keys. Debug window Alt+F7 (`KSP.DebugTools`), MP debug Ctrl+Shift+F8, biome debug Shift+F10, UI toggle F2, FPS counter (`FPSUIManager`). `PhysicsSettings.cs` holds cheat flags (`UnbreakableJoints` propagated by `GameInstance.BroadcastCheatValueChanged`).

### 2.3 Kerbal Space Center overview

`KSP.UI/KSCMenuManager.cs`, `KSCButton.cs`, `KSCTargetableBuilding.cs`, `KSCBuildingTooltip*.cs`, `KSCLineRenderer.cs`, `KSCNavEnum.cs`. A 3D fly-over of KSC with clickable buildings and a side menu: VAB, Tracking Station, Research and Development (feature-gated), Mission Control ("AstronautComplex" enum, feature-gated), Training Center, and a launch-site flyout (`LaunchLocationManager`, `LaunchLocationItem`) listing Launchpad 1-4, Runway 1-2, Boat Launch, generic site. Hovering a building shows a tooltip; clicking plays `KSPAudioEventManager.OnKSCBuildingClick`. Hotkeys: M opens the tracking station map, scroll zooms. Loc "Menu/KSC/KSC". Clicking a launch site opens the **Launchpad dialog** (`KSP.game/LaunchpadDialog.cs`): tabs `savedVehiclesList` / `multiplayerList` (!), sort by name/mass/resources, `hideStock`, vehicle card (`vehicleThumbnail, vehicleName, vehicleDescription, criticalConcerns, missionFlag`), `editVehicle` (to VAB), `showCrewManager` (`KerbalManager` drag-drop of roster kerbals into seats), `launchVehicle`, and if a vessel already sits on the pad: `atLaunchpad`, `launchpadVehicleName`, `launchpadClearMessage`, `recoverVehicle`. Stock craft come from `data/stockassemblies/*.json`.

### 2.4 Vehicle Assembly Building (OAB)

Core: `KSP.OAB/ObjectAssemblyBuilder.cs` (3000+ lines), `ObjectAssemblyBuilderHUD.cs`, `ObjectAssemblyToolbar.cs`, `AssemblyPartsPicker.cs`, `ObjectAssemblyHeader.cs`, `ObjectAssemblyWorkspace.cs`, `OABSaveDialog.cs`, `OABLoadDialog.cs`, `ObjectAssemblyUIEngineeringReport*.cs`, `OABAutosave.cs`, `ObjectyAssemblyBuilderHistorian.cs` (undo), `ProceduralWing*/ProceduralFairing*`. Variants `OABVariant {VAB, BAE}`, environments `Terrestrial, Orbital, GroundColony*` (only VAB/Terrestrial/Kerbin ships).

Player-visible elements and data:

- **Header** (`ObjectAssemblyHeader`): workspace/vehicle name (`GHVABView.currentWorkspaceVehicleDisplayName`), New/Save/Load, launch-site dropdown (`launchLocationOption`), Launch button with status and total delta-v (`launchButtonStatus`, `launchButtonDeltaV`, `launchAvailable`).
- **Parts picker** (`AssemblyPartsPicker`, `ObjectAssemblyCategoryButton`, `ObjectAssemblyFilterButton`): categories `All, Favorites, Command, Fueltanks, Engines, Structural, Coupling, Payloads, Aerodynamic, Ground, Thermal, Electrical, Communication, Science, Utility, Subassemblies` (`AssemblyBuilderPartCategory`); filters by size, fuel type (`data/ui/PartsPickerConfigData.json` fuel order Methalox, SolidFuel, MonoPropellant, Methane, Xenon, Hydrogen, ...), mass, type, tech level (`AssemblyPartsTechLevelFilter`, locked parts hidden/greyed per `TechNodeData.UnlockedPartsIDs`); sort `Default/Name/Mass/Resources`; search (Ctrl+F, `OABPartSearchQueryRunner`); favourite with middle click; extended/collapsed picker; part tooltip (`ObjectAssemblyTooltip`, `PartInfoOverlay`). `OABCategorySelected`, `PartAddedToFavorites` messages.
- **Toolbar** (`ObjectAssemblyToolbar`): tool enum `SELECTION, TRANSLATE_ROTATE, ANCHOR, LAUNCH_ASSEMBLY, COLOR_MANAGER` (`OABToolType`; manipulation modes `SELECT, MANIPULATE, SETANCHOR, SETLAUNCH, COLOR`), undo/redo, angle snap (`toggleOABSnap`, C), symmetry flyout `MIRROR, X1, X2, X3, X4, X6, X8` (`BuilderSymmetryMode`, X / Shift+X cycle, `VABSymmetryModeChangedMessage`), local/global alignment toggle, orientation toggle (VAB vs horizontal), centre-of-mass / thrust / drag(pressure) / radiation markers (`ObjectAssemblyCoMMarker`, `CoTMarker`, `CoPMarker`, `CORadiationOverlay`).
- **3D viewport**: orbit camera (right-drag look, middle/arrow pan, scroll zoom, Home reset, F frame of reference), part grab/place with stack and surface attach (`AssemblyPartAttachmentType`, Alt disables stack attach), rotate with WASDQE, Space resets rotation, Delete/Backspace deletes, Ctrl+C/V copy/paste, Ctrl+Z/Y undo/redo, Ctrl+A select whole assembly, blueprint mode (`BlueprintModeEnabledMessage`). Multiple assemblies per workspace; one is the main assembly (`AssemblySetMain`, `OABMainAssemblyChanged`); anchor part (`ObjectAssemblyPartAnchorIcon`). Procedural wings and fairings have gizmos and a `ProceduralPartUI` window; fairing editing (`EditFairingGizmo`, `FairingPanel`).
- **Staging stack** (shared driver `KSP.UI/OABStagingUIDriver.cs`, `StagingDataProvider`, `StageDataContext`): per stage `deltaV, twr, burnTime, startMass, endMass, rawThrust, isp`, part groups with icons and symmetry sets, drag-drop between stages (`moveStage`, `moveStagePartGroup`, `addStage`, `removeStage`), expanded states, `TotalDeltaV`. `StageAddedMessage`, `StageRemovedMessage`, `PartStageChangedMessage`.
- **Delta-v tool** (`KSP.game/DeltaVToolManagerUI.cs`): choose body, vacuum vs atmosphere, altitude slider, apply to all stages; defaults in `data/ui/DefaultDeltaVSituationData.json`; `CalculateStageDVForSituation` message.
- **Engineer report** (`ObjectAssemblyEngineerReport`, `ObjectAssemblyUIEngineeringReport*`): stats (`EngineeringReportStats`: mass, part count, size vs `VABSizeLimits`, cost) and a flaw list with severity `None/Low/High` from checkers `AssemblyCheckEnginesNecessaryFuels, MissingDataTransmitter, MissingRCS, MissingResource, NotEnoughThrust, CommandPodWithNoParachute, ContainsPartCategory, MissingHeatShield (parts listed in data/ui/EngineeringReportFlawListCheckData.json), MissingRadiator, NoCrewWithFoundersModule, OABHasEnoughResources, WithinOABBounds`. `OpenEngineersReportWindowMessage`.
- **Parts manager / action groups** (`KSP.game/PartsManagerCore.cs`, `ActionGroupManager.cs`): part property editing (toggles, sliders, dropdowns, buttons per `PartsManager*Property*.cs`) and assignment of actions to groups `Stage, Gear, Lights, RCS, SAS, Brakes, Abort, SolarPanels, RadiatorPanels, Science, Custom01..10` (`KSP.Sim/KSPActionGroup.cs`).
- **Crew assignment**: `KSP.UI/KerbalManager.cs`, `KerbalManagerPanel.cs` (drag-drop roster to seats, `kerbalCount`).
- **Colour tool** (`ObjectAssemblyColorPicker`, `ObjectAssemblyColorTool`): agency colours on parts.
- **Save/Load dialogs**: save `workspaceName, workspaceVehicleName, workspaceDescription, workspacePreviewThumbnail`; load `loadWorkspace, mergeWorkspace, deleteWorkspace`, sort by name/mass/resources. Autosave (`OABAutosave`). Message-bus events `OABLoadedMessage`, `OABNewAssemblyMessage`, `FirstPartPlacedMessage`, `PartPlacedMessage`, `AssemblyMassChangedMessage`, `SubassemblyLoadedMessage`.

### 2.5 Flight view HUD

Container `KSP.UI.Flight/UIFlightHud.cs` with instruments (`UIFlightHUDInstrument`). Data comes from the providers in `KSP.game/`: `VesselDataProvider`, `TelemetryDataProvider`, `UniverseDataProvider`, `StagingDataProvider`, `ResourceDataProvider`, `ViewDataProvider`.

| Instrument | File | Shows / does |
|---|---|---|
| Navball | `KSP.UI.Flight/NavballInstrument.cs`, `TelemetryDataProvider` | `NavballRotation`, markers `Prograde/Retrograde/Normal/AntiNormal/RadialIn/RadialOut/Maneuver/Target/AntiTarget/Highlight` vectors, `Heading`, SAS mode buttons `SasStabilityAssist, SasPrograde, SasRetrograde, SasNormal, SasAntiNormal, SasRadialIn, SasRadialOut, SasTarget, SasAntiTarget, SasManeuver` (`KSP.Sim/AutopilotMode.cs` adds `Navigation`, `Autopilot`), `SasToggle`/`RcsToggle` actions, `CanSetSASOrientationModes/TargetMode/ManeuverMode`, `VesselControlStatus`; `NavballAligned*Message` events |
| Throttle | `ThrottleInstrument.cs`, `VesselDataProvider.Throttle` | throttle gauge; keys Shift/Ctrl, Z max, X cut; `ThrottleChangedMessage` |
| Altimeter / speed | `TelemetryDataProvider` | `AltimeterMode` (sea level vs terrain, toggled by `changeALT`), `AltimeterDisplay`, `VelocityMode` `Orbit/Surface/Target` (`KSP.Sim/SpeedDisplayMode.cs`, `changeVEL`), `VelocityDisplay`, `VerticalSpeedIndicator` (`VerticalSpeedInstrument.cs`), `GForce` |
| Atmosphere gauge | `AtmosphericIndicatorInstrument.cs` | `AtmosphereStaticPressurekPa, Temperature, Density, Normalized` |
| Orbital readout | `KSP.UI/OrbitalReadoutInstrumentManager.cs` | `ApoapsisDistance, PeriapsisDistance, ApoapsisTime, PeriapsisTime` from `PatchedConicsOrbit.OrbitMarkerHandler` |
| Burn timer | `BurnTimerInstrument.cs`, `KSP.UI.Binding/BurnTimerLight*` | countdown to node, lights; `ManeuverPerfectBurnTimedMessage` |
| Time warp | `TimeWarpInstrument.cs`, `UniverseDataProvider`, `KSP.Sim.impl/TimeWarp.cs` | rate index and display; levels from `Assembly-CSharp/PhysicsSettings.cs`: 1, 2, 4, 10, 50, 100, 1000 (min alt 50 km), 10k (150 km), 100k (500 km), 1M (4000 km); physics warp vs on-rails (`IsPhysicsTimeWarp`), `WarpTo(UT)`; keys `.` `,` `/`; messages `TimeZoomChangeActivated/Completed`, `TimewarpCantIncreaseDueToAltitude`, `ForceDecreaseTimewarp`, `TimeWarpToLocationInitialized/Completed` |
| Staging stack | `FlightStagingStackInstrument.cs`, `KSP.UI/FlightStagingUIDriver.cs`, `StagingDataProvider` | stages with part groups, per-stage resources (`fuelOne/fuelTwo type, curAmount, totalAmount, normalized`), parachute states, `canStage`, `hasReadyStage`, delta-v per stage; Space stages (`StageActivatedMessage`) |
| Non-stageable resources | `NonStageableResourcesUI.cs`, `ResourceDataProvider`, `ResourceInfoDataContext` | EC, monoprop etc.: `ResourceName, Abbreviation, Icon, CurAmount, TotalAmount, Units, AmountNormalized` |
| Vessel status | `VesselStatusInstrument.cs`, `VesselDataProvider` | `VesselStatusBarText` (situation), `MissionEllapsedTime`, recovery menu (`VesselStatusRecoveryStatus`, `VesselStatusMenuContextButtonAction`: recover or back to KSC) |
| Action bar | `ActionBar.cs`, `ActionBarButton.cs` | action groups `Abort, Brakes, Lights, Gear, Solar, Radiator, Science` with state (`ActionGroup*` properties and `TriggerActionGroup*` actions); variants for vessel, kerbal, colony, science (`ActionBarVisible*`) |
| App bar | `AppBar.cs`, `AppBarButton.cs`, `ViewDataProvider` | toggles for Parts Manager, Resource Manager, Action Group Manager, Mission Tracker, Trip Planner, Flight Report, Delta-v tool, Flag site, Research Inventory, Kerbal Manager, Non-stageable resources, Tracking Station (`Is*Visible` properties, `ToggleTrackingStation`) |
| Function menu | `FunctionMenuInstrument.cs`, `UILocator_FuncitonMenu` | radial/function menu (`ActiveFuncitonMenu`) |
| World labels | `FlightHUDWorldLabelsInstrument.cs`, `UIInfoLabel` | vessel/body labels in 3D (F4 `ToggleVesselLabels`) |
| Portraits | `IVAPortraits.cs` | crew portraits, paging; EVA button *(inferred: `Interact` F / portrait button leads to `EVAUtils`)* |
| Go button | `GoButtonInstrument.cs`, `UIWidget_GoButton` | pre-launch start (`GoButtonPressedMessage`, `PrelaunchSequenceInitiatedMessage`, `LaunchSequenceInitiated/Complete`, setting `SkipLaunchSequence`, `DefaultThrottleInPrelaunch`) |
| Interact prompt | `InteractInstrument.cs`, `KSP.game/InteractSystem.cs` | EVA interact (F, B, Space) |
| Flag site | `FlagSiteUIManager.cs` | plant-flag dialog `OnAcceptFlagSite/OnCancelFlagSite`; `PlantFlagMessage`, `FlagPlantedMessage` |
| Flight report | `FlightReportUIManager.cs` | on `VesselRecoveredMessage` / `ActiveVesselDestroyedMessage`: status text, records (`MissionTime, Altitude, SpeedOverLand, SpeedOverall, GroundDistance, TotalDistance, GForce` from `*RecordMessage`), travel log (`TravelLogEventAdded`), research items, buttons `OnReturnToVAB, OnReturnToLaunch, OnKSC, OnTrackingStation, OnTrainingCenter` |

Windows opened from the app bar: **Parts manager / PAM** (`PartsManagerCore`, right-click part or `TogglePartsManager`; per-part entries, favourites, destroy), **Resource manager** (`KSP.UI/ResourceManagerUI.cs`: families, parts, containers, transfer list, `StartResourceTransfer/StopResourceTransfer`, flow states), **Action group manager** (`ActionGroupManager`), **Trip planner** (`KSP.game/TripPlanner.cs`, nodes from `data/ui/TripPlanner.json`: origin, destination, one-way/round trip, `deltaV`, `InclinationCost`, `AerobrakePossible`, launch-window warp `LaunchButton`, `displayTargetUT`), **Mission tracker** (`Assembly-CSharp/MissionTracker.cs`), **Research inventory** (`KSP.UI/ResearchInventoryManagerUI.cs`: reports with filters by body/situation/experiment/type, `TransmitReport`, `PauseReport`, `RemoveReport`, `TransmitAllReports`, totals of sample and data science, EC and time to transmit, `VesselCanTransmit`, `TransmitErrorMessage`), **Kerbal manager** (crew transfer), **Delta-v tool**.

Cameras: `KSP.Sim/FlightCameraMode.cs` `AUTO, FREE, ORBITAL, CHASE, LOCKED` (V cycles), mouselook Tab, free camera middle mouse, `KerbalCameraMode {ORBITAL, FREE}` on EVA. Flight input map (`KSP.Input/GameInput.cs`, bindings parsed from the embedded asset): pitch W/S, yaw A/D, roll Q/E, translate IJKL/HN, precision CapsLock, trim Alt, SAS T, RCS R, gear G, brakes B, lights U, action groups 1-0, map M, focus next/prev ] [, camera V, wheel steer A/D and throttle W/S, interact F/B/Space, labels F4, aero GUI F12, delete toggles rot/lin controls.

EVA (`KSP.Sim.impl/EVAInputHandler.cs`, `KSP.game/ClimbSystem.cs`, `InteractSystem.cs`, `KSP/EVAUtils.cs`): move W/S/A/D, strafe/yaw Q/E, up/down Shift/Ctrl, rotate IJKL, run Shift, jump Space; interactions (ladder climb, board, plant flag, take sample via `ScienceActionGroup`), `EVAActionInitiated/Completed` messages. A kerbal on EVA is a vessel (`KerbalComponent`) so the HUD stays the same with the kerbal action bar.

### 2.6 Map view and Tracking Station

Core `KSP.Map/MapCore.cs`, `Map3DView.cs`, `MapCamera.cs`, `OrbitRenderer.cs`, `Map3DFocusItem.cs`, `Map3DTrajectoryEvents.cs`, `Map3DManeuvers.cs`, `MapManeuverGizmo.cs`, `MapUI.cs`, `Map3DPopup.cs`, `Map3DTooltips.cs`. Modes `MapMode {Hidden, FlightMap, FlightTrackingStation, TrackingStation}`; `MapModeChangedMessage`, `MapInitializedMessage`, `MapFocusChangedMessage`, `MapOrbitAdded/RemovedMessage`, `MapRequestFocusMessage`, `MapRequestSelectionMessage`.

Elements:

- **3D scaled-space scene**: bodies, orbit lines (`OrbitRenderer`, patches coloured per conic, configurable patch count via `MaxPatchesToDisplay`), items of type `CelestialBody, Debris, Vessel, Astronaut, Colony, RescueBeacon, Monument, Waypoint, ManeuverNode, Flag, DeliveryRoute, KSC` (`MapItemType`), icons that squish/stack (`Map3DPopup.PerformSquish`). Camera: left-drag rotate, scroll zoom, Home reset, click `Focus`, ] [ next/previous item, M hides map.
- **Trajectory event markers** (`Map3DTrajectoryEvents.EventType`): `ClosestApproach, Capture, Acceleration, ExitingSOI, EnteringSOI, EndThrust, PartialOutOfFuel, CompletelyOutOfFuel, Collision, Periapsis, Apoapsis, Intersect1A/1B` (and AN/DN *(inferred)*), each with a tooltip (`KSP.UI/TrajectoryEventTooltip.cs`); `SOIChangePredictedMessage`, `InterceptAchievedMessage`.
- **Maneuver nodes**: click an orbit (`OrbitClickedMessage`) -> `AddManeuver`/`WarpTo` popup with `TimeToPoint`; gizmo with six handles `Prograde, Retrograde, Normal, AntiNormal, RadialOut, RadialIn` (`ManeuverGizmoInputType`), states `Expanded, Interpolating, CollapsedMenu, CollapsedIcon, Movement`, actions `DoMove` (drag node along orbit), `DoTrash`, `DoWarp`, `DoRevive`; messages `ManeuverCreated/Removed/Finished`, `ManeuverNodeClickedMessage`, `ActiveManeuverNodeReachedMessage`. Burn details appear on the navball/burn timer rather than in a separate panel.
- **Object list / info panel** (`MapUI.cs`, `MapUISelectableEntry.cs`): search (`UpdateSearch`), filters by `TrackingStationObjectTypeFilter` (`CelestialBody, Colony, Vessel, GroundedVessel, InFlightVessel, Astronaut, DeliveryRoute, Flag, RescueBeacon, Monument, Waypoint, ManeuverNode, Debris`), sort by bodies / object type / favourites, nested tree (`NestingToggleType`), favourite, rename (`EditObjectEntryName`). Selected-item panel: for vessels `TrajectoryType, Altitude, Velocity, DeltaVRemaining, PowerStored`, crew, alerts, actions `OnFocus, OnControl, OnVAB, OnDestroy, OnRecover, OnGoToKSC`; for bodies `SemiMajorAxis, OrbitalVelocity, Eccentricity, SiderealPeriod, Inclination, AxialTilt, Circumference, Radius, RotationVelocity, Mass, SolarDay, Density, Gravity, Atmosphere, OxygenPresent, SphereOfInfluence, TemperatureRange, WaterPresent, ...`, synopsis, `DisplayResourceOverlay`, `DisplayBiomeOverlay` (science regions, `KSP.Game.Science/PQSScienceOverlay.cs`).
- **Tracking Station** is the same `MapUI` in `TrackingStation` mode entered from KSC, with the object list open and no active vessel; `TrackingStationLoadedMessage`, `GHTrackingStation` header, `FlightTrackingStationView` when toggled from flight (`ToggleTrackingStation`).
- Navball may be shown in map view (`ShowNavballInMapView` setting). Time-warp and header stay visible.

### 2.7 Research and Development

`KSP.UI/ResearchDevelopmentController.cs`, `KSP.Research/ResearchDevelopmentTechTreeController.cs`, `ResearchManager.cs`, `TechnologyNode.cs`, `KSP.Game.Science/RDCenterUIController.cs`, `TechNodeView.cs`, `TechNodeDataStore.cs`, `RDCameraManager.cs`, `RDZoomPanel.cs`, `RDPinMapTool.cs`. Data: `data/technodedata/tNode_*.json` (85 nodes in tiers 0v-4v: `ID, NameLocKey, IconID, RequiredSciencePoints, UnlockedPartsIDs, RequiredTechNodeIDs, TierToUnlock, TechTreePosition`), `data/research/TechTest*.json` are leftovers of an older `Technology` schema with `Benefits`.

Screen: pannable, zoomable tree (arrows, drag, scroll, `ScrollTechTree`), nodes with states `Hidden, Visible, Purchasable, Owned` (`TechnologyVisibilityType`), tier bands with `CurrentTechTreeTier`, `TierUnlockNotification`, `TierToUnlock`; side panel `TechNodeName, Description, RequiredTechnologies, RequiredSciencePoints, Unlockable, Unlocked, HasEnoughSciencePoints`, part miniatures with tooltip (Shift expands), `UnlockTechNode` button text, science balance (`SciencePointBalance`), PAIGE helper text. Messages `TechResearchedMessage`, `TechTierUnlockedMessage`, `ResearchMenuOpenedMessage`.

Science data model (`KSP.Game.Science/`): experiments (`data/scienceexperiments/CrewReport.json`: type Data/Sample/Both, valid body+situation+region), situations `HighOrbit, LowOrbit, Atmosphere, Splashed, Landed`, regions per body (`CelestialBodyScienceRegionsData`), reports stored on the vessel then transmitted (needs transmitter + EC + time) or recovered; messages `ResearchReportAcquired/Stored/Transferred/Scored`, `ScienceEarnedOnRecoveryMessage`, `SciencePointCapacityUpdateMessage`; `ScienceManager.UpdateSciencePointCapacity`. In flight the vessel data provider exposes `ScienceResearchOpportunityAvailable`, `ScienceRegionState`, `ScienceResearchExperimentInProgress`.

### 2.8 Mission Control

`Assembly-CSharp/MissionControlManager.cs`, `MissionLog.cs`, `MissionPopover.cs`, `MissionTracker.cs`, `KSP.UI/MissionControlMenuController.cs`, `KSP.Game.Missions/*`, `KSP.Game.Missions.Definitions/*`, `Assets.Scripts.Missions.Definitions/MissionGranter.cs`. Data `data/missioncontrol/*.json` and `data/explorationmissions/*.json`: ~56 missions, `type Primary|Secondary|Tutorial|FTUE`, `Owner Global|Agency|Player`, stages with `Objective`, `DisplayObjective`, `MissionRewardType` (`None` or `SciencePoints`), `RewardAmount`, actions (`MessageBoxAction`, `FullscreenVideoAction`, `MissionCharacterDialogAction`, `MissionHighlightAction`, `MissionIndicatorAction`, `MissionSetViewStateAction`, `MissionDisableInputAction`, `MissionPauseAction`, `MissionCreateSnapshotAction`, `MissionReloadSnapshotAction`, `MissionSlideshowAction`, `MissionScriptAction` Lua), conditions `EventCondition` (a `KSP.Messages` type observed), `PropertyCondition`, `ConditionSet`, `ScriptCondition`, branches (`Debrief`, `OnSubmit`). Primary chain PM01-PM11 (Kerbin x3, Mun or Bust, Mun OSS, Mun, Minmus bad signal, Minmus, Duna, Jool, Tylo); secondaries per body ("Land on X!", experiments, flags, rovers, keostationary orbit...).

Screen: a 3D mission-control room backdrop, left mission log with tabs (active/completed, primary/secondary) and status `Default, New, Completed, Submitted, Tracked`, right pop-over `MissionTitle, MissionDescription, objectives, scienceReward, granterName/Description/Icon, TrackMission (toggle), MissionBriefing, MissionDebriefing (character dialog), SubmitMission (only when completed, not yet turned in)`, shortcut buttons `vab`, `researchDevelopment`. Submitting runs the `Debrief` branch, then `FinalizeSubmitMission` marks `TurnedIn`, activates `OnSubmit`, moves the entry to completed, updates science capacity; `MissionTriumphWindow` (`MissionTitle, TotalSciencePoints, triumphVideo, DismissTriumphWindow`) is shown on completion (`SpawnTriumphWindow`, `MissionCompleteMessage`, `OnMissionTriumphDismissed`). In flight the **Mission tracker** window lists tracked objectives (`OnMissionTrackingToggled`, `MissionStageCompleted`). `NewMissionAvailableMessage` drives a notification.

### 2.9 Training Center, tutorials and first-time user experience

`KSP.UI/TrainingCenterMenuController.cs`, `TutorialsItems.cs`, `MainTutorialsItem.cs`, `LessonTutorialsItem.cs`, `TutorialMessageBox.cs`, `TutorialFullScreenVideo.cs`, `KSP.Game.Missions/KSP2MissionManager.cs`. Data `data/tutorials/`: four groups (`KSP2MainTutorial_1/2/3/5.json`: "Space is the Place", ...) each with an animation video lesson ("How Rockets Work", "Missing the Ground", "Orbits are Weird", "Orbital Transfers") and 4-6 lessons (1.1 Launch a Test Rocket, 1.2 Simple Rocket Construction, 1.3 VAB Camera Controls, 1.4 Staging a Rocket, 1.5 Time Zoom and Altimeter, 1.6 Parachuting and Recovery; 2.1 Stack Decoupler and Subassemblies, 2.2 Radial Boosters and Symmetry, 2.3 Editing the Staging Stack, 2.4 SRBs, Navball and Gravity Turn, 2.6 Circularization and Throttle; 3.1 Earning Science, 3.2 Manipulating Orbits, 3.3 Electric Charge, 3.4 Deorbit Burn, 3.5 Heat Shields; 5.1 Planning a Maneuver, 5.2 Executing a Maneuver Plan, 5.3 Capturing). Each lesson is a mission JSON (`type Tutorial`, `MissionGroup TutorialN`, 8-23 stages) started on a dedicated save (`Tutorial*_Save.json`); lesson types `Default, Video, Flight, VAB, VAB_Flight`. Screen: agency name, tutorial cards with progress icons, lesson list, reset (`MissionReset`), start (`MissionActivated`), exit. Messages `StartTutorial`, `GoToNextTutorialStep`, `EndTutorial`, `AbortTutorial`, `ResetTutorial`, `TutorialStarted/Ended/ReStarted`, `LessonMenuUIOpened/Closed`.

FTUE (`data/firsttimeuserexperience/FTUE_*.json`, 21 scripts, `type FTUE`, enabled per player via `NewPlayerSetupInfo.IsFTUEEnabled` and settings `IsFTUEenabled`; `FTUEToggled` message; `KSP2MissionManager.LoadFTUEProgress`): triggered by events, each shows PAIGE character message boxes (`imageKey PAIGE_emote_*`, loc `[FTUE/...]`): New Game (intro video + welcome), VAB, Flight, Map, Tracking Station, Training Center, Mission Control, R&D, First Maneuver, First EVA, Experiment, Research Report, Mission Submit, Mission Complete, First Purchasable Tech, First Recoverable, Launch Too Big, Lost CommNet, Lost Power, Overheat, Vessel Stranded.

### 2.10 Kerbals and agency

Roster: `KSP.game/KerbalRosterManager.cs`, `KerbalInfo.cs`, `KerbalAttributes.cs` (name, surname "Kerman", origin, type, hair colour, constitution, biography, stupidity, bravery, veteran), `KerbalVarietySystem.cs` (cosmetics from `data/kerbal_variety_text/*.json`, 100+ named kerbals plus per-language name lists), `KerbalPhotoBooth.cs` (portraits). Messages `KerbalAddedToRoster`, `KerbalRemovedFromRoster`, `KerbalLocationChanged`. There is no astronaut-complex screen in 0.2.2; kerbals are managed in the VAB/Launchpad crew manager and in flight (`KerbalManager`). Agency: `AgencyManager.cs`, `AgencyEntry.cs`, `AgencyFlag*.cs` (flags from the `agencyflags` bundle), per-campaign players `CampaignPlayerManager` (`CampaignPlayerEntry` records starting/historical game state, tracked missions, OAB snapshot). Both managers carry `*MPAction` callbacks (`AgencyMPAction`, `CampaignPlayerMPAction`, `ScienceMPAction`): the original authoritative-replication hooks.

---

## 3. User flows

Each step names the screen (section 2) and the code that drives it.

**F1. Start a new campaign.** 1 Splash -> legal (first run) -> Main menu (`LandingHUD`). 2 Single Player -> Campaign menu (tiles). 3 New -> Create campaign: pick mode (Exploration/Sandbox), difficulty preset or custom options, campaign name, agency name, flag and colours, FTUE on/off. 4 Start: load flow `Load_StartNewCampaign` with curtain `EnterGamePlay`, autosave. 5 KSC overview; with FTUE on, `FTUE_New_Game` plays the intro video and PAIGE welcome.

**F2. Build a rocket and launch.** 1 KSC -> VAB (curtain `ToVABKSC`, `OABLoadedMessage`). 2 Pick parts by category (Command pod first; `FirstPartPlacedMessage`), stack and surface attach, symmetry X, snap C, rotate. 3 Watch staging stack and total delta-v; open Engineer report and fix flaws; optionally Parts manager for part settings and Action group manager. 4 Crew via Kerbal manager. 5 Save workspace (header Save). 6 Choose launch site, press Launch: `VABLaunchButtonPressedMessage` -> `SpawnAssembly` -> FlightView with pre-launch Go button (or skip sequence). Alternative: KSC -> Launchpad dialog -> saved vehicle -> Launch.

**F3. Fly to orbit.** 1 Flight HUD: SAS on (T), throttle max (Z), Space to stage. 2 Watch altimeter, speed (surface mode), navball heading; gravity turn. 3 Stage as the staging stack shows empty fuel; `StageActivatedMessage`. 4 Open map (M) to watch Ap/Pe markers; cut throttle (X) at target Ap. 5 Warp to Ap (`WarpTo`), SAS prograde, burn to circularise (`StableOrbitCreatedMessage`). 6 Orbital readout shows Ap/Pe; quicksave F5.

**F4. Plan a maneuver and reach the Mun.** 1 Map view: click own orbit -> Add Maneuver. 2 Drag prograde handle until the trajectory shows `EnteringSOI` (Mun) and `Periapsis` markers; drag node along orbit (`DoMove`) to tune. 3 Burn timer and navball maneuver marker appear; SAS Maneuver mode; warp to node (`DoWarp`, warp drops near events). 4 Burn until the remaining delta-v reaches zero; `ManeuverFinishedMessage`. 5 Warp; `SOIEnteredMessage`, breadcrumbs change to Mun. 6 New node at Pe to capture (retrograde handle). Trip planner can be consulted for budgets.

**F5. Land, EVA, return and recover.** 1 Node to lower Pe, retrograde burn, surface speed mode, gear (G), SAS retrograde, land (`VesselLandedMessage`). 2 Portrait or interact to EVA (`EVAEnteredMessage`); kerbal action bar: plant flag (`FlagSiteUIManager` dialog), run experiment / take sample (Research inventory shows new report, FTUE "Your first report!"). 3 Board (`EVALeftMessage`). 4 Launch, orbit, node for Kerbin return, aerobrake, parachutes (staging stack shows chute safety state), splash or land. 5 Vessel status menu shows Recoverable -> Recover (`VesselRecoveredMessage`). 6 Flight report: records, travel log, science earned (`ScienceEarnedOnRecoveryMessage`); choose KSC / VAB / Tracking Station. Alternative: Tracking Station entry -> `OnRecover`.

**F6. Do a mission and claim rewards.** 1 KSC -> Mission Control (`MissionControlLoadedMessage`). 2 Select a mission -> pop-over with objectives and science reward; Briefing (character dialog); Track. 3 Fly; Mission tracker window shows objectives; `MissionStageCompleted`; on completion the Triumph window pops with science total. 4 Return to Mission Control; mission is marked Completed -> Submit -> Debrief dialog -> `FinalizeSubmitMission`, science added, entry moves to completed; FTUE "Submitting Your First Mission!".

**F7. Unlock a tech node.** 1 KSC -> R&D (curtain, `ResearchAndDevelopmentLoadedMessage`). 2 Pan/zoom tree, click a Purchasable node; side panel shows cost and parts. 3 Unlock (`UnlockTechNode`) when `HasEnoughSciencePoints`; `TechResearchedMessage`; tier unlock notification when a tier completes. 4 Back to VAB: parts picker now shows the parts.

**F8. Save and load.** Quick: F5 / F9 anywhere in session (`AllowQuickLoad` difficulty rule). Manual: Esc -> Save game -> name -> confirm overwrite modal; Esc -> Load game -> list -> load (curtain, full load flow). From main menu: Campaign tile -> Load menu -> save -> load. Autosaves on campaign creation and configurable events; `UpdateAutoSaves022` migrates old autosaves.

**F9. Multiplayer join.** Not present in the shipped UI: `MultiplayerMenu` is an empty data context, `ShouldIncludeMultiplayerMenuOption()` is false, `ClientMode` has only `None` and `SPPlayer`, `Status.IsClientConnected => false`. What exists is the plumbing: `MPFramework`, `UIHooks` (log in/out), `LaunchpadDialog.multiplayerList`, `ESCMenuActionElementsController.SetQuittingActionButtonNames(isHost)` / `HideActionsForMPOnlyModes`, header `ServerName`/`Admin`, per-player `CampaignPlayerEntry`, `*MPAction` replication callbacks, `OABProvider.HandleLaunchButtonPressforMPSpectators`, `Save_SaveSpecificPlayerToFile`. OSE must design the lobby itself (section 4).

**F10. First-time user path (FTUE on).** Main menu -> Create campaign (FTUE toggle on) -> KSC: intro video, PAIGE welcome, pointer to Training Center or VAB -> VAB: `FTUE_VAB` walks through picker, placing, staging, launch -> Flight: `FTUE_Flight` (stage, throttle, altimeter) -> `FTUE_First_Recoverable` -> Map: `FTUE_Map` -> Tracking Station, Mission Control, R&D on first visit -> event hints (first maneuver, EVA, experiment, report, lost power/comms, overheat, stranded, too big). All are mission JSONs with `EventCondition` triggers; a generic "scripted hint" engine covers them.

---

## 4. Experiences to build (ordered by player need)

Sizes: S under a week, M one to three weeks, L more. Phases refer to `02-architecture-and-plan.md`. OSE currently has only a map view (section 5), so everything below the map rows is new.

### 4.1 Menu, session and game creation (Phase 3-4)

| # | Experience | Size | Phase | Notes |
|---|---|---|---|---|
| A1 | Boot and main menu: Play, Settings, Credits/About, Quit; first-run notice replaces EULA | S | 3 | web page is the "splash" |
| A2 | Session browser (replaces campaign tiles): list saves hosted by the local/LAN server with thumbnail, mode, UT, last played; delete | M | 4 | server endpoint lists saves |
| A3 | Create session: mode (Sandbox/Exploration), difficulty rules (the 8 options + 3 exploration options), name, FTUE toggle | M | 4 | rules enforced by server |
| A4 | Player/agency setup: name, flag, base and accent colour; per-player on join | M | 4 | OSE "lightweight accounts" (ADR-0001) |
| A5 | Join lobby: connect URL, player list, ready, who is host; the screen KSP2 never shipped | M | 4 | |
| A6 | Loading curtain with progress text and images | S | 3 | mirrors `SetLoadingBarText` |

### 4.2 KSC and buildings (Phase 5, visuals Phase 7)

| # | Experience | Size | Phase |
|---|---|---|---|
| B1 | KSC hub screen: building buttons VAB, Tracking Station, R&D, Mission Control, Training Center, launch sites; feature-gated by mode; 2D hub first, 3D fly-over later | S (2D) / L (3D) | 5 / 7 |
| B2 | Launchpad dialog: saved craft list, sort, stock filter, crew assign, launch, recover craft on pad | M | 5 |
| B3 | Global header: UT clock, warp control, player flag/name/science, escape button; multiplayer additions: player list and "who holds warp" | M | 3-4 |
| B4 | Breadcrumbs star > body > vessel | S | 3 |
| B5 | Escape menu with save/load/revert/KSC/buildings/settings/quit; host-aware labels | M | 4 |
| B6 | In-session save/load dialog; quicksave/quickload; autosave | M | 4 |

### 4.3 Editor (Phase 5)

| # | Experience | Size | Phase |
|---|---|---|---|
| C1 | Parts picker: 16 categories, search, filters (size, fuel, mass, type, tech), sort, favourites, part tooltip | M | 5 |
| C2 | 3D assembly: grab, stack/surface attach, rotate, snap, symmetry 1/2/3/4/6/8 + mirror, delete, copy/paste, undo/redo, multiple assemblies and anchor, CoM/CoT/CoP markers | L | 5 |
| C3 | Staging stack with drag-drop and per-stage dV/TWR/burn time | M | 5 |
| C4 | Engineer report (stats + flaw checkers) and Delta-v situation tool | M | 5 |
| C5 | Parts manager (PAM) and action group editor | M | 5 |
| C6 | Workspace save/load/merge, autosave, craft thumbnails; upload craft document to server | M | 5 |
| C7 | Crew assignment and colour tool | S | 5 / 8 |
| C8 | Procedural wings and fairings gizmos | L | 5+ |

### 4.4 Flight (Phase 6)

| # | Experience | Size | Phase |
|---|---|---|---|
| D1 | Navball with markers, SAS mode buttons, SAS/RCS toggles, heading | M | 6 |
| D2 | Throttle, altimeter (ASL/AGL), speed (orbit/surface/target), vertical speed, G, atmosphere gauge | S | 6 |
| D3 | Orbital readout Ap/Pe and times; burn timer | S | 2-3 (readout) / 6 |
| D4 | Flight staging stack with stage resources, parachute states, Space to stage | M | 5-6 |
| D5 | Non-stageable resources panel, resource manager with transfers | M | 5-6 |
| D6 | Action bar (gear, lights, brakes, abort, solar, radiator, science) and action-group hotkeys 1-0 | S | 6 |
| D7 | App bar window toggles; draggable/resizable windows | S | 6 |
| D8 | Vessel status bar, recovery, flight report with records and travel log | M | 6 |
| D9 | Pre-launch Go button and launch sequence; revert to launch / VAB (server snapshot) | M | 6 |
| D10 | Camera modes, mouselook, vessel labels, crew portraits | M | 6 |
| D11 | EVA controls, interact prompts, flag planting, ladder/board | L | 7-8 |
| D12 | Notification toasts (tiers, importance, dismiss on state change) and message boxes | S | 3-4 |
| D13 | Pause and universe-time pause, server-arbitrated warp HUD | S | 4 |

### 4.5 Map and navigation (Phase 2-3, extend in 6)

| # | Experience | Size | Phase |
|---|---|---|---|
| E1 | Full maneuver gizmo: six handles, drag along orbit, trash, warp-to, burn readout on navball | M | 3 |
| E2 | Trajectory event markers (Ap, Pe, SOI enter/exit, closest approach, intersects, collision, out of fuel) with tooltips | M | 3 |
| E3 | Object list with filters, search, favourites, body/vessel info panel, focus/control/recover/destroy | M | 3-4 |
| E4 | Tracking Station mode entered from KSC; `FlightTrackingStation` toggle | S | 4 |
| E5 | Body resource and biome (science region) overlays | M | 8 |
| E6 | Trip planner (dV budgets, launch windows) | M | 8 |

### 4.6 Missions and progression (Phase 8)

| # | Experience | Size | Phase |
|---|---|---|---|
| G1 | Mission Control: log, pop-over, track, briefing/debriefing dialog, submit, triumph window; mission JSON import | L | 8 |
| G2 | Mission tracker window in flight; condition engine over the message bus | M | 8 |
| G3 | R&D tech tree: pan/zoom, tiers, node states, unlock, part previews; import 85 nodes | M | 8 |
| G4 | Science: experiments, situations and regions, research inventory (transmit/recover), science balance | L | 8 |
| G5 | Training Center with tutorial groups and lessons on dedicated saves; scripted-hint engine shared with FTUE | L | 8 |
| G6 | FTUE hints with PAIGE-style character boxes | M | 8 |
| G7 | Kerbal roster, attributes, portraits | M | 8 |

### 4.7 Settings and meta (Phase 4+)

| # | Experience | Size | Phase |
|---|---|---|---|
| H1 | Settings: general, gameplay, input rebinding, audio, UI (scale, labels), accessibility, graphics (WebGPU quality), per-browser persistence | M | 4 |
| H2 | Cheats/debug: teleport to orbit/surface/rendezvous, add science, aero overlay (host-only in multiplayer) | S | 4 |
| H3 | Localisation keys and string tables (see 5.1) | M | 8 |
| H4 | Credits, legal/first-run notice | S | 3 |

---

## 5. Comparison checklist

OSE status today: map view with bodies and orbits, UT and warp HUD, the player's probe with trajectory, a maneuver-node panel (prograde only). Everything else is "none".

| Screen / flow | KSP2 behaviour (one line) | Where in the decompile (`Assembly-CSharp/`) | OSE status |
|---|---|---|---|
| Splash and legal | skippable splash, versioned EULA/PP/TOS/photosensitivity acceptance | `KSP.game/GameManager.cs`, `KSP.Game.StartupFlow/LegalMenu.cs` | none |
| Main menu | Single Player, Settings, Extras, Credits, Exit; login hidden | `KSP.Game.StartupFlow/LandingHUD.cs` | partial (name + Play, Esc resumes) |
| Campaign tiles | list campaigns with thumbnails, delete | `KSP.game/CampaignMenu.cs` | none |
| Create campaign | mode, difficulty, name, agency name, flag/colours, FTUE | `KSP.game/CreateCampaignMenu.cs`, `GameModePicker.cs`, `DifficultyOptionsMenu.cs`, `AgencyFlagManager.cs` | none |
| Load campaign | save list, sort, details, load/delete | `KSP.game/CampaignLoadMenu.cs` | none |
| Multiplayer menu | empty prefab, compiled out | `KSP.Game.StartupFlow/MultiplayerMenu.cs`, `KSP.Networking.MP/MultiplayerCompileFlagsUtil.cs` | none (OSE design) |
| Loading curtain | context images, fades, progress bar | `KSP.game/UIManager.cs`, `data/loadingscreens/LoadingScreens.json` | none |
| Game state machine | 20 states, enter/exit effects, 3 messages per change | `KSP.game/GameStateMachine.cs`, `GameState.cs`, `UIStateViews.cs` | none |
| KSC hub | 3D KSC with building buttons and launch-site flyout | `KSP.UI/KSCMenuManager.cs`, `KSCNavEnum.cs` | none |
| Launchpad dialog | saved craft, crew, launch, recover pad | `KSP.game/LaunchpadDialog.cs` | none |
| Global header | escape, contextual menu, warp, UT, player/science | `KSP.game/GlobalHeaderManager.cs`, `GlobalHeaderTimewarpManager.cs`, `UniverseDataProvider.cs` | partial (UT, warp and holder, player, campaign members) |
| Breadcrumbs | star > body > vessel, clickable | `KSP.game/BreadcrumbsController.cs` | done |
| Escape menu | resume/save/load/revert/recover/buildings/settings/quit | `KSP.game/GlobalEscapeMenu.cs`, `KSP.UI/ESCMenu*.cs` | none |
| Save/load dialog and quicksave | F5/F9, named saves, autosave, flow actions | `KSP.game/SaveLoadDialog.cs`, `SaveLoadManager.cs`, `KSP.Game.Load/*` | none |
| Settings | 7 tabs plus hidden multiplayer | `KSP.UI/SettingsMenuManager.cs` and `*SettingsMenuManager.cs` | none |
| Notifications | tiers, importance, event-driven toasts | `KSP.game/NotificationManager.cs`, `NotificationEvents.cs` | none |
| Message boxes, tooltips, windows | modal boxes, tutorial boxes, character dialog, complex tooltips, drag/resize windows | `KSP.UI/KSP2MessageBox.cs`, `KSP2UIWindow.cs`, `KSP2ComplexTooltip.cs`, `KSP.Game.Missions/CharacterDialogWindow.cs` | none |
| Cheats/debug menu | Alt+F8 teleport, science, aero GUI | `KSP.game/CheatsMenu.cs` | none |
| VAB: parts picker | categories, filters, sort, search, favourites | `KSP.OAB/AssemblyPartsPicker.cs`, `AssemblyBuilderPartCategory.cs` | none |
| VAB: assembly tools | attach, symmetry, snap, rotate, undo, markers, anchor | `KSP.OAB/ObjectAssemblyBuilder.cs`, `ObjectAssemblyToolbar.cs`, `BuilderSymmetryMode.cs` | none |
| VAB: staging stack | drag-drop stages, dV/TWR per stage | `KSP.UI/OABStagingUIDriver.cs`, `KSP.game/StagingDataProvider.cs`, `StageDataContext.cs` | none |
| VAB: engineer report, dV tool | stats, flaw checkers, situation dV | `KSP.OAB/ObjectAssemblyUIEngineeringReport.cs`, `AssemblyCheck*.cs`, `KSP.game/DeltaVToolManagerUI.cs` | none |
| VAB: PAM, action groups, crew, colour | part settings, group assignment, seats, colours | `KSP.game/PartsManagerCore.cs`, `ActionGroupManager.cs`, `KSP.UI/KerbalManager.cs`, `KSP.OAB/ObjectAssemblyColorPicker.cs` | none |
| VAB: workspace save/load, launch | save/load/merge, launch site, launch | `KSP.OAB/OABSaveDialog.cs`, `OABLoadDialog.cs`, `ObjectAssemblyHeader.cs`, `KSP.game/OABProvider.cs` | none |
| Flight: navball and SAS | markers, 10 SAS modes, SAS/RCS toggles | `KSP.UI.Flight/NavballInstrument.cs`, `KSP.game/TelemetryDataProvider.cs`, `KSP.Sim/AutopilotMode.cs` | none |
| Flight: throttle, altimeter, speed, G, atmosphere | gauges with mode toggles | `KSP.UI.Flight/ThrottleInstrument.cs`, `VerticalSpeedInstrument.cs`, `AtmosphericIndicatorInstrument.cs`, `KSP.game/TelemetryDataProvider.cs` | none |
| Flight: orbital readout, burn timer | Ap/Pe + times, node countdown | `KSP.UI/OrbitalReadoutInstrumentManager.cs`, `KSP.UI.Flight/BurnTimerInstrument.cs` | none |
| Flight: time warp | 10 levels with altitude limits, physics warp, warp-to | `KSP.Sim.impl/TimeWarp.cs`, `PhysicsSettings.cs`, `KSP.UI.Flight/TimeWarpInstrument.cs` | partial (shared warp request; no altitude limits or warp-to) |
| Flight: staging stack and resources | stages, fuel bars, chutes, non-stageable panel, resource manager | `KSP.UI.Flight/FlightStagingStackInstrument.cs`, `NonStageableResourcesUI.cs`, `KSP.UI/ResourceManagerUI.cs` | none |
| Flight: action bar and app bar | action groups, window toggles | `KSP.UI.Flight/ActionBar.cs`, `AppBar.cs`, `KSP.game/VesselDataProvider.cs`, `ViewDataProvider.cs` | none |
| Flight: vessel status, recovery, flight report | situation text, recover, records and log | `KSP.UI.Flight/VesselStatusInstrument.cs`, `FlightReportUIManager.cs` | none |
| Flight: pre-launch Go, launch sequence, revert | Go button, skip option, revert to launch/VAB | `KSP.UI.Flight/GoButtonInstrument.cs`, `KSP.game/GlobalEscapeMenu.cs`, `KSP.Messages/RevertToLaunchMessage.cs` | none |
| Flight: cameras, labels, portraits | AUTO/FREE/ORBITAL/CHASE/LOCKED, F4 labels, IVA portraits | `KSP.Sim/FlightCameraMode.cs`, `KSP.UI.Flight/FlightHUDWorldLabelsInstrument.cs`, `IVAPortraits.cs` | none |
| EVA | kerbal as vessel, movement keys, interact, flag | `KSP.Sim.impl/EVAInputHandler.cs`, `KSP/EVAUtils.cs`, `KSP.UI.Flight/FlagSiteUIManager.cs`, `KSP.game/InteractSystem.cs` | none |
| Map: scene and camera | bodies, orbit patches, icons, focus, zoom | `KSP.Map/Map3DView.cs`, `OrbitRenderer.cs`, `MapCamera.cs`, `Map3DFocusItem.cs` | partial (bodies, orbits, own probe) |
| Map: maneuver nodes | six-handle gizmo, move, trash, warp-to | `KSP.Map/MapManeuverGizmo.cs`, `Map3DManeuvers.cs`, `ManeuverGizmoInputType.cs` | partial (prograde-only panel) |
| Map: trajectory events | Ap/Pe/SOI/closest approach/intersect/collision markers | `KSP.Map/Map3DTrajectoryEvents.cs`, `KSP.UI/TrajectoryEventTooltip.cs` | none |
| Map: object list and info panel | filters, search, favourites, body/vessel data, actions | `KSP.Map/MapUI.cs`, `MapUISelectableEntry.cs`, `TrackingStationObjectTypeFilter.cs` | none |
| Tracking Station | map in TrackingStation mode from KSC | `KSP.game/MapProvider.cs`, `KSP.Map/MapMode.cs`, `KSP.UI/GHTrackingStation.cs` | none |
| Trip planner | dV budget between nodes, launch window warp | `KSP.game/TripPlanner.cs`, `data/ui/TripPlanner.json` | none |
| Mission Control | log, pop-over, track, brief/debrief, submit, triumph | `MissionControlManager.cs`, `KSP.UI/MissionControlMenuController.cs`, `MissionPopover.cs`, `KSP.Game.Missions/MissionTriumphWindow.cs` | none |
| Mission tracker | in-flight objective list | `MissionTracker.cs`, `KSP.Game.Missions/KSP2MissionManager.cs` | none |
| R&D tech tree | pan/zoom tree, tiers, unlock | `KSP.Game.Science/RDCenterUIController.cs`, `TechNodeView.cs`, `KSP.UI/ResearchDevelopmentController.cs`, `data/technodedata/` | none |
| Science and research inventory | experiments, regions, store/transmit/recover reports | `KSP.Game.Science/ScienceManager.cs`, `KSP.UI/ResearchInventoryManagerUI.cs`, `data/scienceexperiments/` | none |
| Training Center and tutorials | groups, lessons on tutorial saves, reset/start | `KSP.UI/TrainingCenterMenuController.cs`, `data/tutorials/` | none |
| FTUE | 21 event-triggered PAIGE hint scripts | `data/firsttimeuserexperience/`, `KSP.Game.Missions/KSP2MissionManager.cs` | none |
| Kerbal roster and agency | names, attributes, portraits, flags, per-player entries | `KSP.game/KerbalRosterManager.cs`, `AgencyManager.cs`, `CampaignPlayerManager.cs` | none |
| Flow: new campaign | menu -> create -> load flow -> KSC (+FTUE) | section 3 F1 | none |
| Flow: build and launch | KSC -> VAB -> launch -> flight | F2 | none |
| Flow: fly to orbit | HUD, staging, warp, circularise | F3 | none (probe is placed, not flown) |
| Flow: maneuver to Mun | node, burn, SOI, capture | F4 | partial (prograde node on own probe) |
| Flow: land, EVA, return, recover | landing, EVA, flag, science, recovery report | F5 | none |
| Flow: mission and rewards | track, complete, submit | F6 | none |
| Flow: unlock tech | R&D unlock, parts appear | F7 | none |
| Flow: save and load | quick, manual, menu | F8 | none |
| Flow: multiplayer join | not shipped; plumbing only | F9 | none (OSE native) |
| Flow: first-time user | FTUE chain across screens | F10 | none |

### 5.1 Not found or not extracted

- **Localization strings.** The code references I2 Localization keys everywhere (`LocalizationManager.GetTranslation("Menu/Main/...")`, `[FTUE/...]`, `Missions/Primary/PM01/Name`, `Science/TechNodes/Names/*`) but `assets/ksp2/data/` holds no string table; `KSP.Localization/` has only font and part helpers. The I2 `LanguageSourceAsset` must live in a ScriptableObject inside a bundle (likely `ui` or `localization`) and should be added to the importer before building menus.
- **Default key bindings** were recovered by parsing the input asset JSON embedded in `KSP.Input/GameInput.cs`; gamepad bindings are there too but are not listed here.
- **Prefab layouts** (where each widget sits on screen, colours, fonts) are in Unity prefabs, not in C#; the decompile gives data bindings and behaviour only. Side-by-side layout comparison needs screenshots of the original.
- **Launch sequence publisher** for `LaunchSequenceInitiatedMessage`/`PrelaunchSequenceInitiatedMessage` was not located in the files read; behaviour inferred from the Go button, `SkipLaunchSequence` and `DefaultThrottleInPrelaunch` settings.
- **AN/DN markers** and the full `Map3DTrajectoryEvents.EventType` tail were not verified beyond `Intersect1B`.
- **Colonies, BAE, ColonyView, delivery routes, PlanetViewer, PhotoMode** exist as code (`KSP.Game.Colonies/`, `GameState.BaseAssemblyEditor`, `KSP.PlanetViewer/`) but have no shipped UI entry; excluded from the build list.
- **Multiplayer screens** do not exist; only replication hooks and hidden menu options (section 3, F9).
