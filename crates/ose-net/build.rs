use std::process::Command;

fn main() {
    let output = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let revision = output(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=OSE_BUILD_REVISION={revision}");
    for path in [
        output(&["rev-parse", "--git-path", "HEAD"]),
        output(&["symbolic-ref", "HEAD"]).and_then(|r| output(&["rev-parse", "--git-path", &r])),
    ]
    .into_iter()
    .flatten()
    {
        println!("cargo:rerun-if-changed={path}");
    }
}
