# OTP sign-in stops responding

Reported on 2026-11-14: the sign-in button looks active but clicking has no effect.
The production source was pinned to `97897eb9e70659a0291d2c06b27f13f91f437fe7`.
An ordinary initialized Chromium session reached the public WebSocket server and
displayed validation errors correctly. No OTP delivery failures appeared in the
service logs. The exact state of the reporting browser was unavailable.

## Reproductions and corrections

| Condition | Before | Fixed |
| --- | --- | --- |
| Client download is delayed | The HTML sign-in button is disabled until WASM starts; no loading explanation. | The first click is queued with a visible loading message and submitted once when the client is ready. |
| Client cannot load | The button can remain disabled indefinitely. | A loading failure message and Reload sign-in action appear after 30 seconds. |
| WebSocket handshake stalls | The active button sets `login_requested`; later clicks return early indefinitely. | The form shows connection progress, the handshake times out after 15 seconds, and the next click opens a fresh socket. |
| Open socket receives no reply | The pending sign-in keeps ignoring later clicks. | A 20-second request deadline releases the pending state and offers a fresh connection. |
| Old socket closes while a retry begins | An existing closing/closed socket can prevent reconnection. | Only an open or connecting socket is reused; old callbacks are detached before replacement. |

The first timeout regression also demonstrated that calling `close()` during a
pending handshake does not reliably deliver `onclose` promptly. The deadline must
clear the socket and pending request itself. Request deadlines are tagged by attempt
so an older deadline cannot cancel a newer attempt.

## Verification

- `e2e/tests/startup_connection.spec.ts`: eight tests passed in Chromium and Firefox,
  including delayed WASM, failed WASM, stalled handshakes, missing replies, duplicate
  clicks, and reconnecting a remembered account. Playwright's virtual clock advances
  deadlines; WebSocket routes hold the actual transport or server responses.
- The existing SMTP-backed browser script passed against the rebuilt client and
  an isolated server: legacy account linking, email sign-in, campaign invitations,
  and recovery of both new and legacy accounts from fresh browser contexts. Its local
  runner used protocol 69 and `target/otp-login-web`. All email stayed in a local SMTP sink.
- The server's `campaign_access` email OTP integration test passed.
- Workspace Clippy, Rust formatting, the WASM target check, and all 44 Python tests passed.

The server protocol, account records, OTP validity, and mail relay configuration do
not change. The browser must load the updated HTML, bootstrap script, and WASM client.
The SMTP sink establishes the sign-in flow without claiming external inbox delivery.
