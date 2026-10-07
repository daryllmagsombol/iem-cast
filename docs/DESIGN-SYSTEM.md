# IEM Cast design system

**DRAFT — proposed normative standard, pending user review.** Nothing here describes implemented UI, approved aesthetics, or stage qualification. “Must” specifies acceptance requirements for future interfaces. Product scope follows [README.md](../README.md); conflicting product decisions require review, not silent changes.

## 1. Purpose and boundaries

Use a calm, high-contrast instrument console: opaque surfaces, readable labels, restrained blue actions, and clear state boundaries. No glass, neon, simulated hardware textures, marketing layouts, or decorative motion. Dark is the proposed default; a complete light alternative supports bright-room readability. Theme choice must not change audio or listening state.

The Rust host captures Soundcraft USB sources and generates each musician’s personal mix: channel volume/mute, linked stereo pairs, and master attenuation. Each phone receives one stereo WebRTC stream, not individually mixed source streams. Plan for 5–8 musicians, then qualify 13; support Ethernet or Wi-Fi hosts. Phones use wired earphones, remain visible and screen-on, and carry audio only.

Personal controls cannot change other mixes or front-of-house settings. Talkback, EQ, effects, and recording are outside MVP. A digital limiter cannot guarantee safe ear SPL. Digital meters are not acoustic calibration; reported network RTT is not analog-input-to-ear latency.

## 2. Canonical tokens and themes

This document is the draft source of truth. After approval, one shared canonical token file must supply desktop and receiver primitives; do not maintain separate theme copies. Proposed CSS names use `--iem-color-*`, `--iem-space-*`, `--iem-type-*`, and corresponding radius, size, elevation, and motion prefixes. Values below are specifications, not existing code.

### Color

| `--iem-color-` suffix | Dark | Light | Role |
| --- | --- | --- | --- |
| `canvas` | `#11161C` | `#F3F5F7` | Page background |
| `surface` | `#1B232D` | `#FFFFFF` | Cards, dialogs, Master dock |
| `surface-raised` | `#26313D` | `#E8EDF2` | Inputs, hover/pressed secondary controls |
| `text` | `#F2F5F8` | `#18232F` | Primary content |
| `text-secondary` | `#B5C0CC` | `#4B5B6B` | Hints, unavailable explanations |
| `boundary` | `#8795A5` | `#667586` | Essential control borders, meter track outlines |
| `accent` | `#8CBFFF` | `#194F90` | Primary actions, links, focus |
| `on-accent` | `#11161C` | `#FFFFFF` | Filled action text/icons |
| `success` | `#7CD6A3` | `#17633D` | Connection/normal digital level |
| `warning` | `#F1C66D` | `#795100` | Caution/high digital level |
| `danger` | `#FF9A9A` | `#A12432` | Failure/clipping/stop outline |

Status badges use status-colored text/icons on opaque surfaces, not untested tinted fills. Destructive buttons use danger text and border on surface. Primary buttons retain accent fill on hover/press; add an on-accent inset outline instead of lowering opacity. Secondary buttons switch to surface-raised. Disabled controls retain secondary text, native disabled semantics, and an explanation; do not fade essential information. Selected states add a checkmark and text. Focus uses a 2px accent outline with a 2px surface-colored gap; it must remain visible around filled buttons.

### Calculated contrast

WCAG sRGB linearization uses the 0.04045 threshold, luminance weights 0.2126/0.7152/0.0722, and `(Llighter + 0.05)/(Ldarker + 0.05)`. These Python-calculated minimum ratios cover each foreground against **all three theme surfaces**, rounded only for presentation:

| Pair group | Dark minimum | Light minimum | Required use |
| --- | --- | --- | --- |
| text / surfaces | 12.08:1 | 13.50:1 | Normal text ≥4.5:1 |
| text-secondary / surfaces | 7.16:1 | 5.93:1 | Normal text ≥4.5:1 |
| accent / surfaces | 6.93:1 | 6.96:1 | Text ≥4.5:1; focus ≥3:1 |
| success / surfaces | 7.55:1 | 6.17:1 | Status text ≥4.5:1 |
| warning / surfaces | 8.20:1 | 5.96:1 | Status text ≥4.5:1 |
| danger / surfaces | 6.51:1 | 6.34:1 | Status text ≥4.5:1 |
| boundary / surfaces | 4.32:1 | 4.00:1 | Essential non-text edges ≥3:1 |
| on-accent / accent | 9.53:1 | 8.20:1 | Button text ≥4.5:1 |

Boundary is not a text token. Meter tracks have surface interiors, boundary outlines, and surface gaps between segments. These checks do not establish whole-interface WCAG compliance or distinguish adjacent meter segment colors; labels, positions, and numbers supply redundancy. Recalculate new combinations, transparency, and overrides before adoption.

### Typography and geometry

Offline font stack: `system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif`. Readouts use `ui-monospace, "SFMono-Regular", Consolas, "Liberation Mono", monospace` with tabular numerals. No runtime font downloads. Labels use ordinary casing; reserve uppercase for short technical units.

| Token suffix | Specification |
| --- | --- |
| `type-caption` | 14px/20px, weight 400; nonessential metadata |
| `type-body` | 16px/24px, weight 400 |
| `type-label` | 16px/24px, weight 600 |
| `type-section` | 20px/28px, weight 600 |
| `type-title` | 28px/36px, weight 600 |
| `type-readout` | 18px/24px, weight 600, tabular |
| `space-1` through `space-8` | 4, 8, 12, 16, 24, 32, 48, 64px |
| `radius-control`, `radius-panel`, `radius-badge` | 4, 8, 4px |
| `size-target`, `size-target-compact` | 48px default; 44px minimum desktop |
| `size-icon` | 20px, consistent 2px stroke |
| `elevation-flat`, `elevation-overlay` | None; `0 8px 24px #00000033` |

Use rem equivalents relative to a 16px root; allow user scaling. Panels and controls have 1px boundary outlines; shadows are never their sole boundary. Standard density: 16px panel padding and gaps, 24px section gaps. Compact operator density: 12px padding, 8px gaps, 44px control height; never apply it to phone controls. Overlay surfaces remain opaque; use a `#00000099` scrim only behind them.

## 3. Responsive structure

Support phones from 320 CSS px without horizontal page scrolling. Use 12px gutters below 480px and 16px thereafter. Channel controls form one vertically scrolling list with horizontal gain ranges; never require tiny vertical faders, sideways strips, swipes, or dragging to operate. At 768px, optional two-column cards may improve space use if labels and targets still fit. At 1024px, the desktop shell gains persistent 240px navigation and 24px gutters; cards may use denser grids. Wide layouts cap content at 1440px.

Each strip shows source name, channel identity, stereo-link label when applicable, gain value, meter, and Mute. Long names may ellipsize visually, but preserve the full accessible name and provide a tap/keyboard-operable details disclosure; hover alone is insufficient. Preserve stable source identifiers across renames.

The personal **Master** dock remains visible with attenuation, mute, and Stop listening. Reserve its measured height plus bottom safe-area inset in scroll content. Include top safe-area clearance and scroll offsets so docks never cover focused controls. At zoom or short landscape heights, use a compact visible dock with an accessible expansion for full controls, without clipping content. Do not assume browser chrome can be hidden or themed.

## 4. Shared component contract

Tailwind CSS is the selected styling approach for the React + TypeScript desktop admin and phone UI. Nothing is implemented or installed; shared primitives and tokens remain proposed. No component library is selected or required.

Use shared canonical semantic CSS tokens to feed Tailwind utilities in both dark and light themes. Preserve specified tokens; do not duplicate palettes, embed raw hex values in components, or repeat arbitrary-value utilities overriding the system. Reusable React components own behavior; semantic HTML, visible and accessible labels, keyboard support, non-drag alternatives, and native accessible controls remain implementation responsibilities, not features supplied by CSS.

Generate and bundle Tailwind CSS at build time with local assets; no show-time CDN or runtime styling network dependency. Node remains build tooling only. Choose the Tailwind version and integration before coding; this document selects no version, plugin, or configuration.

Lucide outline is the single proposed icon family, bundled as offline SVG assets after license review. Keep visible text on safety actions; hide redundant icons from assistive technology and name standalone icon controls.

| Component | Required behavior |
| --- | --- |
| Button | Primary, secondary, destructive; visible focus and disabled/pending explanation |
| Toggle | Native checkbox or button with `aria-pressed`; label states explicitly |
| GainControl | Named native range, readout, decrease/increase buttons, numeric entry |
| ChannelStrip | Stable source identity; linked stereo controls act as one unit |
| DigitalMeter | Input/output identity, dBFS readout, peak/clipping text, stale state |
| StatusBadge | Text plus icon; transport and listening states remain separate |
| PairedSessionNotice | Musician/session identity, permissions, pairing expiry/recovery |
| Notice | Persistent actionable empty, warning, error, or reconnect message |
| Dialog | Named modal, focus containment/restoration, explicit Cancel |

Host Stop all listening requires confirmation naming the affected session. It is admin-only. Personal Master Mute and Stop listening act locally and immediately without confirmation or animation delay; Channel Mute requests a host-side change and shows pending state until DSP application. Dialogs must not hide an available immediate personal silence action.

### Gain and meters

Host configuration defines valid gain bounds and steps; the UI must not invent DSP ranges. Proposed attenuation-only maximum is deliberate **0 dB** (unity), subject to host approval; never offer local boost. Label mute as **Muted (−∞ dB)**, separately from the lowest finite gain. Remembering an earlier value must not automatically unmute. An explicit Unmute restores a bounded value only within an armed session.

Use native range semantics with visible labels and `aria-valuetext`, for example “Lead vocal, minus 12 decibels”; mute is a separate named control, not a fabricated numeric infinity. Support arrow-key host steps, Home/End bounds, labeled decrease/increase buttons, and validated numeric entry. No double-tap reset to unity or scroll-wheel gain change. Preserve vertical page scrolling outside the thumb; non-drag controls provide complete access. Space gain buttons at least 8px apart.

Display optimistic edits as “Requested −12 dB · Pending”; show the previous DSP-applied value separately. “Accepted” means the server validated a request; “Applied on host” requires a matching server revision confirming audio-worker application, not merely message receipt or acceptance. This does **not** establish which audio is currently audible through receiver buffers. Never label it “Playing at −12 dB.” Rejects restore DSP-applied values with an explanation; stale replies cannot replace newer requests or display success. Reconnection resynchronizes state before accepting edits.

Propose meter telemetry at 10 updates/second, independent of audio callbacks and UI frame rate. Label input versus personal stereo output and units as dBFS. Green/amber/red segments, numeric digital peak, and explicit “Clipping” text are redundant; no “safe hearing” zone. A cosmetic one-second peak hold is proposed; thresholds and measurement windows require DSP agreement. Mark missing telemetry “Unavailable,” not zero, and suppress repetitive screen-reader announcements.

### LatencyDiagnostics

Mobile latency diagnostics are approved for the reusable macOS POC; this is a draft component contract, not implemented UI. Refresh the display once per second, separately from 10Hz meters and safety watchdogs; diagnostics must never delay silence or re-arm handling.

| Visible label | Meaning |
| --- | --- |
| Network RTT | Last reported WebRTC selected ICE-pair STUN round trip, in ms; not one-way latency. A display refresh does not imply a fresh RTT measurement. |
| Browser buffer delay | Latest-interval average jitter-buffer hold, in ms; not the full audio path. |
| End-to-end audio latency | “Not measured” unless a physical analog-reference-to-headphone electrical measurement has been performed; any measured result must identify its test conditions. |

For network/buffer values, missing or unsupported data shows “Unavailable”; first samples awaiting a valid interval show “Waiting for sample”; stale values show “Unavailable” with the last-updated label. Never substitute zero. On disconnect, clear current readings to “Unavailable” and retain a last-updated label where known. Distinguish display refresh time from measurement/report freshness; do not invent a fresh timestamp for unchanged RTT data. Never sum RTT and buffer delay or halve RTT to claim end-to-end latency. Network statistics cannot establish stage qualification or hearing safety.

Place compact, stacked label/value rows near the phone’s personal output meter, in normal scroll flow without displacing the Master dock. Use existing semantic text/surface tokens, tabular readouts, and explicit ms units; allow wrapping at narrow widths. No new palette or green “stage qualified” indicator. Keep values screen-reader accessible without live announcements every second; announce connection changes through existing status notices.

## 5. Screens, permissions, and states

Desktop views: device/capture setup; available sources and stereo mapping; Ethernet/Wi-Fi interface and reachable address selection; certificate enrollment instructions; session pairing; and operator monitoring of musicians. Keep USB master L/R excluded from source summation by default. A separate master-only mode, if approved, must be distinct from personal Master attenuation.

Phone views: join/pair; connected-but-muted listening gate; current personal channels and Master; and saved personal settings. MVP presets save only the musician’s channel/master settings, never global configuration, pairing credentials, or armed status. Loading a preset cannot unmute a channel or start playback implicitly. Admin visibility and authorization are unavailable to musician sessions, not merely hidden tabs.

| State | Display and permitted action | Silence/recovery rule |
| --- | --- | --- |
| Empty sources | “No sources available”; operator setup guidance | Start listening unavailable |
| Loading/pairing | “Connecting”; Cancel, no fake progress | Remain muted |
| Connected, unarmed | “Connected · Not listening”; Start listening | Explicit gesture arms after readiness checks |
| Armed | “Listening”; immediate Mute/Stop listening | Do not infer audibility or safe SPL |
| Edit pending/rejected | Requested/applied distinction; inline reason | No stale success or unsafe retry |
| Warning | Clipping or screen-awake unavailable; clear guidance | Never auto-adjust/unmute to dismiss |
| Stale/disconnected | “Connection lost · Output silenced” only after local silence; otherwise “Silencing output” | Suppress stale playback; no automatic resume |
| Reconnecting | “Reconnecting · Not listening”; Cancel | Restore acknowledged settings, remain unarmed |
| Offline/host stopped | “Host unavailable”; retry guidance | No queued audio replay |
| Output-route/lifecycle interruption | “Check wired earphones · Start listening again” | Silence and require re-arm |
| Certificate/pairing failure | Enrollment or fresh-pairing instructions | No warning bypass or automatic listening |

Joining starts muted. Start listening is an explicit gesture; inability to start browser playback leaves the user unarmed with an explanation. Stop listening silences locally and disarms the session. Local attenuation may reduce output, never boost it. Channel mute requests affect the host mix; personal silence must not wait for a network acknowledgement.

Certificate failures may prevent JavaScript loading entirely: recovery instructions belong in operator setup/offline onboarding, not a purported in-page privileged repair button. Display “Keep this page visible and screen on.” Request screen wake lock only where available, best effort; denial/release needs guidance and never implies lock-proof playback. Detectable route changes require re-arm; reliable interruption detection, silence timing, and wake behavior need real-device qualification. Never equate a connected transport with armed audio.

## 6. Motion, accessibility, and acceptance

Use rare 100ms state feedback and 180ms dialog transitions with ease-out. No pulsing, animated clipping alarms, parallax, or layout-shifting presses. Reduced motion removes nonessential transitions. Mute/disconnect feedback is immediate; audio ramps belong to host DSP, not CSS timing.

Before UI acceptance, test keyboard-only operation, native range alternatives, VoiceOver and TalkBack names/states, modal focus, and restrained live-region notices. Check 48px phone targets, 44px desktop minimums, 320px width, 200% zoom, portrait/landscape, both themes, reduced motion, and forced-colors/high-contrast mode. Preserve platform focus/control visibility; do not suppress user color overrides.

Negative-path tests must cover stale acknowledgements, preset loading, output changes, network changes, and host restart: none may auto-unmute or arm. Verify silence remains immediately available during failures/dialogs. Native receiver fallback, if separately qualified, follows these semantics using platform widgets, not an assumed existing native interface.

## 7. Governance and evidence

Approve this draft before implementing tokens. Changes must update this document and the future canonical tokens together, name affected components/states, and include contrast and interaction evidence. All screens follow the same system; no arbitrary page overrides. User review must resolve theme approval, host gain bounds/steps, meter semantics, and preset scope. Hardware qualification must resolve browser lifecycle, safe silence, and actual audio performance.

Sources used: repository README baseline; `ui-ux-pro-max` skill, design-system query and narrower style search, and professional checklist; [W3C Contrast Minimum guidance](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html). The initial skill result suggested marketing/neumorphic styling and was rejected; narrower dark/dashboard guidance informed restrained density, not neon effects. Validation here is document review and calculated token contrast only. No browser, screen-reader, runtime, or hardware tests have been performed.
