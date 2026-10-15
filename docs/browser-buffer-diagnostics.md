# Browser buffer diagnostics

This note defines the additional, read-only browser latency diagnostics added alongside the
existing Network RTT and Browser buffer delay rows. It is **observability only**: nothing here
tunes buffering, changes packetization, changes the jitter-buffer target value, or alters playback.

## Fact definitions

All fields live on `DiagnosticsSnapshot` (`apps/web/src/receiver-ports.ts`), refreshed at the
existing **1 Hz** interval. Values are the explicit `'Unavailable'` label (never a fabricated `0`)
or `'Waiting for sample'` while awaiting a first/comparable interval.

| Snapshot field | Meaning | Unit / shape |
| --- | --- | --- |
| `buffer_target_delay_ms` | Latest-interval average browser **target** jitter-buffer hold, from `jitterBufferTargetDelay`. A target, not the actual buffer. | ms, or `'Unavailable'`/`'Waiting for sample'` |
| `buffer_minimum_delay_ms` | Latest-interval **minimum achievable** delay from network characteristics alone, from `jitterBufferMinimumDelay`. | ms, or `'Unavailable'`/`'Waiting for sample'` |
| `packets_lost_delta` | Non-negative lost-packet delta over the latest interval, from the **signed** `packetsLost` counter. A genuine unchanged count is `0`. | count, or `'Unavailable'`/`'Waiting for sample'` |
| `concealment_percent` | `100 × Δ(concealedSamples) / Δ(totalSamplesReceived)` for the latest interval. | percent, or `'Unavailable'`/`'Waiting for sample'` |
| `jitter_buffer_preference` | `{ state, requested_ms }` describing the **request** outcome, not the actual buffer. | see below |

Raw fields are carried on `InboundStats` (`targetDelay`, `minimumDelay`, `packetsLost`,
`packetsReceived`, `concealedSamples`, `totalSamplesReceived`) only when finite and non-negative;
absent raw fields stay **absent** (`undefined`), never `0`.

### Computed values

- Target/minimum delay reuse the existing buffer-delay identity and interval guards:
  `1000 × Δ(field) / Δ(jitterBufferEmittedCount)`, in ms. The raw browser fields are seconds.
- Concealment uses a **compatible positive** `Δ(totalSamplesReceived)` denominator; a
  `Δ(concealedSamples)` larger than it is an impossible/asymmetric counter reset and yields
  `'Unavailable'`, never a value > 100 or a clamped 100.
- Availability is **independent per metric**: a missing minimum delay, for example, does not erase
  a valid target delay.
- Same-interval counter decrease, reset, non-finite, or zero/negative denominators yield
  `'Unavailable'`; a first sample or a changed `sessionEpoch`/`statsId`/`stream`/`epoch` yields
  `'Waiting for sample'`. A reading stale past the **3 s** freshness window becomes `'Unavailable'`
  while a genuine `'Waiting for sample'` interval is preserved. Because `last_updated` advances on
  each successful refresh, staleness can only trip after a `getStats()` stall longer than the
  window (covered by an explicit controller test).

### Jitter-buffer preference states

`jitterBufferTarget` is a **preference, not a cap** (MDN: it must be a positive value ≤ 4000 ms and
only *influences* the user agent; the actual target varies with the UA's own limits and may change
at any time; readback does not reflect the UA's actual target). Browsers that lack it (older
Chrome/Safari) get the seconds-valued `playoutDelayHint` fallback; the millisecond request is
converted. Both are preferences: the request only takes effect while the audio sink is playing, so
the port re-asserts it after `play()` succeeds and at most once per diagnostics refresh.

| `state` | Meaning |
| --- | --- |
| `'Not requested'` | A supported receiver was never asked (before `setJitterBufferTargetMs`). |
| `'Accepted'` | The assignment did not throw. **Never** proof of the actual buffer or lower latency. |
| `'Unsupported'` | A receiver **was observed** and genuinely lacked both `jitterBufferTarget` and `playoutDelayHint`. |
| `'Rejected'` | The property exists on a supported receiver but the assignment threw. |
| `'Unavailable'` | No capability was observed: the port exposed no status accessor, no receiver existed, or the feature-detect itself could not be evaluated. No capability claim is made. |

The feature-detect uses `'jitterBufferTarget' in receiver` (falling back to
`'playoutDelayHint' in receiver`), which checks property presence and does **not** invoke a getter;
only a `Proxy` `has` trap could throw there. A throwing `has` trap is an **unknown** capability, not
an observed absence, so it also surfaces as `'Unavailable'` — never an invented `'Unsupported'`.
`'Unsupported'` is reserved for a receiver that was actually observed without either property. This
distinguishes unsupported from unknown/error without inventing an `'Unsupported'` claim when nothing
was observed. `requested_ms` is the millisecond value last requested, or `null` when nothing was
observed.

## Limitations

- These are `getStats()` sampled, windowed values. They are **not** one-way measurements, not
  electrical latency, and do not describe the audio a listener actually hears.
- `end_to_end` stays **`'Not measured'`**: no browser statistic is a physical
  analog-reference-to-headphone measurement.
- RTT and buffer values are never summed or halved, and no "stage qualified"/"safe hearing" claim
  is made.
- `'Accepted'` is not evidence of an actual browser target value, of a reduced buffer, or of lower
  end-to-end delay. Confirming that requires before/after measurement on real hardware, which has
  **not** been performed here.

## UI presentation

The existing **Latency diagnostics** panel keeps **Network RTT**, **Browser buffer delay**, and
**End-to-end audio latency** visible. RTT and buffer readings remain separate; end-to-end remains
**Not measured**. The existing full-path/physical-measurement explanation stays visible.

A collapsed native **Browser buffer details** disclosure adds:

- **Browser target delay** and **Browser minimum delay**, in rounded milliseconds.
- **Packets lost**, as a packet count over the latest sample interval, not a rate or cumulative total.
- **Audio concealment**, as a latest-interval percentage (up to two decimals; a positive value below
  0.01% is shown as **<0.01%**, not zero).
- A **Jitter buffer preference** group with **Requested preference** in ms and **Request outcome**.
  A requested **0 ms** is explicitly a preference, not the actual buffer or a cap. A null requested
  value shows **Not requested** only for the matching state; otherwise it shows **Unavailable**.

Zero-valued metrics stay zero. **Unavailable** and **Waiting for sample** remain literal labels
without fabricated numbers or unit suffixes. No threshold colors, diagnoses, latency totals, or
performance promises are added. The panel is read-only and contains no setting/playback controls.

Final helper copy:

> Network and buffer readings are not full-path audio latency. End-to-end requires a physical measurement.

> Readings below cover the latest sample interval. Target and minimum are browser-reported interval averages, not measurements of the sound reaching your ears or its minimum possible delay.

> Audio concealment is the share of audio samples the browser replaced to cover missing audio.

> Accepted means the request was accepted, not that the actual buffer matches it. The browser may ignore the preference or choose a higher delay.

The native disclosure supports Enter/Space, preserves its open state across snapshot updates, uses
the existing focus/theme tokens and a 48px minimum target, and participates in the receiver's
existing dock-clearance behavior. Wrapping label/value rows support narrow screens and larger
text. Snapshot updates are deliberately **not** live-announced every second.

Focused UI checks:

```sh
npm test --workspace apps/web -- src/ui/__tests__/LatencyDiagnostics.test.tsx
# From apps/web; owns and closes its own Vite server on strict port 5194 and browser instances
node src/ui/__tests__/LatencyDiagnostics.browser.mjs
```

The browser harness mounts the real `ReceiverView` with fixed diagnostics and a long source name;
it performs no media/signaling operations. Chromium and WebKit verify 320px/390px/desktop/landscape
layouts, light/dark themes, keyboard disclosure/focus, dock clearance, 200% text scaling, and request
and metric states. These fixture checks are not physical-phone or audio-latency measurements.

## Retest instructions

```sh
# From the worktree root
npm ci
npm test --workspace apps/web -- transport/__tests__/diagnostics transport/__tests__/stats transport/__tests__/media transport/__tests__/controller
npm run typecheck
npm test
npm run build:web
```

The focused suites cover per-metric availability, counter resets/decreases, stale/reset behavior,
unit conversions (seconds → ms), signed-negative loss handling, and every preference state
(unsupported-observed, feature-detect `has`-trap throw, assignment throw, readback, the
no-receiver case, and the absent-accessor case).
