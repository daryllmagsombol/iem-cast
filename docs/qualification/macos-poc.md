# macOS POC qualification record

> Status: POC in progress — **not stage qualified**. Software checks in this record are separate
> from physical audio qualification. No hardware/electrical latency claim is made here.

## 1. Prerequisite and approval gate (plan Task 0)

Detected environment (inspected):

| Tool | State |
| --- | --- |
| Node | v24.20.0 (>= 24.15 floor) |
| npm | 11.19.0 |
| macOS | 27.0, arm64 |
| Xcode CLT | `/Library/Developer/CommandLineTools` |
| Rust (rustup) | 1.99.0 installed after explicit operator approval |
| CMake | 4.4.4 installed after explicit operator approval |
| mkcert | 1.4.4 installed after explicit operator approval |

Approvals recorded:

- **Toolchain install: approved by operator** and completed. No install is automatic; a future
  machine must repeat this gate.
- **mkcert root-CA trust: NOT installed.** The root CA must never be trusted automatically. Trusting
  the local CA on the test phones is a separate, manual operator action (see §5). `rootCA-key.pem`
  must never be committed or transferred to a phone.
- Repository had no `HEAD` at start; no initial commit, worktree, or git-config change was created
  for isolation. Lanes used disjoint file ownership instead.

## 2. Automated software verification (run on this machine)

These results are **software** evidence only. They do not prove real Soundcraft capture, real phone
browsers, Wi-Fi behavior, or end-to-end latency.

| Check | Command | Result |
| --- | --- | --- |
| Rust contract serde | `cargo test -p host-core --test contract_serde` | PASS |
| Rust host-core suite | `cargo test -p host-core` | PASS (see below) |
| Rust lints | `cargo clippy -p host-core --all-targets -- -D warnings` | PASS, no issues |
| Frontend typecheck | `npm run typecheck --workspace apps/web` | PASS |
| Frontend unit tests | `npm run test --workspace apps/web` | PASS (14 files, 44 tests) |
| Admin bundle | `npm run build:admin --workspace apps/web` | PASS |
| Musician bundle | `npm run build:musician --workspace apps/web` | PASS |
| Static site bundle | `npm run build:site --workspace apps/web` | PASS |
| Browser gate (Chromium) | `npx playwright test --project=chromium` | PASS (5 tests) |
| Browser gate (WebKit) | `npx playwright test --project=webkit` | PASS (5 tests) |

Record the exact `cargo test -p host-core` counts here when the transport lane lands and the full
suite is re-run in §3.

## 3. Integration suite (plan Task 9)

Pending final run after all lanes are reconciled:

- `cargo test -p host-core` (all modules, including transport + the `poc_flow` integration test).
- `cargo clippy -p host-core --all-targets -- -D warnings`.
- `npm run typecheck`, `npm run test`, `npm run build:web`.
- `npx playwright test --project=chromium --project=webkit` (mocked; **not** fulfillment of the
  real wireless requirement; emulated WebKit is not an iPhone).

## 4. Manual real-hardware qualification (plan Task 10 — blocked until hardware is present)

Not performed. Requires, and cannot be faked:

- A real **Soundcraft Signature 22 MTK** connected over USB on this Mac.
- Physical **iPhone Safari** and **Android Chrome** devices with **wired** earphones.
- A Wi-Fi or Ethernet AP on the selected LAN.
- The operator to have trusted the local certificate on each real device (§5).

Steps to perform (record actual observations, never inferred passes):

1. **One-phone real USB path:** operator selects the actual device, verifies USB channel mapping,
   labels sources, joins one phone, and listens. Real captured audio must leave the phone output;
   page visible and screen on. Verify L/R mapping manually — driver labels are not proof.
2. **Two independent mixes:** two phones, different personal gains/mutes; each hears its own mix;
   both may listen to the same shared source with independent settings; silence on disconnect;
   explicit re-arm after interruption.
3. **Safe failure and re-arm:** exercise Stop, disconnect/reconnect, device loss, and output-route
   change. Expect immediate local silence, no backlog replay, and no auto-unmute/auto-arm. Stop
   invokes an immediate JS local mute — this is **not** an instant analog-silence promise. A
   **detectable** route change requires re-arm; if not detected, record it as unsupported.
4. **Physical latency measurement (procedure only):** use a **matched ADC on the same capture
   clock** — a mono impulse on channel 1 as the analog reference and the **single chosen headphone
   channel L** on channel 2, recorded simultaneously; repeat on R separately. Do not assume a
   currently owned lab ADC. Report median, p95, p99, worst excursion, and outage tracks. **Agree
   the pass statistics before qualifying the ≤10 ms goal.** Until then, the ≤10 ms target is
   unproven.

## 4b. No-mixer smoke test with BlackHole (software path only)

The mixer is not required to exercise the capture → mix → encode → WebRTC → phone path. A virtual
loopback device gives a **stereo** system-audio source, which is enough to prove the pipeline runs
end to end before the real console arrives.

Setup (operator action):

1. Install the loopback driver: `brew install blackhole-2ch` (approves an admin prompt and briefly
   restarts Core Audio). `BlackHole 2ch` then appears as both an output and an input.
2. In **Audio MIDI Setup**, create a **Multi-Output Device** containing your speakers/headphones
   first and `BlackHole 2ch` second. Set macOS **Sound → Output** to that device, so you still hear
   the Mac while BlackHole receives a copy.
3. Set `BlackHole 2ch` to **48 kHz**. The app rejects a device that does not report 48 kHz.
4. In the app, select **BlackHole 2ch** as the capture device.

Honest limits of this test:

- **Stereo only.** BlackHole 2ch exposes two channels, so every phone hears the same stereo feed.
  It proves the transport, not per-instrument personal mixing.
- It does **not** exercise the Soundcraft USB channel map or the 22 individual post-gain/pre-EQ
  sources; only the real mixer can do that.
- Selecting `BlackHole 2ch` as the macOS output directly (rather than via a Multi-Output Device)
  silences the Mac — expected, not a fault.

## 5. Certificate and trust steps (manual operator action)

- Generate a certificate whose SAN matches the **selected LAN interface** IP or resolvable hostname.
  `mkcert -cert-file local-certs/cert.pem -key-file local-certs/key.pem "$LAN_IP" localhost 127.0.0.1 ::1`
- Transfer **only the public root certificate** (`mkcert -CAROOT` → `rootCA.pem`) to test devices.
  Never transfer or commit `rootCA-key.pem`.
- **iOS:** open the CA file so the profile downloads, install it under
  **Settings → General → VPN & Device Management**, then enable **full trust** under
  **Settings → General → About → Certificate Trust Settings**. Skipping the final step leaves HTTPS
  untrusted.
- **Android:** install it as a **CA certificate** under Security/Encryption & credentials, then
  **verify in Chrome on the real device**. The settings path varies by manufacturer.
- Browser trust requires the visited URL to match a certificate SAN; hitting the host by a name the
  certificate does not cover still warns even when the CA is trusted.
- If the selected interface changes, **reject the mismatched certificate** and require operator
  reconfiguration. Never bypass certificate warnings and never auto-trust a CA.

## 6. Honest status

- Automated software gates above: passing as recorded.
- Live host startup, real pairing credentials, the browser receiver, and the local monitor are
  implemented and unit-tested, but the full analog path has **not** been exercised on hardware.
- Real audio from the mixer, real phones, real AP, real device trust, and physical latency: **not yet
  attempted**. No stage or qualification claim is made.
- If hardware is unavailable, this task is recorded as **blocked** — never a fabricated pass. The
  BlackHole smoke test (§4b) is a software-path check only and never substitutes for it.
