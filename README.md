# IEM Cast

**Status: prototype in progress — UI preview available; live casting is not wired end to end.**

IEM Cast is a prototype for a live-performance in-ear monitor (IEM) system for bands and musicians.
The intended path takes multichannel audio from a Soundcraft Signature 22 MTK connected
by USB to a Rust desktop host, mixes personal monitor feeds on that host, and delivers one
independent stereo stream per musician over a local network to a phone browser. It is
audio only; video is out of scope.

The repository contains Rust `host-core` building blocks (capture, DSP, Opus, control/safety,
HTTPS/WSS, WebRTC transport), a Tauri operator shell, and React + TypeScript + Tailwind
interfaces. These do **not yet form a working live-casting application**. Native host startup,
secure pairing, source-setting persistence, monitor IPC, and the live phone connection remain
incomplete; the UI marks unsupported actions unavailable rather than reporting success.
**Real-hardware qualification has not been performed.** Rendering a screen or passing unit tests
does not prove that mixer audio reaches a phone. See [the qualification record](docs/qualification/macos-poc.md).

**Project site:** <https://daryllmagsombol.github.io/iem-cast/> — documentation plus a clearly
labeled silent, simulated mix demo (no host connection, no audio).

## Run the UI

From the repository root, with Node.js 24.15 or newer:

```bash
npm ci
npm run dev:admin --workspace apps/web
```

Open the address Vite prints (normally <http://127.0.0.1:5173/>). The operator dashboard shows
setup values and current build limitations. Choose **Open simulated preview** to try fictional
device selection and mix controls. Preview mode is silent, uses in-memory fixtures, and never
connects to hardware or a host.

For the native operator window, with Rust and the macOS development prerequisites installed,
run these in two terminals:

```bash
# Terminal 1: keep the admin development server running
npm run dev:admin --workspace apps/web
```

```bash
# Terminal 2: build the web assets and launch the Tauri window
npm run dev:desktop
```

The desktop context can enumerate devices through IPC. It does not make the disabled live-casting
features operational. The standalone browser view has no native device access.

To run the public documentation/demo site locally:

```bash
npm run build:site --workspace apps/web
npm run preview:site --workspace apps/web
```

Open <http://127.0.0.1:4173/iem-cast/>. This is a separate silent demo, not a hosted audio server.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) — proposed implementation contracts.
- [Design system](docs/DESIGN-SYSTEM.md) — shared UI/interaction standard.
- [macOS POC plan](docs/superpowers/plans/2026-10-07-macos-poc-implementation.md) — execution plan
  and approval gates.
- [Qualification record](docs/qualification/macos-poc.md) — verified software results and the
  remaining manual hardware gates.

## Goals and requirements

- Support 5–8 independent musicians initially, with an expansion target of 10–13.
- Target desktop hosts on Windows, macOS, and Linux; qualify audio capture on each OS.
- Capture and mix the live performance in real time with per-musician control.
- Personal mixes are delivered to iPhone Safari and Android Chrome browsers first.
- Musicians listen through wired earphones via the phone headphone jack or USB-C/Lightning
  adapters only. No wireless earbuds in the initial scope.
- The browser page must remain active and visible, with the phone screen on. Background playback and
  screen-lock operation are not guaranteed.
- Keep conventional wired monitoring available while testing. No "stage ready" claim is made
  without full qualification.

Browser delivery is the first approach. Native receivers are a separately qualified fallback
only if browsers fail; native does not inherently guarantee latency either.

## Hardware and source constraints

The approved source is the Soundcraft Signature 22 MTK. Per the manufacturer's user guide
(§7.2), USB sends 1–22 carry the individual analog inputs post-gain/pre-EQ, and USB sends
23/24 carry the master L/R.
These sends are not the console EQ/fader mix, and changes to analog input gain are shared
across all destinations. A stereo analog audio output cannot provide independently adjustable
original source channels without additional distinct inputs or interfaces. We therefore treat
the USB multichannel capture as the primary source, not analog sends.

The master is excluded from source summation by default to avoid doubling constituent sources.
A separate master-only listening option may be offered. The initial version captures only and
does not return audio to the mixer over USB, to avoid routing and feedback risk. Verify
stereo-pair mapping against the manual and actual device during capture qualification.

## Architecture

- **Host (Rust):** USB multichannel capture, DSP, gain matrix, audio encoding, network
  transport, bundled local assets, and the local web server.
- **Desktop shell (proposed):** Tauri hosting a React + TypeScript admin UI used by the
  operator to choose available sources, label them, and manage musicians.
- **Build tooling:** Node is used for build tooling only. There is no required runtime audio
  server component in Node.
- **Receivers (initial):** phone browsers (Safari, Chrome) act as thin clients, not as a
  multichannel audio engine.

One independent stereo WebRTC/Opus stream is produced per musician. The host also runs the
per-musician gain matrix: volume/mute, linked stereo pairs, and a personal master attenuation.
Realtime audio callbacks must not perform blocking network or UI work; queues are bounded and
must not replay stale audio backlog. Capture, DSP, and network share one timeline; gain
smoothing and clock/drift handling (including WebRTC policy behavior) are handled and
measured rather than assumed. No SFU or cloud service is required initially.

## Network model

The host works over **either Ethernet or Wi-Fi**. An Ethernet host connected to a dedicated
5 GHz access point is the recommended qualification baseline, but it is not a requirement.

The host detects usable interfaces and lets the operator choose one when multiple exist. The
QR join address must be reachable through the selected interface, and phones must be able to
reach the host on the same routable LAN. Guest networks, AP isolation, and firewalls can
prevent this and are explicit qualification concerns. Network changes update the address and
require a safe reconnect; there is no seamless-reconnect guarantee. Streams are per-client
unicast, so scaling must be tested at 8 and at 13 clients. No multicast behavior is promised.

## Joining, controls, and safety

- Local bundled assets mean there is no show-time cloud dependency.
- Musicians join by scanning a QR code with pairing credentials that are separate from admin
  permissions. No musician control can affect another musician's mix.
- Web assets, controls, and signaling use trusted HTTPS; media uses WebRTC's encrypted
  transport, not HTTPS audio streaming. Initial device qualification uses documented local
  certificate enrollment; bypassing certificate warnings is not an accepted standard
  workflow. Per-interface addressing and certificate identity, plus offline TLS onboarding,
  are qualification tasks.
- Listening starts only after an explicit user gesture.

Safety behavior is planned as follows: start muted; ramp changes and unmute transitions;
bound gains; apply a personal-mix limiter; keep attenuation and mute personal rather than
allowing unlimited boosting; silence output on disconnect instead of replaying stale audio;
and re-arm on reconnect or output-route changes. A digital limiter is **not** a safe SPL
guarantee — calibration, phone volume, and earphone sensitivity all matter.

## Technology candidates

These are candidates to qualify, not final selections.

- **Capture/backend:** CPAL is a candidate for cross-platform capture, but is not final. The
  capability test must confirm hardware and full channel access with practical buffer sizes on
  macOS CoreAudio, Windows WASAPI/ASIO (including driver, build, and licensing caveats), and a
  chosen Linux ALSA distribution.
- **Capture rate:** prefer a 48 kHz initial capture mode, pending hardware validation. The
  manufacturer documents supported 44.1/48 kHz modes.
- **Desktop shell:** Tauri with React + TypeScript (proposed).

For receiver buffering, `RTCRtpReceiver.jitterBufferTarget` is a preference that influences,
but does not exactly control, the jitter buffer
(<https://developer.mozilla.org/en-US/docs/Web/API/RTCRtpReceiver/jitterBufferTarget>).
`AudioWorklet` requires a secure context
(<https://developer.mozilla.org/en-US/docs/Web/API/AudioWorklet>), and the WebRTC receiver
baseline need not introduce it unless the implementation requires it. Opus interoperability
is described in RFC 7874 (<https://www.rfc-editor.org/rfc/rfc7874.html>).

### Approach comparison

| Approach | Strengths | Costs / caveats |
| --- | --- | --- |
| **Host mix + WebRTC (recommended)** | Low phone workload; source coherence maintained at the host | Browser buffering limits how low latency can go |
| **Browser multichannel mix** | Local faders in the browser | High traffic/decode load; difficult sample alignment; defer |
| **Native receiver** | More lifecycle and buffer control | More app work; still must be tested as a fallback |

No promise of sub-10 ms latency is made for any approach.

## Validation plan

The engineering target is **≤10 ms analog-input to wired-headphone electrical output**. This
is an engineering goal, not a universal comfort boundary and not a guaranteed browser
capability. It is measured as electrical input-to-output latency, not as ping and not from
packet timestamps.

Method: record a simultaneous input reference and the headphone output, then correlate
impulses to derive latency across the full path — USB capture, DSP, codec, Wi-Fi, jitter,
decode, phone DAC, and limiter. Report median, high percentiles, worst excursions, and
dropouts. Qualify one phone per OS before building the full UI, and the first desktop
hardware before the remaining OS targets.

Load and environment testing covers 5–8 musicians, then 13; host on Ethernet and on Wi-Fi; a
proposed two-hour soak; and musician trials. Disruption tests include disconnect/rejoin, host
restart, calls and notifications, background/lock interruptions, dongle removal, output-route
changes, and stale-audio behavior. Screen-on is an operational requirement.

Before the first probe, agree which latency percentiles and maximum excursions must meet
the target, and define an acceptable interruption limit. Those statistical pass/fail criteria
are not yet agreed; no stage-ready claim is permitted without them. Record outage durations
and require musician approval rather than silently relaxing the goals. CI builds cannot
qualify physical hardware.

## Milestones

1. Capability probe: confirm capture backend, channel access, and practical buffers on the
   first desktop OS. Use a minimal browser receiver with trusted HTTPS to measure the full
   electrical latency path before building the full control UI.
2. Qualify one iPhone and one Android phone, then repeat capture qualification on the
   remaining desktop OS targets. Stop to evaluate native reception if browser timing fails.
3. Build operator source selection/labeling and the per-musician gain matrix.
4. Turn the qualified receiver into the musician interface; add QR joining, pairing,
   and documented local TLS onboarding.
5. Scale and soak testing at 8 and 13 clients over Ethernet and Wi-Fi.
6. Disruption and rejoin testing, then musician trials and stage qualification.

If browsers fail qualification, evaluate native reception separately. If native reception also
fails, the system is not suitable for live IEM use, and the alternatives are conventional wired
monitoring, dedicated hardware IEM, or a control-only product.

## Packaging and release (approved, not implemented)

GitHub Actions should match the OS matrix, build Rust plus the bundled frontend, run checks,
produce artifacts with checksums, and attach them to a draft Release created from version
tags. Planned formats: a Windows `.exe` installer/MSI; a macOS `.app` in a DMG for Apple
Silicon and Intel with signing and notarization; and a PKG as additional custom packaging if
needed, which is not promised as native Tauri output. Linux would ship AppImage and deb
initially, with named-distribution hardware qualification required. Windows signing
certificates, Apple credentials, and ASIO SDK/driver licensing and redistribution must be
reviewed; secrets are never committed. There are no automatic updates during monitoring, and
unverified workflow versions are avoided.

## Decisions to resolve through qualification

- **Actual test host OS/hardware, phones, adapters, and AP** are collected before the capability
  probe begins.
- **Backend and transport implementation:** use candidate implementations for the probe;
  confirm or replace them based on its measurements before committing to a production stack.
- **Stage approval** requires measured latency and load results plus a live musician trial; it
  cannot be granted from design review or CI alone.

## Sources

- Soundcraft Signature 16/22 User Guide, §7.2:
  <https://adn.harmanpro.com/product_documents/documents/4359_1477349038/Soundcraft_Signature_16-22_User_Guide_original.pdf>
- Tauri distribution: <https://v2.tauri.app/distribute/>
- Tauri GitHub pipelines: <https://v2.tauri.app/distribute/pipelines/github/>
- CPAL: <https://github.com/RustAudio/cpal>
- MDN `jitterBufferTarget`:
  <https://developer.mozilla.org/en-US/docs/Web/API/RTCRtpReceiver/jitterBufferTarget>
- MDN `AudioWorklet`: <https://developer.mozilla.org/en-US/docs/Web/API/AudioWorklet>
- RFC 7874 (Opus interoperability): <https://www.rfc-editor.org/rfc/rfc7874.html>

_This document describes the intended architecture and the prototype's current limits. Software
checks have been run; real-device playback, electrical latency, and stage qualification remain unverified._
