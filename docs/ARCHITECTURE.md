# IEM Cast architecture

**DRAFT — proposed implementation contract, pending user review.** This document proposes an
internal implementation design for the product requirements already approved in
[README.md](../README.md). It does not change approved product scope. Proposed tuning defaults
(queue sizes, timeouts, gains, bitrate and frame choices) are **qualification inputs** to be
validated or replaced by measurement — **not performance promises**. Source facts and agreed
product requirements retain their stated status. No implementation, build, runtime, or hardware
test exists yet. UI/token contracts are owned by [DESIGN-SYSTEM.md](DESIGN-SYSTEM.md); this
document does not redefine them.

## 1. Scope and non-goals

Capture is **input-only** from the Soundcraft Signature 22 MTK over USB: individual channels
1–22 post-gain/pre-EQ, plus master L/R 23/24. Master is a **separate** source and is not summed
with its constituents. There is **no USB RTN** return path to the mixer. Initial capture rate is
48 kHz only when the actual device verifies it; unsupported rates are **rejected, never silently
resampled**. All streamed personal mixes are **stereo**; mono sources are placed centered into
that stereo bus (see §8). Support 5–8 musicians then 13. Receivers are active browsers (Safari on
iPhone, Chrome on Android) with wired earphones only. Host network is Ethernet **or** Wi-Fi. The
full analog-input-to-wired-headphone electrical target is ≤10 ms as an engineering goal, gated by
percentile/outage criteria that must be agreed before probing. Native receivers are a separately
qualified fallback with no capability promise.

Out of scope for MVP: EQ, effects, talkback, recording, SFU/cloud, video, wireless buds,
audio worklets used solely as a mute path, plugin frameworks.

### 1.1 First reusable POC — macOS

The first POC is a **reusable application foundation**, not disposable test code. It proves the
real audio path while establishing the module boundaries later work will extend.

- **Rust host-core** with reusable **capture / DSP / control / media** boundaries.
- **Minimal Tauri operator window:** selects the **actual Soundcraft input device** via CoreAudio,
  verifies USB channel mapping, lets the operator choose and label available sources, and
  starts/stops the local server on the selected LAN interface.
- **React + TypeScript phone controls with Tailwind** (Tailwind is the selected styling approach;
  the design system has already been updated): channel gain, channel mute, personal master
  attenuation, master mute, **Start listening**, and **local immediate Stop**.
- **Real USB multichannel → host → independent stereo WebRTC stream per receiver.**
- **Audio-path qualification order:** one phone first, then **2 independent listener sessions**.
- Targets **iPhone Safari and Android Chrome** with wired outputs, page visible and screen on.
- **Local trusted HTTPS, pairing, and re-arm gates** from the existing security/safety design are
  required — not stubbed for the test.
- Device USB mapping only; **no RTN**.
- Host over **Ethernet or Wi-Fi** on the selected interface, per the existing network contract.
- Source-core capture targets **macOS first** and preserves **Windows/Linux backend seams**, without
  any claim that other OS targets are tested.

**Synthetic audio** is permitted **only** as an explicit diagnostic mode and **never counts as the
real Soundcraft working**. The POC starts muted with bounded gains, smoothing, and a limiter, and
makes **no ear-SPL guarantee**; the actual **≤10 ms electrical target still needs measurement**.
The POC must prove capture, browser output, **independent mix isolation**, and **safe failure**
before it is treated as qualified.

**Diagnostics are included but do not measure end-to-end audio delay** — Network RTT and Browser
buffer delay are separate WebRTC statistics, not full-path measurements (see §17.1).

**Deferred beyond the POC** (future goals, not removed): 8/13-client qualification, saved presets,
polished full shell, installers/release CI, native apps, and full other-OS support. The POC is
limited to **2 active receivers**, a bound **separate from** the future production default of 8.

## 2. Repository layout

```
crates/
  host-core/            # one crate, internal modules by responsibility
    src/capture/        # device enumeration, CPAL candidate, capture pool
    src/audio/          # DSP gain chain, ramps, limiter, stereo mapping
    src/encoder/        # per-listener Opus encoder workers
    src/transport/      # WebRTC/session transport candidate adapters
    src/control/        # protocol envelopes, revisions, ack/apply
    src/server/         # HTTPS/WSS app server, TLS identity, pairing
    src/config/         # versioned persistence, presets
    src/diagnostics/    # off-thread telemetry accumulators
apps/
  desktop/src-tauri/    # privileged operator shell; narrow IPC only
  web/src/admin/        # operator React+TS UI (bundled)
  web/src/musician/     # public musician receiver page (bundled)
  web/src/protocol/     # shared message types, revisions, envelope guards
docs/                   # architecture, design system, qualification records
```

One desktop Rust process owns capture, DSP, encoding, transport, and server. Tauri exposes a
**privileged, narrowly scoped IPC** surface used only by bundled operator code. The LAN browser
musician API is a public endpoint but is **authenticated and musician-scoped**: it is not
anonymously accessible, grants no privileged operations, and can never address another musician's
ID. Node is **build tooling only**; there is no required Node runtime audio server. We deliberately
avoid a crate per tiny responsibility and any plugin framework.

## 3. Trust and process model

| Surface | Reachable by | Privilege |
| --- | --- | --- |
| Bundled admin UI/assets | Local desktop webview | Validated Tauri IPC; no LAN admin routes in MVP |
| Tauri IPC | Bundled desktop frontend only | Admin: setup, interface choice, cert enrollment, narrow commands |
| Musician HTTPS/WSS API | Authenticated, musician-scoped LAN browser | Musician-scoped only; no privileged operations; no other IDs |

Authorization determines the calling musician; a client can never target another musician's ID.
Admin visibility is unavailable to musician sessions rather than merely hidden.

## 4. Identity and epoch model

| Identity | Created when | Kind | Invalidates |
| --- | --- | --- | --- |
| `hostEpoch` | Random per host process restart | Opaque random string | All prior control state |
| `audioEpoch` | New capture start, mapping change, rate change, or unknowable gap | Opaque random string | All prior audio/timeline references |
| `sessionEpoch` | Per replaced listener/connection | Opaque random string | That listener's prior messages |
| `catalogRevision` | Source catalog/label/pairing change | Monotonic u64, decimal string on the wire | Stale catalog references |
| `mixRevision` | Accepted mix change | Monotonic u64, decimal string on the wire | Stale mix references |
| `safetyGeneration` | Every arm/disarm and emergency | Monotonic u64, decimal string on the wire | Queued edits/acks that could re-open listening |

`hostEpoch`, `audioEpoch`, and `sessionEpoch` are **opaque random identifiers** — they carry no
numeric meaning and need not be decimal. `requestId` is an opaque, unique per-client string.
`catalogRevision`, `mixRevision`, `safetyGeneration`, and `startSample` are u64 values whose
**wire representation is decimal-only** so they remain exact in JavaScript. `frameCount` is
native u32, serialized as a decimal string constrained to the u32 range. Inside
the host, the capture block uses a **native u64** `startSample`; the string notation describes
external serialization only, never internal host allocations. Counters are monotonic **within
their governing epoch**. Messages with stale epochs are rejected. Authenticated `listen.disarm`
requests for the current session are **not rejected solely because their safety generation
predates an outstanding arm**. They cancel the applicable arm attempt, close the host safety
gate, and advance the host generation. Other safety-sensitive commands and worker completions
require the current generation.

## 5. Audio block and source catalog

A capture block is a preallocated pool slot with fixed metadata:

| Field | Type | Notes |
| --- | --- | --- |
| `audioEpoch` | opaque string | Discriminates capture generations (random, non-decimal) |
| `startSample` | u64 native | Position of first sample **frame**, not channel; decimal string only when serialized |
| `frameCount` | u32 | Frames in this block |
| `sampleRateHz` | u32 | Actual verified rate |
| `channelCount` | u16 | Interleaved channels |
| `channelMapRevision` | string | Maps interleaved index → physical source |
| `poolSlot` | handle | Slot identity, not a copy |
| `samples` | interleaved f32 | Allocated before realtime run |
| `captureTimeEstimate` + `uncertainty` | optional | Discontinuity flagged explicitly |

`startSample` counts sample **frames**, shared across all channels, so all sources stay on one
USB capture timeline and preserve source coherence. It is a native u64 in host memory; only the
external JSON form is a decimal string. The catalog gives each source a **stable ID**, physical
index, label, source role, authorization/availability, and verified stereo-pair mapping. Renaming
preserves identity.

## 6. Threading and queues

```
USB callback ──ready slots──▶ single dedicated DSP thread ──per-listener PCM──▶ encoder worker per listener
     ▲                             │                                                   │
     └──── free slots ◀────────────┘                                    encoded queue ─┘──▶ WebRTC event loop
```

- The callback may perform only **bounded per-block copying**, **sample/time counter updates**,
  and **nonblocking atomic queue operations**. It must **not** allocate or deallocate, log, touch
  disk, do networking, encode, take blocking locks, or call the control service.
- Capture pool: proposed default **8 slots × ≤256 multichannel frames**, larger callbacks split,
  bounded, allocation-free. Slots move through **paired SPSC queues**: a *free-slots* queue (the
  callback is the consumer; DSP is the producer) and a *ready-slots* queue (the callback is the
  producer; DSP is the consumer). The producer **never removes or evicts ready entries from the
  consumer-owned side**; it only appends to its own queue and pops from the other side.
- If no free slot exists when the callback needs one, the incoming block is dropped, the absolute
  source-frame timeline **advances**, and **atomic drop counters** record it **even if no block was
  published**. Timeline progression never depends on a block being consumed; this is the defined
  continuing-failure behavior, not a stall.
- Per-listener path: **2 stereo codec frames** of PCM, **2 encoded packets**, plus a **latest
  complete settings snapshot** and an **independent priority safety gate**.
- Capture storage is a memory cap of proposed **42.7 ms @48 kHz** — a bound, **not a permitted
  delay**.

The DSP thread is single; one slow listener cannot block capture or other listeners. Encoder
workers are one per listener.

## 7. Freshness, gaps, and staleness

- A capture block is discarded if its **END** sample position falls more than draft **4 ms**
  behind the latest captured position. END (not start) avoids discarding a normally-wide
  callback.
- Downstream, a complete frame queue residence beyond draft **2 ms** interrupts the **affected
  listener** only (not partial assembly time).
- Gaps reset partial-frame assembly as a safe interruption. If continuity is unknowable, a new
  `audioEpoch` starts.
- All pending work re-checks epochs and `safetyGeneration` and drops stale items; stale audio is
  **never replayed as backlog**.
- All capacities and timing boundaries above are **experiments requiring measurement and
  tuning**.

## 8. Gain and signal chain

Gain conversion is linear `= 10^(dB/20)`. Mute is a **boolean logical −∞**, never numeric
JSON infinity. Proposed ranges are **−60..0 dB in 1 dB steps** with **no positive boost** for
source and master. **Malformed or non-finite values are rejected**, not clamped. Only a **valid
finite numeric value outside the range is clamped** to bounds, after which the host returns the
**canonical setting**. The host validates that a channel or master **unmute is forbidden while
unarmed**; ordinary gain changes are allowed within permissions in a ready state if approved and
must **not force an auto-arm** from a patch alone. Mute is **always allowed** as a priority safety
action, including for the master. This mirrors the mute/unmute and requested-vs-applied semantics
in [DESIGN-SYSTEM.md](DESIGN-SYSTEM.md) §4.

Mono sources are centered with explicitly fixed coefficients (draft unity into each side before
fixed bus attenuation); stereo-linked pairs preserve L/R with **no pan in MVP**. Summation is
float with **no per-input clipping**. With 22 correlated unity sources the worst case is
≈ **26.85 dB**; the POC now uses a fixed **−6 dB bus headroom** (measured; previously a
`−27 dB` probe-only placeholder that made a lone source far too quiet), matching the limiter
ceiling for a single full-scale source. There is **no adaptive fader normalization**, no
source-count scaling, and no AGC. This is a fixed POC policy, not a calibrated hearing-safety
guarantee; correlated sums above the ceiling are bounded by the limiter, and real-hardware
calibration is still required.

Chain order:

```
Σ sources → fixed headroom → master attenuation → stereo-linked limiter → encoder
```

Ordinary gain/unmute ramps use draft **5 ms** amplitude ramps. A downstream **emergency
attenuation** exists at the local receiver and **cannot boost**. The limiter probe ceiling is
**−6 dBFS** with lookahead **0 / 0.5 / 1 ms**; release/attack are chosen from measurement. Any
lookahead adds nonzero latency. Codec output peaking differs from input peaking and is **not a
safe-SPL guarantee**. NaN is sanitized; the affected path faults. Analog clipping upstream is
unrepaired.

## 9. Encoder

Opus runs with **per-listener unique state**, 48 kHz stereo. Draft probe frames are **2.5 ms /
120 frames** compared against **5 ms / 240 frames**, at ≈ **128 kbps**, DTX off, low-delay mode
only where a binding supports it. Query encoder lookahead; FEC adds constraints and is **not free
reliability**. RTP uses 48 kHz timestamps incrementing **120 / 240 per packet** — stereo does
**not** double the timestamp step. On extended-timeline gaps the timestamp advances; an unchanged
RTP source is **not** reset.

## 10. Clocks and drift

The mixer oscillator/source sample clock drives the shared index; the host schedules against
monotonic deadlines. The phone DAC is independent, and WebRTC/browser add jitter and drift. A
**shared index is not a shared physical clock**; NTP does not solve drift. At 13 listeners,
≈ 13 × 400 packets/s = **5200 packets/s** plus protocol overhead. Each phone is expected to
remain a stable low-delay endpoint, but phones are **not sample-synchronous** with each other.

## 11. WebRTC media and transport

Audio media uses WebRTC with **DTLS/SRTP encryption**, separate from the HTTPS/WSS control
channel. Flow: the **browser creates one `recvonly` audio transceiver** and sends `rtc.offer`;
the **host `sendonly` answers**; ICE is **authenticated trickle**. The receiver requests **no
microphone permission**, and no video or data channel is offered. Exactly one intended audio
media section is allowed; SDP containing extra media is rejected, bound to the intended audio,
and rejected again if it deviates.

An SDP `opus/48000/2` line does **not** prove stereo. Actual stereo must be negotiated through
explicit codec preferences and then **qualified by electrical L/R decoding** as in §18. RTP
timestamps derive from the source-frame index, and scheduling/pacing of packet delivery and
stack-internal queue age are **measured** as part of the path, not assumed.

- **str0m** is the candidate `SansIO` stack: it needs an explicit clock, network, and event loop,
  with a **mutation/output-drain invariant** and **external Opus and ICE-gathering** integration.
- **webrtc-rs** is an alternate async option to evaluate for runtime, crypto, and version
  behavior. No unverified API calls or version pins are claimed here.
- **libwebrtc** is a **later fallback only**; it is documented if needed and not committed now.
- `RTCRtpReceiver.jitterBufferTarget` is a latest-preference, **feature-detected** request and
  **not a guarantee**.
- ICE uses host candidates on the **selected interface**, with mDNS resolution and firewall/UDP
  policy handled through a local connection probe; **no public STUN/TURN dependency** is assumed
  for the initial local LAN. Custom SDK configuration values are not invented.

## 12. Control protocol

Transport is HTTPS with WSS upgrade: `GET /join`, `POST /api/v1/pair`, `GET /api/v1/ws`
(WSS). Envelope v1: `v` is the protocol version; `type` names the message; `requestId` is an opaque
unique client string; `hostEpoch` and `sessionEpoch` are opaque random identifiers; the `payload`
always carries a complete, schema-valid body (no placeholder). A `mix.patch` example:

```json
{
  "v": 1,
  "type": "mix.patch",
  "requestId": "req-7f3a-1",
  "hostEpoch": "host-epoch-example",
  "sessionEpoch": "session-epoch-example",
  "payload": {
    "baseRevision": "120",
    "catalogRevision": "34",
    "sources": [{ "sourceId": "ch07", "gainDb": -12, "muted": false }],
    "masterDb": -6,
    "masterMuted": false
  }
}
```

`host-epoch-example` and `session-epoch-example` are descriptive opaque placeholders, not literal
values from any verified function. Wire fields: `requestId`, `hostEpoch`, `audioEpoch`,
`sessionEpoch`, and `armNonce` are opaque strings; revisions, generations, and `startSample`
are decimal-only u64 strings. `frameCount` is a decimal string constrained to the u32 range;
`gainDb`/`masterDb` are finite JSON numbers and mute is boolean.

| Direction | Types |
| --- | --- |
| Client | `mix.patch`, `preset.apply`, `listen.arm`, `listen.disarm`, `rtc.offer`, `rtc.candidate`, `heartbeat` |
| Server | `session.snapshot`, `catalog.snapshot`, `mix.ack`, `mix.applied`, `listener.state`, `listen.armed`, `rtc.answer`, `rtc.candidate`, `telemetry`, `error` |

`mix.patch` uses **absolute** intent: `baseRevision`, `catalogRevision`, absolute `sources` with
`gainDb`/`muted`, plus `masterDb`/`masterMuted`. Validation is atomic against current permissions;
a revision conflict is rejected with a fresh snapshot; a bounded idempotent request cache
deduplicates retries. An accepted `mix.ack` carries `acceptedRevision` and canonical settings; a
`mix.applied` notification carries `appliedRevision`, `audioEpoch`, `startSample`, and settings,
emitted **only for installed snapshots** — skipped or coalesced snapshots are never confirmed as
applied. An `arm` payload carries the current epochs (including `audioEpoch`), `safetyGeneration`,
`appliedRevision`, and a fresh `armNonce`. **Requested ≠ accepted ≠ DSP-applied ≠ audible.** Only
**one patch may be in flight**; unsent UI edits coalesce while preserving newer intentions;
reconnect snapshots state before editing resumes. Draft limits: **30 patch/s**, bounded burst,
**8 KiB** ordinary / **64 KiB** SDP / **64** ICE candidate messages.

## 13. State machines

```
capture:  stopped → probing → running ⇄ discontinuous → fault
server:   stopped → interface-trust setup → listening ⇄ reconfiguration → fault
listener: unpaired → paired → negotiating → ready-muted → armed
                                   ▲                  │        │
                                   └── interrupted ◀──┘        │ (disarm / interrupt / revoke)
                                        (resync) → ready-muted  ▼
                                                          revoked (terminal until re-pair)
```

The listener path is explicit: **armed → interrupted → ready-muted (after resync) → armed only via
a new explicit arm**. `revoked` is **terminal** until a new pairing. A `hostEpoch` or
`audioEpoch` change **invalidates any armed listener** and returns it to
`ready-muted` after resync.

An `arm` carries the current `safetyGeneration`, DSP-applied mix revision, `audioEpoch`, and a
fresh `armNonce`. The host **serializes** each validated arm, assigns a **new generation**, and
captures the nonce; DSP confirms the **exact tuple** before `listen.armed`. `disarm` increments
the generation **separately** and is priority-valid for an authenticated current session even
when its supplied generation predates a pending arm, so a stale worker cannot arm. Safety worker
settings are an explicit, **independent atomic safety state**, so queued ordinary edits cannot
reopen listening. The browser ignores replies with a cancelled nonce or non-current generation.

## 14. Arm, silence, and local gate

The join/arm sequence:

```
userStart gesture
  → local gate closed (still muted)
  → host request validated (epochs, safetyGeneration, appliedRevision, nonce)
  → DSP applies and raises listen.armed with matching tuple
  → browser playback readiness confirmed
  → explicit local gate eligible to open (may need a second gesture; never faked)
```

Source/master protection mutes are maintained separately, so starting never auto-unmutes a
channel or the master. Locally the receiver **stays muted until** a current confirmation **and**
playback readiness. If policy needs a second gesture, the receiver stays muted and must **not fake
success**.

| Action | Local | Host |
| --- | --- | --- |
| Channel mute | — | Mix request; cannot separately mute a constituent of a received stereo pair |
| Personal master mute | Sticky local gate, immediate | Master mute |
| Stop | Local mute/stop/detach; invalidates nonce/generation | Disarm |
| Detected interruption | Local gate | Disarm; re-arm required |

Invoking a local mechanism is **not** an instant electrical-silence guarantee; mute/pause/detach
of HTML media has a real electrical delay that must be qualified, and no AudioWorklet is added
solely to mute. **Acknowledgements, preset recovery, reconnects, and transport recovery never
reopen the local sticky safety gate.** Only an **explicit valid start/unmute** with current epochs
and playback readiness can open it; a **cancelled `armNonce` is permanently invalid for that
attempt**. Stop invalidates the nonce and generation and silences locally with **no network
acknowledgement wait**. Host attenuation is authoritative; local attenuation may only reduce.

Playback visibility, connection, and media stats offer **no universal** promise of audibility or
output-route behavior; wake lock is optional. Draft liveness: **1 s** heartbeat with **3-miss**
lease, and progress polling **250 ms** / absence **500 ms** to interrupt — these are experiments,
**not** latency or dropout acceptance criteria. Busy-browser and buffer-concealment timing are
measured, not assumed.

## 15. Network and TLS pairing

The operator selects a bind interface; candidates and the QR reachable address derive from it.
Ethernet or Wi-Fi is allowed. **VPN/public interfaces are excluded by default**, operator-overridable.
Firewall, AP isolation, multicast DNS, and UDP port policy are validated and the chosen policy is
documented; SDK configs are not invented. The certificate SAN must match the selected IP or a
resolvable hostname, using a documented local CA trust, enrollment, renewal, and removal path,
with offline DNS provisioning and **no warning bypass**.

Pairing uses an OS-random **256-bit**, **one-use**, draft **120 s** token delivered in a QR
fragment, exchanged via `POST`, removed from history, and never logged. Sessions use secure
`HttpOnly`, `SameSite` cookies with an exact allowed-origins list for POST/WS (musician page
same-origin; admin native uses separate IPC). Draft **12 h** expiry or restart, warn before
expiry, **one active receiver per musician** (replacement disarms the old), cap **8** until 13 is
qualified, with bounded pair sessions and control rates. Revocation invalidates epochs/gates and
closes streams, not just UI. Private TLS keys are protected secrets with OS storage to be
qualified.

## 16. Persistence and presets

Configuration is versioned local JSON written **off the audio thread** via atomic replace,
platform validated. Stored: **labels, mapping, and personal settings only**. **Never** stored:
raw pairing or session credentials, cookies, armed state, gate state, browser permissions, audio
data, or recordings. The TLS private key is kept in **separate restricted storage**, not in the
plaintext JSON config; a secret-store candidate is a qualification task. Unsupported versions
**fail safe**. Presets reauthorize sources, clamp to bounds, and **never unmute**: effective mute
is `currentMuted OR presetMuted`, and omitted fields preserve current source/master values. Local
master mute is latched and needs explicit unmute only while armed. On restart, everything is
**unarmed**.

## 17. Telemetry and errors

Telemetry publishes at proposed **10 Hz** from off-thread accumulators: source input, pre-limiter,
and post-limiter with an explicit meter window chosen before implementation; limiter reduction;
queue age/drops; health; epochs; and samples with missed-measurement windows marked
**Unavailable, not zero**. Slow consumers **drop old** data. Safety commands outrank telemetry;
mix application stays independent and prompt with bounded publication.

**Structured errors** are: `REVISION_CONFLICT`, `SOURCE_FORBIDDEN`, `STALE_EPOCH`, `CAPTURE_FAULT`,
and `TLS_IDENTITY_INVALID`. **Separately**, RTP jitter, concealment, and loss are secret-redacted
**statistics with interval deltas**, allowed to be unavailable — they are **not** error codes.
None of these is electrical latency.

### 17.1 Mobile diagnostics contract

The phone displays network/queue diagnostics at **one refresh per second**, independent of the
**10 Hz** digital meters. This 1 s rate is a distinct concern from the **500 ms** progress watchdog
and the **3-miss host heartbeat** safety lease (§14); none of them substitute for each other.

- **Network RTT:** the last reported value is read from the **selected candidate pair**
  (`currentRoundTripTime × 1000`, converting seconds to ms). It reflects the **latest STUN
  round-trip** and is **not guaranteed** to have produced a new measurement on every UI refresh.
- **Browser buffer delay:** only when stats are consistent, the latest interval is
  `1000 × Δ(jitterBufferDelay) / Δ(jitterBufferEmittedCount)`. A value is shown only if the
  **same session, same stats ID, and same stream** hold, in a **consistent epoch** with a
  **positive emitted count** and **finite, non-negative deltas**. On a **new stats ID, session, or
  epoch**, the baseline resets; a **counter reset** starts a fresh first sample and is **not**
  compared against an unrelated counter. Awaiting a baseline/interval shows **Waiting for sample**.
  **Missing, zero-emitted, stale, or unsupported** cases
  show **Unavailable**, never `0`.
- `getStats` provides **sampled, windowed** values; these are **not one-way** measurements and do
  **not** describe the actual audio that is audible.
- **Do not** compute `RTT/2` as one-way, **do not** add `RTT + buffer` into a total, and **do not**
  label link quality or "live-qualified" from network numbers.
- **End-to-end electrical** fields are shown as **"Not measured"** unless an actual **physical
  input-reference vs headphone-output** measurement exists. The POC ships **instructions/procedure**
  for that measurement, not an automated phone full-path measurement promise. An externally
  recorded qualification value may be displayed **only** if it is explicitly marked **historical and
  hardware-matched**, and only if that later feature is approved; **no new persistence** is added now.
- **Freshness** is a required metric: the last successful update time is shown, and failure to
  update is made clear. A selected-candidate RTT is **derived from an actual STUN round-trip**, and
  the `getStats` timestamp alone is **not proof** of a new RTT sample.
- **No software metric here proves the ≤10 ms analog-input-to-headphone-output electrical target.**
  Only the physical input-reference vs headphone-output measurement can approach that claim.
- Optional host block/queue/encoder diagnostics live elsewhere and are **not summed into a total**.
  Browser network stats are polled outside the audio pipeline; host diagnostics are collected
  off the audio thread and **never block the audio callback**.

## 18. Testing plan

- **Unit:** DSP finite-gain/ramps, queue ownership, gap/RTP handling, revisions/ACK, skipped
  snapshots, arm races, authorization, presets.
- **Integration:** controlled clocks and injected faults.
- **Physical:** capture mapping, L/R decoding, peaks, actual silence time, reference + headphone
  electrical correlation; one phone per OS before the full UI; 8 then 13; Ethernet and Wi-Fi;
  2-hour soak; interruption/calls/notifications/lock/route/adapter tests requiring fail-then-rearm;
  user trial.

**No tests have been run.** This means no software, runtime, or hardware checks have been
executed; document review is not a software verification and no such claim is made. Percentile,
excursion, and outage thresholds must be agreed before the probe.

## 19. CI and packaging (future, not implemented)

Version tags drive a matching-OS matrix that builds locked Rust plus frontend, runs lint/types/
unit/integration, and packages a draft release with checksums: Windows EXE/MSI; macOS
Intel/Apple-Silicon APP in DMG with signing/notarization; optional custom PKG; Linux AppImage/DEB
for a named distro/backend. Signing credentials are external. ASIO SDK acquisition and licensed
driver redistribution require review. **Compile success does not equal hardware qualification.**
No automatic updates during monitoring.

## 20. Native fallback

A native receiver would reuse the host DSP, catalog, authorization, and protocol; its transport
is a candidate that must be qualified, and native implementation is **not** started now. If
browsers fail browser qualification, native is evaluated separately. If **both** fail, the project
**stops stage qualification** and **proposes** conventional hardware IEM or a control-only tool as
an alternative for a **separate user decision** — it does not silently change the approved product
scope, and no goal is relaxed.

## 21. Sources

- [README.md](../README.md) and [DESIGN-SYSTEM.md](DESIGN-SYSTEM.md)
- Soundcraft Signature 16/22 User Guide (§7.2), URL in README
- CPAL: <https://github.com/RustAudio/cpal>
- str0m: <https://github.com/algesten/str0m>
- webrtc-rs: <https://github.com/webrtc-rs/webrtc>
- RFC 7587 (Opus RTP): <https://www.rfc-editor.org/rfc/rfc7587>
- RFC 6716 (Opus): <https://www.rfc-editor.org/rfc/rfc6716.html>
- Opus encoder controls: <https://opus-codec.org/docs/opus_api-1.5/group__opus__encoderctls.html>
- MDN `addTransceiver`:
  <https://developer.mozilla.org/en-US/docs/Web/API/RTCPeerConnection/addTransceiver>
- MDN `jitterBufferTarget`:
  <https://developer.mozilla.org/en-US/docs/Web/API/RTCRtpReceiver/jitterBufferTarget>
- W3C WebRTC stats: <https://w3c.github.io/webrtc-stats/>
- MDN `RTCIceCandidatePairStats.currentRoundTripTime`:
  <https://developer.mozilla.org/en-US/docs/Web/API/RTCIceCandidatePairStats/currentRoundTripTime>
- MDN `RTCInboundRtpStreamStats.jitterBufferDelay`:
  <https://developer.mozilla.org/en-US/docs/Web/API/RTCInboundRtpStreamStats/jitterBufferDelay>
- Tauri security: <https://v2.tauri.app/security/>
- Tauri release URLs: see README

These are supplied references reviewed as design inputs, not independently verified SDK APIs.
Candidate names such as `str0m` and `webrtc-rs` denote draft options, not existing selected SDK
functions. No versions are pinned here.
