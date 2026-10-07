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
  connect(): Promise<void>;
  requestMix(patch: MixPatch): Promise<MixAck>;
  arm(arm: ListenArm): Promise<void>;
  disarm(): Promise<void>;
  onEvent(handler: (ev: ServerEvent) => void): () => void;
}

/** Browser WebRTC receive-only audio plus media element control. */
export interface AudioMediaPort {
  createRecvOnlyAudio(): Promise<void>;
  setJitterBufferTargetMs(ms: number): Promise<boolean>;
  play(): Promise<void>;
  stop(): void;
  mute(muted: boolean): void;
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
  /** Invokes `media.play()` from the gesture before awaiting host confirmation. */
  arm(): Promise<void>;
  personalMasterMute(muted: boolean): void;
  stop(): void;
  disconnect(): void;
}

/** Ports-based factory; concrete implementation is Task 6. */
export type CreateReceiverController = (ports: ReceiverPorts) => ReceiverController;

/** Re-exported for callers that need the branded counter type alongside the controller. */
export type { CounterString };
