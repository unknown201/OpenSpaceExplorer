//! The Mun run as an automated test, against a freshly spawned `ose-server`.
//!
//! `FRONTIER` records how far the game lets the bot get today. The test fails when any step FAILS (a feature
//! exists and the bot cannot use it, or the orbital mechanics are wrong) or when a step expected to pass is
//! now blocked or skipped (a regression). A step expected to be blocked that now PASSES is progress: it is printed loudly but does not fail, so the
//! thread adding the feature is not punished; move the frontier forward in the same change.

use ose_bot::{run_mun_campaign, BotConfig, Outcome, STEP_NAMES};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// Must pass.
    Pass,
    /// Must pass, or be skipped when this machine lacks the KSP2 data.
    PassOrSkip,
    /// The feature does not exist yet; blocked is the expected state, passing is progress.
    Blocked,
}

const FRONTIER: [(&str, Expect); 11] = [
    ("server_health", Expect::Pass),
    ("connect_hello", Expect::Pass),
    ("start_campaign", Expect::Blocked),
    ("load_stock_craft", Expect::PassOrSkip),
    ("upload_and_launch_craft", Expect::PassOrSkip),
    ("ascend_to_orbit", Expect::Blocked),
    ("plan_mun_transfer", Expect::Pass),
    ("execute_transfer_burn", Expect::Pass),
    ("coast_to_mun_encounter", Expect::Pass),
    ("mun_capture_burn", Expect::Pass),
    ("mun_landing", Expect::Blocked),
];

#[test]
fn frontier_names_every_step_in_order() {
    let names: Vec<&str> = FRONTIER.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, STEP_NAMES.to_vec());
}

#[tokio::test]
async fn bot_plays_the_mun_run_as_far_as_the_game_allows() {
    let server = ose_bot::harness::spawn_server(Duration::from_secs(600))
        .await
        .expect("spawn ose-server");
    let cfg = BotConfig {
        server: Some(server.addr.clone()),
        ..BotConfig::default()
    };
    let results = run_mun_campaign(&cfg).await;
    ose_bot::print_report(&results);

    let mut problems = Vec::new();
    let mut progress = Vec::new();
    for (r, (name, expect)) in results.iter().zip(FRONTIER) {
        assert_eq!(r.name, name);
        match (expect, &r.outcome) {
            (_, Outcome::Failed { error }) => problems.push(format!("{name} FAILED: {error}")),
            (Expect::Pass, Outcome::Passed(_)) => {}
            (Expect::Pass, other) => problems.push(format!(
                "{name} regressed: {} {}",
                other.label(),
                other.detail()
            )),
            (Expect::PassOrSkip, Outcome::Passed(_) | Outcome::Skipped { .. }) => {}
            (Expect::PassOrSkip, other) => problems.push(format!(
                "{name} regressed: {} {}",
                other.label(),
                other.detail()
            )),
            (Expect::Blocked, Outcome::Blocked { .. }) => {}
            (Expect::Blocked, Outcome::Passed(d)) => {
                progress.push(format!("{name} now passes: {d}"))
            }
            (Expect::Blocked, Outcome::Skipped { reason }) => {
                problems.push(format!("{name} skipped instead of blocked: {reason}"))
            }
        }
    }
    for p in &progress {
        println!("PROGRESS: {p} (move the frontier in tests/mun_run.rs)");
    }
    assert!(
        problems.is_empty(),
        "Mun run problems:\n{}",
        problems.join("\n")
    );
}
