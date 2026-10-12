/**
 * Frozen Task 1 receiver ports and controller contract.
 *
 * The LAN musician bundle is created through injected ports so it never imports `HostBridge` or
 * `@tauri-apps/api`. The concrete `createReceiverController` implementation lives in
 * `apps/web/src/transport/ReceiverController.ts` (Task 6); this module declares only the frozen
 * interfaces it consumes and returns.
 */

import type {
  CatalogSnapshot,
  CounterString,
  ListenerPhase,
  MixAck,
  MixPatch,
  MixSnapshot,
  ListenArm,
  ServerEvent,
} from './protocol';

// ---------------------------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------------------------

/** Authenticated musician WSS signaling. */
export interface MusicianSignaling {
  connect(signal?: AbortSignal): Promise<void>;
  requestMix(patch: MixPatch, signal?: AbortSignal, requestId?: string): Promise<MixAck>;
  /** Optional correlation id lets the controller identify an arm's host error. */
  arm(arm: ListenArm, requestId?: string, signal?: AbortSignal): Promise<void>;
  disarm(): Promise<void>;
  onEvent(handler: (ev: ServerEvent) => void): () => void;
}

/**
 * Outcome of the browser's latest jitter-buffer target preference.
 *
 * `jitterBufferTarget` is a **preference**, not a cap: `'Accepted'` only means the browser
 * accepted the assignment for later consideration — it never means the actual buffer is that
 * value or that latency is reduced. `'Unsupported'` is reserved for a receiver that was actually
 * observed without the property; a port with no status accessor, no receiver, or a feature-detect
 * that could not be evaluated reports `'Unavailable'` rather than inventing an `'Unsupported'`
 * claim.
 */
export type JitterBufferPreferenceState =
  | 'Not requested'
  | 'Accepted'
  | 'Unsupported'
  | 'Rejected'
  | 'Unavailable';

export interface JitterBufferPreference {
  state: JitterBufferPreferenceState;
  /** The millisecond value last requested, or `null` before any request. */
  requested_ms: number | null;
}

/**
 * Raw feature-detect/assignment outcome reported by the concrete media port.
 *
 * `undefined` (from the optional accessor) means the port did not expose the capability at all —
 * an older or fake port — and is surfaced as `'Unavailable'`, never an invented `'Unsupported'`.
 * `supported: false` is only set when a receiver was actually observed, so it maps to a real
 * `'Unsupported'`; no receiver, or an unevaluatable feature-detect, leaves the status `undefined`.
 */
export interface JitterBufferPreferenceStatus {
  /** Whether the receiver actually exposed a `jitterBufferTarget` property. */
  supported: boolean;
  /** The millisecond value last requested, or `null` before any request. */
  requestedMs: number | null;
  /** Whether the assignment did not throw. Never proof of the actual browser buffer. */
  accepted: boolean;
}

/** Browser WebRTC receive-only audio plus media element control. */
export interface AudioMediaPort {
  createRecvOnlyAudio(signal?: AbortSignal): Promise<void>;
  setJitterBufferTargetMs(ms: number): Promise<boolean>;
  play(): Promise<void>;
  stop(): void;
  mute(muted: boolean): void;
  /**
   * Optional preference accessor. Concrete browser ports report the real feature-detect and
   * assignment outcome; older/fake ports may omit it entirely, in which case the controller
   * reports the preference as `'Unavailable'` rather than inventing an `'Unsupported'` claim.
   */
  jitterBufferStatus?(): JitterBufferPreference;
}

/** Trusted user-gesture source, required before opening the sticky local gate. */
export interface ActivationPort {
  onUserGesture(handler: () => void): () => void;
  /** Whether a current, still-trusted user gesture is available. */
  isActive(): boolean;
}

/** A normalized browser inbound-RTP sample. `delay` is in seconds. */
export interface InboundStats {
  sessionEpoch: string;
  statsId: string;
  stream: string;
  epoch: string;
  emitted: number;
  delay: number;
  sampledAtMs: number;
  /**
   * Optional raw browser fields, carried only when finite and present. Absent stays absent
   * (`undefined`) rather than `0`; each is normalized to seconds/`jitterBufferEmittedCount`
   * deltas by the diagnostics helpers, independently of the required fields.
   */
  targetDelay?: number;
  minimumDelay?: number;
  packetsLost?: number;
  packetsReceived?: number;
  concealedSamples?: number;
  totalSamplesReceived?: number;
}

/** WebRTC media statistics; reads cached normalized samples refreshed via `refresh()`. */
export interface StatsPort {
  selectedCandidateRtt(): { currentRoundTripTime: number } | undefined;
  inbound(): InboundStats | undefined;
  /** Collects `getStats()` asynchronously before cached samples are read; never overlaps. */
  refresh(): Promise<void>;
}

/** All external dependencies of a receiver controller. */
export interface ReceiverPorts {
  signaling: MusicianSignaling;
  media: AudioMediaPort;
  activation: ActivationPort;
  clock: () => number;
  stats: StatsPort;
}

// ---------------------------------------------------------------------------------------------
// Snapshot / diagnostics
// ---------------------------------------------------------------------------------------------

/**
 * One-second-refresh mobile latency diagnostics.
 *
 * Missing values are the explicit `'Unavailable'` label (never fabricated `0`); an awaiting first
 * interval is `'Waiting for sample'`; end-to-end is always `'Not measured'` unless a physical
 * measurement exists.
 */
export interface DiagnosticsSnapshot {
  network_rtt_ms: number | 'Unavailable';
  buffer_delay_ms: number | 'Unavailable' | 'Waiting for sample';
  /** Latest-interval average browser target jitter-buffer hold, in ms. */
  buffer_target_delay_ms: number | 'Unavailable' | 'Waiting for sample';
  /** Latest-interval minimum achievable jitter-buffer delay, in ms. */
  buffer_minimum_delay_ms: number | 'Unavailable' | 'Waiting for sample';
  /** Latest-interval lost-packet count; a genuine unchanged count is `0`. */
  packets_lost_delta: number | 'Unavailable' | 'Waiting for sample';
  /** Latest-interval concealment percentage; a genuine interval with none is `0`. */
  concealment_percent: number | 'Unavailable' | 'Waiting for sample';
  /** Whether the port reported the jitter-buffer target preference and its outcome. */
  jitter_buffer_preference: JitterBufferPreference;
  end_to_end: 'Not measured';
  last_updated: number | null;
}

/** Immutable view of the receiver delivered to subscribers. */
export interface ReceiverSnapshot {
  phase: ListenerPhase;
  catalog: CatalogSnapshot;
  requested_mix: MixSnapshot | null;
  accepted_mix: MixSnapshot | null;
  applied_mix: MixSnapshot | null;
  master_local_muted: boolean;
  /** Explicit output-unmute intent awaiting host acceptance; local output is still muted. */
  master_unmute_pending?: boolean;
  diagnostics: DiagnosticsSnapshot;
  error: string | null;
}

// ---------------------------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------------------------

/** Receiver lifecycle controller consumed by the phone UI. */
export interface ReceiverController {
  subscribe(handler: (snapshot: ReceiverSnapshot) => void): () => void;
  getSnapshot(): ReceiverSnapshot;
  connect(): Promise<void>;
  /** Resolves on accepted (canonical ack), never on DSP-applied. */
  requestMix(patch: MixPatch): Promise<MixAck>;
  /** Invokes `media.play()` in the gesture; resolves only on matching host confirmation. */
  arm(): Promise<void>;
  personalMasterMute(muted: boolean): void;
  stop(): void;
  disconnect(): void;
}

/** Ports-based factory; concrete implementation is Task 6. */
export type CreateReceiverController = (ports: ReceiverPorts) => ReceiverController;

/** Re-exported for callers that need the branded counter type alongside the controller. */
export type { CounterString };
