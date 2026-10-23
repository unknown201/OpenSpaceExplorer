//! End-to-end bot for OpenSpaceExplorer.
//!
//! The bot plays the campaign the way a player would, through `ose-server` and the `ose-net` protocol:
//! start a campaign, build and launch a stock craft, reach Kerbin orbit, transfer to the Mun, land. Most of
//! those systems do not exist yet, so every step reports one of four [`Outcome`]s: `Passed`, `Blocked` (the
//! game lacks the feature, with the missing feature named), `Skipped` (this machine lacks data) or `Failed`
//! (the feature exists and the bot could not do it). A `Blocked` step is expected; a `Failed` one is a bug.
//!
//! Steps that only need orbital mechanics run against the bot's own in-process copy of `ose-sim` ("local"
//! steps) so the Mun run is exercised end to end on rails today; when the server grows the matching
//! protocol they move to the server. See `docs/ADR-0003-e2e-bot.md`.

#![forbid(unsafe_code)]

pub mod client;
pub mod data;
pub mod harness;
pub mod plan;
pub mod rails;
pub mod report;
pub mod steps;

pub use report::{print_report, Mode, Outcome, StepResult};
pub use steps::{run_mun_campaign, BotConfig, STEP_NAMES};
