/**
 * Frozen Task 1 protocol contract (TypeScript mirror).
 *
 * The Rust contract keeps native structs snake_case and maps them to **camelCase wire DTOs**.
 * TypeScript consumes the wire shape, so every DTO here is camelCase and matches
 * `crates/host-core/src/contract.rs` (`#[serde(rename_all = "camelCase")]`).
 *
 * Decimal counter values stay **verbatim strings** (`CounterString`); `BigInt` is used only to
 * validate/compare and never appears in a returned structure, so `JSON.stringify` of a parsed
 * envelope stays lossless.
 *
 * `parseEnvelope` preserves the wire key `type` (not renamed to `kind`).
 */

// ---------------------------------------------------------------------------------------------
// Scalar / identity wire types
// ---------------------------------------------------------------------------------------------

/** A decimal-only u64/u32 value kept as a string verbatim. */
export type CounterString = string & { readonly __brand: 'CounterString' };

/** Opaque random UUID string (host global control identity). */
export type HostEpoch = string;
/** Opaque random UUID string (capture generation identity). */
export type AudioEpoch = string;
/** Opaque random UUID string (per-listener/connection identity). */
export type SessionEpoch = string;
/** Opaque random UUID string (fresh per arm attempt). */
export type ArmNonce = string;
/** Stable source identity (UUID string), preserved across renames. */
export type SourceId = string;
/** Opaque unique per-client request identifier. */
export type RequestId = string;

/** Listener lifecycle phase; kebab-case on the wire (`ready-muted`). */
export type ListenerPhase =
  | 'unpaired'
  | 'paired'
  | 'negotiating'
  | 'ready-muted'
  | 'armed'
  | 'interrupted'
  | 'revoked';

// ---------------------------------------------------------------------------------------------
// Capture / catalog / config
// ---------------------------------------------------------------------------------------------

/** Native sample format summary reported for a capture device. */
export type SampleFormat = 'F32' | 'I16' | 'U16';

/** Source role in the mix. */
export type SourceRole = 'inputChannel' | 'masterLr';

export interface DeviceInfo {
  deviceId: string;
  name: string;
  isDefault: boolean;
  inputChannels: number;
  sampleFormats: SampleFormat[];
  sampleRateHz: number | null;
  bufferMinFrames: number | null;
  bufferMaxFrames: number | null;
}

export interface CaptureRequest {
  deviceId: string;
  sampleRateHz: number;
  bufferFrames: number;
}

export interface InterfaceInfo {
  name: string;
  ipAddress: string;
  prefix: number;
}

export interface SourceInfo {
  sourceId: SourceId;
  physicalIndex: number;
  label: string;
  role: SourceRole;
  authorized: boolean;
  available: boolean;
  stereoPair: SourceId | null;
}

export interface CatalogSnapshot {
  catalogRevision: CounterString;
  sources: SourceInfo[];
}

export interface StartHostRequest {
  capture: CaptureRequest;
  interface: InterfaceInfo;
  certificatePath: string;
  keyPath: string;
}

export interface StartHostResult {
  hostEpoch: HostEpoch;
  audioEpoch: AudioEpoch;
  joinUrl: string;
  catalog: CatalogSnapshot;
}

// ---------------------------------------------------------------------------------------------
// Mix
// ---------------------------------------------------------------------------------------------

/** One per-source gain setting. */
export interface SourceGain {
  sourceId: SourceId;
  gainDb: number;
  muted: boolean;
}

/**
 * Personal mix snapshot (wire `settings` / `canonicalSettings`, and the receiver-domain view).
 * The native `SessionContext` is not nested on the wire.
 */
export interface MixSnapshot {
  catalogRevision: CounterString;
  mixRevision: CounterString;
  sources: SourceGain[];
  masterDb: number;
  masterMuted: boolean;
}

/** Absolute mix intent from a listener. */
export interface MixPatch {
  baseRevision: CounterString;
  catalogRevision: CounterString;
  sources: SourceGain[];
  masterDb: number;
  masterMuted: boolean;
}

/** Accepted mix acknowledgement carrying canonical settings. */
export interface MixAck {
  acceptedRevision: CounterString;
  catalogRevision: CounterString;
  canonicalSettings: MixSnapshot;
}

/** Notification that a mix revision was installed by the DSP. */
export interface MixApplied {
  appliedRevision: CounterString;
  audioEpoch: AudioEpoch;
  startSample: CounterString;
  settings: MixSnapshot;
}

// ---------------------------------------------------------------------------------------------
// Listen / session
// ---------------------------------------------------------------------------------------------

/** Wire form of the native session identity. */
export interface SessionContext {
  sessionEpoch: SessionEpoch;
  audioEpoch: AudioEpoch;
  safetyGeneration: CounterString;
}

/** Listener arm request (sessionEpoch comes from the envelope). */
export interface ListenArm {
  audioEpoch: AudioEpoch;
  safetyGeneration: CounterString;
  appliedRevision: CounterString;
  armNonce: ArmNonce;
}

/** Host confirmation that a listener is armed. */
export interface ListenArmed {
  sessionEpoch: SessionEpoch;
  safetyGeneration: CounterString;
  audioEpoch: AudioEpoch;
  armNonce: ArmNonce;
}

/** Listener disarm request. */
export interface ListenDisarm {
  sessionEpoch: SessionEpoch;
  safetyGeneration: CounterString;
}

/** Full server-side view of one listener session. */
export interface SessionSnapshot {
  sessionEpoch: SessionEpoch;
  context: SessionContext;
  phase: ListenerPhase;
  catalogRevision: CounterString;
  requestedMix: MixSnapshot | null;
  acceptedMix: MixSnapshot | null;
  appliedMix: MixSnapshot | null;
}

/** Condensed listener state for UI updates. */
export interface ListenerState {
  sessionEpoch: SessionEpoch;
  phase: ListenerPhase;
  armed: boolean;
}

// ---------------------------------------------------------------------------------------------
// Transport / diagnostics / error payloads
// ---------------------------------------------------------------------------------------------

export interface RtcAnswer {
  sdp: string;
}

export interface RtcCandidate {
  candidate: string | null;
  sdpMid?: string;
  sdpMLineIndex?: number;
}

export interface ProtocolError {
  code: string;
  message: string;
  retryable: boolean;
}

export interface Telemetry {
  audioEpoch: AudioEpoch;
  startSample: CounterString;
  sourcePeaksDbfs: Record<string, number | null>;
  outputPeaksDbfs: [number | null, number | null];
  limiterReductionDb: number | null;
}

// ---------------------------------------------------------------------------------------------
// Envelope / server event union (ARCHITECTURE §12)
// ---------------------------------------------------------------------------------------------

/** Protocol v1 envelope wrapping a complete payload. `type` is the wire message name. */
export interface EnvelopeV1<T> {
  v: number;
  type: string;
  requestId: RequestId;
  hostEpoch: HostEpoch;
  sessionEpoch: SessionEpoch | null;
  payload: T;
}

export type SessionSnapshotEvent = EnvelopeV1<SessionSnapshot> & { type: 'session.snapshot' };
export type CatalogSnapshotEvent = EnvelopeV1<CatalogSnapshot> & { type: 'catalog.snapshot' };
export type MixAckEvent = EnvelopeV1<MixAck> & { type: 'mix.ack' };
export type MixAppliedEvent = EnvelopeV1<MixApplied> & { type: 'mix.applied' };
export type ListenerStateEvent = EnvelopeV1<ListenerState> & { type: 'listener.state' };
export type ListenArmedEvent = EnvelopeV1<ListenArmed> & { type: 'listen.armed' };
export type RtcAnswerEvent = EnvelopeV1<RtcAnswer> & { type: 'rtc.answer' };
export type RtcCandidateEvent = EnvelopeV1<RtcCandidate> & { type: 'rtc.candidate' };
export type TelemetryEvent = EnvelopeV1<Telemetry> & { type: 'telemetry' };
export type ProtocolErrorEvent = EnvelopeV1<ProtocolError> & { type: 'error' };

/** A complete server event envelope; never an unvalidated partial payload. */
export type ServerEvent =
  | SessionSnapshotEvent
  | CatalogSnapshotEvent
  | MixAckEvent
  | MixAppliedEvent
  | ListenerStateEvent
  | ListenArmedEvent
  | RtcAnswerEvent
  | RtcCandidateEvent
  | TelemetryEvent
  | ProtocolErrorEvent;

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

/** Raised when an envelope or frame count fails validation. */
export class ProtocolParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ProtocolParseError';
  }
}

const MAX_U32 = 0xffffffffn;

/**
 * Counter-valued wire fields. Values are kept verbatim as strings and only validated with
 * `BigInt`; a returned structure never contains a `bigint`.
 */
const COUNTER_FIELDS: ReadonlySet<string> = new Set([
  'baseRevision',
  'catalogRevision',
  'acceptedRevision',
  'appliedRevision',
  'mixRevision',
  'safetyGeneration',
  'channelMapRevision',
  'startSample',
  'frameCount',
]);

function assertRecord(value: unknown, field: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new ProtocolParseError(`${field} must be an object`);
  }
  return value as Record<string, unknown>;
}

function requireString(value: unknown, field: string): string {
  if (typeof value !== 'string') {
    throw new ProtocolParseError(`${field} must be a string`);
  }
  return value;
}

/**
 * Validate a decimal-only counter string and keep it verbatim as a branded `CounterString`.
 * `BigInt` is used only for validation/magnitude checks.
 */
export function toCounterString(value: unknown, field: string): CounterString {
  const text = requireString(value, field);
  if (!/^[0-9]+$/.test(text)) {
    throw new ProtocolParseError(`${field} must be a decimal-only string`);
  }
  // Validate via BigInt only; the original string is preserved verbatim.
  const magnitude = BigInt(text);
  if (magnitude < 0n) {
    throw new ProtocolParseError(`${field} must be non-negative`);
  }
  return text as CounterString;
}

/**
 * Recursively brand known counter fields, preserving decimal strings verbatim. The returned
 * value is JSON-serializable (no `bigint` is ever introduced).
 */
function brandCounters(value: unknown, path: string): unknown {
  if (Array.isArray(value)) {
    return value.map((item, index) => brandCounters(item, `${path}[${index}]`));
  }
  if (typeof value === 'object' && value !== null) {
    const source = value as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(source)) {
      const child = source[key];
      out[key] = COUNTER_FIELDS.has(key)
        ? toCounterString(child, `${path}.${key}`)
        : brandCounters(child, `${path}.${key}`);
    }
    return out;
  }
  return value;
}

/**
 * Parse a complete v1 envelope. Preserves the wire key `type`, preserves decimal counter strings
 * verbatim, and never returns a `bigint`.
 *
 * The payload generic defaults to a permissive shape because callers narrow it with the frozen
 * wire DTOs (`EnvelopeV1<MixPatch>`, etc.) or the `ServerEvent` union; validation is deliberately
 * shallow and runtime-visible rather than a full schema.
 */
export function parseEnvelope<T = Record<string, any>>(input: unknown): EnvelopeV1<T> {
  const raw = assertRecord(input, 'envelope');
  const v = raw.v;
  if (typeof v !== 'number' || v !== 1) {
    throw new ProtocolParseError(`unsupported protocol version: ${String(v)}`);
  }
  const type = requireString(raw.type, 'type');
  const requestId = requireString(raw.requestId, 'requestId');
  const hostEpoch = requireString(raw.hostEpoch, 'hostEpoch');
  const sessionEpoch = raw.sessionEpoch == null
    ? null
    : requireString(raw.sessionEpoch, 'sessionEpoch');
  const payload = brandCounters(raw.payload, 'payload') as T;
  return { v, type, requestId, hostEpoch, sessionEpoch, payload };
}

/**
 * Parse a `frameCount` decimal string to a `number`, rejecting non-decimal text and values above
 * `u32::MAX` (4294967295).
 */
export function parseFrameCount(value: string): number {
  if (typeof value !== 'string' || !/^[0-9]+$/.test(value)) {
    throw new ProtocolParseError('frameCount must be a decimal-only string within the u32 range');
  }
  const magnitude = BigInt(value);
  if (magnitude > MAX_U32) {
    throw new ProtocolParseError('frameCount exceeds the u32 range');
  }
  return Number(magnitude);
}

// ---------------------------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------------------------
//
// Port and bridge interfaces are declared in `receiver-ports.ts` / `desktop-bridge/contracts.ts`
// so the LAN bundle can depend on ports without the Tauri bridge. These type-only re-exports let
// consumers import the full frozen surface from a single `protocol` entrypoint. Because they are
// `export type`, they introduce no runtime import cycle.

export type { HostBridge, PairingCredential } from '../desktop-bridge/contracts';
export type {
  ActivationPort,
  AudioMediaPort,
  CreateReceiverController,
  DiagnosticsSnapshot,
  InboundStats,
  MusicianSignaling,
  ReceiverController,
  ReceiverPorts,
  ReceiverSnapshot,
  StatsPort,
} from '../receiver-ports';
