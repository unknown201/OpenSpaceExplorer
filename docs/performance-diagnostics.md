# Diagnosing client stalls

The client automatically posts compact performance summaries to `/api/diagnostics` every 30 seconds
while in a game. It also posts after a visible frame gap over 250 ms, a callback/message/input delay
over 100 ms, or a vessel-update gap over one second. Stall reports have a ten-second cooldown.
Hidden-tab time is excluded. Collection uses constant space; it does not retain frame histories.

**Settings > Graphics > Report performance issue** sends a report immediately and displays its ID.
The most recent payload is also available as `document.body.dataset.oseDiagnostics` if transport fails.
The payload includes an opaque per-page session ID, sequence, build commit, protocol, scene, vessel IDs,
warp, viewport size, WebGPU status, optional JS heap size, WebSocket backlog, and timing counters.
It includes no account tokens, emails, display names, craft contents, input keys, or screenshots.
Tokens are used only in the Authorization header; the server never logs that header.

Reports require a valid account session token, reject unknown/invalid fields, have an 8 KB body limit,
and are limited to 12 per minute per account. HTTP acknowledgments carry the report ID so an old
server's static-page fallback cannot be mistaken for successful delivery. Logs identify both client
and server build commits without changing the gameplay protocol.

## Server logs

Keep `RUST_LOG=info` (the default). The systemd journal retains the structured JSON logs:

```sh
journalctl -u ose --since '15 minutes ago' -o cat | jq -c 'select(.fields.message | test("performance|publication slow"))'
```

Relevant messages:

- `client performance report`: report ID, authenticated player ID, room, both builds and client metrics.
- `server performance summary`: each room's 30-second count/total/max/slow counters per phase.
- `server performance stall`: a phase crossed its threshold; capped at one warning per phase per five seconds.
- `websocket publication slow or failed`: a publication took over 100 ms or failed/timed out; limited per player.
- `server diagnostics ready`: build/protocol at startup.

Server phases cover simulation work and wake-up lag, command lock wait and total handling time,
snapshot/trajectory publication, and save capture/write. Telemetry only tries to read a room ID; it
never waits for the campaign mutex for logging. Report handling does not acquire the simulation mutex.

## Interpreting evidence

Large frame/callback/input gaps with normal server timings point to the client main thread. Large
message handling times or payload sizes point to decode/application work. Normal rendered frames
with old vessel states point to delayed network/server updates; correlate the room/player and log time
with simulation, publication, command-wait and WebSocket warnings. A low normalized server rate or
large clock/motion corrections can explain jumpiness even when render FPS stays high.

Counters are observations, not automatic root-cause classification. JS heap excludes native/GPU memory;
HTTP dispatch is not a measurement of authoritative control acknowledgment. Room IDs may be absent
in a warning when their mutex is busy; player IDs and timestamps still permit correlation.

Validation includes native window/cooldown/visibility tests, payload/auth/rate-limit HTTP tests, and
browser tests that deliberately stall the main thread and verify an automatic authenticated report.
