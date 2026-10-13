# Channel Pan / Stereo Balance — Qualification Record

Branch: `feat/channel-pan-balance` (worktree `.worktrees/channel-pan`)
Base: `9ba4679` · Final: `802fbbf`
Spec: `docs/superpowers/specs/2026-10-12-channel-pan-design.md`
Plan: `docs/superpowers/plans/2026-10-12-channel-pan-implementation.md`

This record states what was verified, how, and what remains unverified. It does not claim
physical-phone or hearing results.

## 1. Delivered

- Per-source pan/balance in the personal mix (`SourceGain.pan`, normalized `[-1,1]`).
- Mono attenuation-only pan law; explicit linked-pair stereo balance with no crossfeed, independent
  member gain/mute and one shared balance.
- Explicit desktop L/R stereo-pair linking and per-source saved defaults for **new** personal mixes.
- Durable, device-bound `ConfigV2` persistence with transactional single-write saves.
- Operator IPC: `read_source_settings`, `save_source_defaults`, `save_source_links`, guarded by the
  actual injected caller window and a local, operator-only capability.
- One shared pure `initialMix(catalog)` seeding both phone paths.
- Phone grouping UI and desktop "Defaults for new personal mixes" UI.
- Browser buffer-delay diagnostics (support/request status, target/minimum delay, packet loss,
  concealment) alongside the existing RTT and buffer readings.

## 2. Verification (final state)

| Gate | Command | Result |
|---|---|---|
| Rust workspace tests | `cargo test --workspace --locked` | **296 passed, 1 ignored** |
| Rust lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean |
| Web unit tests | `npm test` | **300 passed / 28 files** |
| Web types | `npm run typecheck` | clean |
| Web build | `npm run build:web` | admin + musician built |
| Whitespace | `git diff --check` | clean |
| Wired browser (phone) | `node src/ui/__tests__/panReceiver.browser.mjs` | Chromium + WebKit PASS |
| Wired browser (admin) | `node src/admin/__tests__/SourcesPan.browser.mjs` | Chromium + WebKit PASS |
| Wired browser (buffer) | `node src/ui/__tests__/LatencyDiagnostics.browser.mjs` | Chromium + WebKit PASS |
| Wired browser (mix) | `node src/musician/__tests__/mix.browser.mjs` | PASS |

Named integration evidence:
- `crates/host-core/tests/pan_pipeline.rs` — accepted linked pair → real `Pipeline`
  (`AudioEngine` + `OpusWorker`), pre-encode PCM via the public `MonitorPort`; controlled
  `PAN_PIPELINE_MUTATION=omit_pan|reset_identity` mutations shown red then restored.
- `crates/host-core/src/audio/tests.rs` — analytic per-sample pan coefficients, 240-frame travel
  (239/240 endpoints), no-crossfeed, independent mute, identity reset.
- `crates/host-core/src/config/tests.rs` — durability fault stages, migration, binding immutability,
  layout validation, uncertain-state reconciliation.

## 3. Cross-checks confirmed at final state

- Pan is canonicalized in the control actor (`control/actor.rs:219` via `canonicalize_pan`) before
  reaching `pan_coefficients`; the DSP helper consumes a canonical value and does not validate.
- Percentage↔normalized conversion exists only inside `PanControl` (`PanControl.tsx:33,39`).
- Unpaired legacy `MasterLr` keeps `physical_index % 2` parity routing and gets no pan control
  (`audio/engine/routing.rs:164-168`); a paired `MasterLr` is grouped.
- Admin defaults save sends the complete source vector and only calls the settings APIs — it never
  installs a live mix or broadcasts.

## 4. Deferred / not yet verified

Deferred minors (documented, non-blocking; see ledger):
- Direct public `process_block` on a context change retains limiter history (pre-existing); the normal
  `Pipeline::register` path rebuilds state.
- `commands.rs:277` `unwrap_or(0)` masks an unreachable missing-`input_channels` invariant.
- `source_catalog_impl` transitively mints a pending preview on read (read-safe side effect).
- `StartHostResult.host_epoch` is a client-minted UUID (pre-existing, outside the source-settings contract).
- Phone UI minors: invalid-pair live readout value, no unpaired `InputChannel` in the browser harness,
  "Mono source" copy, pair readout prefers the left member.
- Task 7 minors: `findLinkablePair` straddle selection; link-vs-default save serialization relies on the
  native revision guard.
- Durable-save `DurabilityUncertain` is covered by core fault-injection tests and typed-error mapping,
  not by a live desktop filesystem failure.

**Physical verification still owed (not performed here):**
- Actual iPhone loudness and perceived balance/pan placement.
- Whether the browser buffer delay (e.g. ~152 ms) changes with the new diagnostics on real hardware;
  the displayed request "Accepted" is not proof of a reduced actual buffer.
- Capture dropout/skipping behavior and Wi-Fi/browser playout on real devices.
- Battery/thermal and wake-lock behavior over a real session.

Browser fixtures are fixed test values; headless WebKit is not a physical iPhone.

## 5. Test-on-device plan (next step for the operator)

1. Rebuild the desktop with embedded assets: `npm run build:web` then `npm run dev:desktop`.
2. **Audio is still ~21 dB louder than the old build.** Mute playback, lower phone and app volume,
   remove earphones initially, then raise slowly.
3. Join from the iPhone browser (wired output only), press Start listening.
4. Verify: mono channels pan L/C/R; a linked pair moves together with one balance; changing a desktop
   default pan does **not** change an already-running phone mix; a new session picks up the saved default.
5. Watch the buffer diagnostics panel and record RTT, browser buffer delay, target/minimum, loss, and
   concealment before/after any change.

Do not repeat an everything-at-maximum test; the digital limiter is not a hearing-safety guarantee.
