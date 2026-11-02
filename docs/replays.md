# Replays: filing, reproducing and fixing bugs

The server records what happens so a bug can be filed with the flight that showed it, run again on a
developer's machine, and kept as a regression test.

## What is recorded

`crates/ose-server/src/replay.rs` keeps rolling five-minute windows. Each window starts with a baseline: the
save file of the world at that moment, the craft library, the revert point and who is connected. After that it
logs everything the players did (every client message, plus joins and leaves) with the simulation step it
arrived after, and a checkpoint of every vessel once a second (position, velocity, stage, parts, propellant).

A filed replay holds the last one or two windows, so between five and ten minutes of play.

## Filing

In the Game Menu (Esc), **Report a bug** asks for a note and files the replay. The server writes it to
`replays/<id>.json` in its working directory (`/var/lib/ose/replays/` on the container), and the menu links
the file for download.

- `GET /api/replays` lists filed replays (id, note, who, length).
- `GET /api/replays/<id>` downloads one.

## Reproducing

```sh
cargo run -p ose-server -- --replay path/to/<id>.json --replay-out report.json
```

This starts the server's world from the replay's baseline (nothing is served or saved), applies each recorded
action after the same number of steps it arrived after live, and steps the simulation as fast as it goes
(seconds for a few minutes of play). Live, actions are applied between steps under the world's locks, so the
same code on the same inputs reproduces the flight. `report.json` shows:

- `divergences`: checkpoints where the replay differs from the recording (a vessel missing or extra, the stage
  or part count different, a position more than 1 m off). An empty list means the run reproduced the flight;
  after a fix, the divergences show where the fixed behaviour departs from the bug.
- `max_position_error_m`, `refusals` (requests the server turned down, with reasons), `final_vessels`.

Determinism holds as long as every change to the world happens inside the step (`step_once`) or in a recorded
action. Code that changes the world on a wall-clock timer breaks it: the vessel registry is kept up on the step
for that reason (`maintain_registry`).

## Turning a report into a test

1. Copy the file to `crates/ose-server/tests/replays/<what-it-shows>.json` and rewrite `note` to say what
   happens in it.
2. Add an `expect` list stating what must be true at the end once the bug is fixed:

   ```json
   "expect": [
     {"vessel": "Kerbal K1", "field": "parts", "op": "eq", "value": 26, "why": "the boosters came off"},
     {"vessel": "Kerbal K1 debris", "field": "count", "op": "eq", "value": 4}
   ]
   ```

   Fields: `stage`, `propellant_t`, `parts`, `landed`, `crashed` (1 or 0), `altitude_m` (above Kerbin's
   radius), `speed_mps`, `count` (vessels with that name). Ops: `eq`, `ne`, `lt`, `le`, `gt`, `ge`.
3. `cargo test -p ose-server --test replays` runs every fixture and fails, naming the expectation, while
   the bug is there.

A replay keeps the protocol version it was recorded with (`protocol`); a protocol change that alters messages
can make old fixtures unreadable, and they are re-recorded then.
