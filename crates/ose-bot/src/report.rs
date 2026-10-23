//! Step outcomes and the progress report.

use serde::Serialize;
use std::time::Duration;

/// Where a step ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Mode {
    /// Through `ose-server` and the protocol: the real end-to-end path.
    Server,
    /// In the bot's own process over `ose-sim`/`ose-math`: stands in until the server does it.
    Local,
}

/// How a step ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Outcome {
    /// The step did what a player would do, with a one-line measurement.
    Passed(String),
    /// The game lacks the feature; `feature` names what has to exist for the step to run.
    Blocked { feature: String },
    /// This machine lacks data the step needs (the KSP2 install); not a game problem.
    Skipped { reason: String },
    /// The feature exists (or the step is pure math) and the bot could not do it: a bug somewhere.
    Failed { error: String },
}

impl Outcome {
    pub fn passed(detail: impl Into<String>) -> Self {
        Outcome::Passed(detail.into())
    }
    pub fn blocked(feature: impl Into<String>) -> Self {
        Outcome::Blocked {
            feature: feature.into(),
        }
    }
    pub fn skipped(reason: impl Into<String>) -> Self {
        Outcome::Skipped {
            reason: reason.into(),
        }
    }
    pub fn failed(error: impl std::fmt::Display) -> Self {
        Outcome::Failed {
            error: format!("{error:#}"),
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Passed(_) => "PASSED",
            Outcome::Blocked { .. } => "BLOCKED",
            Outcome::Skipped { .. } => "SKIPPED",
            Outcome::Failed { .. } => "FAILED",
        }
    }
    pub fn detail(&self) -> &str {
        match self {
            Outcome::Passed(d) => d,
            Outcome::Blocked { feature } => feature,
            Outcome::Skipped { reason } => reason,
            Outcome::Failed { error } => error,
        }
    }
    pub fn is_failed(&self) -> bool {
        matches!(self, Outcome::Failed { .. })
    }
    pub fn is_passed(&self) -> bool {
        matches!(self, Outcome::Passed(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StepResult {
    pub name: &'static str,
    pub mode: Mode,
    pub outcome: Outcome,
    pub elapsed: Duration,
}

/// Human-readable table plus a one-line summary.
pub fn print_report(results: &[StepResult]) {
    println!("{:<28} {:<7} {:<8} detail", "step", "mode", "outcome");
    for r in results {
        println!(
            "{:<28} {:<7} {:<8} {}",
            r.name,
            format!("{:?}", r.mode).to_lowercase(),
            r.outcome.label(),
            r.outcome.detail()
        );
    }
    let passed = results.iter().filter(|r| r.outcome.is_passed()).count();
    let failed = results.iter().filter(|r| r.outcome.is_failed()).count();
    let blocked = results
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::Blocked { .. }))
        .count();
    let furthest = results
        .iter()
        .rev()
        .find(|r| r.outcome.is_passed())
        .map(|r| r.name)
        .unwrap_or("nothing");
    println!(
        "\n{passed} passed, {blocked} blocked, {failed} failed out of {}; furthest step passed: {furthest}",
        results.len()
    );
}
