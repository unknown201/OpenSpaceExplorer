//! Every filed replay kept under `tests/replays/` runs through `ose-server --replay` and its `expect` list must
//! hold: a bug report becomes a regression test by adding the expectations its fix makes true (see
//! `docs/replays.md`).

use std::path::PathBuf;
use std::process::Command;

fn fixtures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/replays");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn filed_replays_hold_their_expectations() {
    let all = fixtures();
    assert!(!all.is_empty(), "no fixtures under tests/replays");
    let tmp = std::env::temp_dir().join(format!("ose-replay-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let mut failed = Vec::new();
    for path in &all {
        let out = tmp.join("report.json");
        let status = Command::new(env!("CARGO_BIN_EXE_ose-server"))
            .current_dir(&tmp)
            .arg("--replay")
            .arg(path)
            .arg("--replay-out")
            .arg(&out)
            .env("RUST_LOG", "warn")
            .status()
            .expect("run ose-server --replay");
        assert!(status.success(), "{} did not run", path.display());
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        let expectations = report["expectations_failed"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if report["ok"] != true || !expectations.is_empty() {
            failed.push(format!(
                "{}: {:?} (divergences from the recording: {})",
                path.display(),
                expectations,
                report["divergences"]
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(failed.is_empty(), "{failed:#?}");
}
