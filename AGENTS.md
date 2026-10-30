# Working on OpenSpaceExplorer with several agents at once

Several threads (and Josh's other agent) work on this repository at the same time, each in its own git
worktree under `D:\Projects\ose-*`. Those worktrees share one `.git`, so every ref, including
`origin/main`, is shared: another thread's `git fetch` moves `origin/main` under you in the middle of a
command. Twice on 2026-10-30 a squash built "onto origin/main" by name captured a base that had just
moved and silently reverted other threads' commits. These rules keep that from happening again.

## Branches and bases

- Work on your own branch in your own worktree: `git worktree add -b <branch> D:\Projects\ose-<name> origin/main`.
  Never switch the branch of the main checkout (`D:\Projects\OpenSpaceExplorer`) or of another thread's worktree.
- Pin your base to a commit, never to the name `origin/main`:
  `BASE=$(git fetch -q origin main && git rev-parse origin/main)`, then `git rebase "$BASE"`.
  Do not use `git reset --soft origin/main`, `git merge origin/main` by name, or a squash against the name.
- Before you push, check that your diff against the pinned base contains only your own files:
  `git diff "$BASE" --stat`. A file you never touched in that list means your base moved; stop and rebase again.
- Rebase and push promptly. A branch that sits for hours collects conflicts with everyone else's merges.

## Protocol numbers

`PROTOCOL_VERSION` in `crates/ose-net/src/lib.rs` is bumped by several threads. Take main's current
number plus one at the moment you rebase for the merge, not when you start the work, and keep every
mention of it (docs, comments, `docs/STATUS.md`) in step.

## Merging to main

`main` is protected: changes land through a pull request with the `check` and `browser-e2e` jobs of
`.github/workflows/ci.yml` passing and the branch up to date with main. Every push to main deploys to
ose.jfly.dev (`.github/workflows/deploy.yml`), so a merge is a release. Rebase onto the pinned base
right before merging, verify the diff as above, and let CI run on the rebased commit.

## Builds and tests

- Share the build cache across worktrees with `CARGO_TARGET_DIR=D:/Projects/OpenSpaceExplorer/target`;
  cargo serialises builds with its lock. A stale wasm build of a crate can fail after a rebase; `cargo clean -p <crate> --target wasm32-unknown-unknown` fixes it.
- `assets/web` and `e2e/node_modules` live only in the main checkout; serve assets with `--assets-dir D:/Projects/OpenSpaceExplorer/assets/web`.
- Before pushing run what CI runs: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `python -B -m unittest discover -s tools/tests`, `cargo check -p ose-client --target wasm32-unknown-unknown`.
- A local `ose-server` left running on a port keeps serving old code and holds `target/debug/ose-server.exe`;
  check `netstat -ano | findstr 779` before benchmarking, and kill only your own process by PID.
