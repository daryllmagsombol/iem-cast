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
at any time; readback does not reflect the UA's actual target).

| `state` | Meaning |
| --- | --- |
| `'Not requested'` | A supported receiver was never asked (before `setJitterBufferTargetMs`). |
| `'Accepted'` | The assignment did not throw. **Never** proof of the actual buffer or lower latency. |
| `'Unsupported'` | A receiver **was observed** and genuinely lacked a `jitterBufferTarget` property. |
| `'Rejected'` | The property exists on a supported receiver but the assignment threw. |
| `'Unavailable'` | No capability was observed: the port exposed no status accessor, no receiver existed, or the feature-detect itself could not be evaluated. No capability claim is made. |

The feature-detect uses `'jitterBufferTarget' in receiver`, which checks property presence and
does **not** invoke a getter; only a `Proxy` `has` trap could throw there. A throwing `has` trap is
an **unknown** capability, not an observed absence, so it also surfaces as `'Unavailable'` — never
an invented `'Unsupported'`. `'Unsupported'` is reserved for a receiver that was actually observed
without the property. This distinguishes unsupported from unknown/error without inventing an
`'Unsupported'` claim when nothing was observed. `requested_ms` is the millisecond value last
requested, or `null` when nothing was observed.

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
