/**
 * Browser musician signaling over the authenticated WSS control channel.
 *
 * Pairing is performed exactly once: the single-use token is read from the URL fragment
 * (`/join#t=<token>`), exchanged for a `Secure`/`HttpOnly`/`SameSite` cookie via
 * `POST /api/v1/pair`, then removed from the URL so it is not left exposed in the address bar or
 * history entry. The socket then opens against `wss://<host>/api/v1/ws`.
 *
 * Outbound control frames use the v1 envelope. Requests that expect a correlated reply
 * (`mix.patch` → `mix.ack`) are tracked in a **bounded** pending map with per-request timeouts so a
 * promise can never hang forever. A structured server `error` scoped to a pending request rejects
 * it; the envelope `hostEpoch`/`sessionEpoch` are learned from every inbound frame so later frames
 * carry the epochs the host expects.
 */

import { parseEnvelope } from '../protocol';
import type {
  CounterString,
  EnvelopeV1,
  HostEpoch,
  ListenArm,
  ListenDisarm,
  MixAck,
  MixPatch,
  ProtocolError,
  RequestId,
  ServerEvent,
  SessionEpoch,
} from '../protocol';
import type { MusicianSignaling } from '../receiver-ports';

/** Maximum concurrent in-flight correlated requests before the oldest is failed. */
const MAX_PENDING_REQUESTS = 32;

/** Default time a correlated request may wait for its reply before rejecting. */
const DEFAULT_REQUEST_TIMEOUT_MS = 15_000;

/** Bounded wait for the host-epoch probe before proceeding without it. */
const DEFAULT_EPOCH_PROBE_TIMEOUT_MS = 3_000;

/** A structured server protocol error surfaced as a rejection. */
export class SignalingError extends Error {
  readonly code: string;
  readonly retryable: boolean;

  constructor(code: string, message: string, retryable: boolean) {
    super(message);
    this.name = 'SignalingError';
    this.code = code;
    this.retryable = retryable;
  }
}

interface PairResponseBody {
  sessionEpoch?: unknown;
  hostEpoch?: unknown;
}

/** Injectable environment seams; production uses the browser globals. */
export interface SignalingDeps {
  /** `fetch`-compatible function; injectable for tests. */
  fetchImpl?: (input: string, init: RequestInit) => Promise<{
    ok: boolean;
    status: number;
    json(): Promise<unknown>;
  }>;
  /** WebSocket constructor; injectable for tests. */
  webSocket?: typeof WebSocket;
  /** Page location (defaults to `globalThis.location`); injectable for tests. */
  location?: Location;
  /** History (defaults to `globalThis.history`); injectable for tests. */
  history?: History;
  /** Correlated-request timeout in milliseconds. */
  requestTimeoutMs?: number;
  /** Host-epoch probe timeout in milliseconds; defaults to a short bounded wait. */
  epochProbeTimeoutMs?: number;
}

/**
 * The concrete browser signaling handle. The RTC send methods are internal seams used by the media
 * port; the public {@link MusicianSignaling} surface is unchanged.
 */
export interface BrowserMusicianSignaling extends MusicianSignaling {
  /** The authenticated session epoch, once paired. */
  currentSessionEpoch(): SessionEpoch | null;
  /** The most recent audio epoch seen from the host, if any. */
  currentAudioEpoch(): string | null;
  /** Sends the local SDP offer (`rtc.offer`). */
  sendRtcOffer(sdp: string): Promise<void>;
  /** Trickles a local ICE candidate (`rtc.candidate`); `null` signals end-of-candidates. */
  sendRtcCandidate(candidate: RTCIceCandidate | null): void;
}

interface PendingRequest {
  resolve(ack: MixAck): void;
  reject(error: unknown): void;
  timer: ReturnType<typeof setTimeout>;
}

/** Browser `crypto.randomUUID` with a defensive fallback for older runtimes. */
function randomRequestId(): RequestId {
  const cryptoApi = globalThis.crypto;
  if (cryptoApi && typeof cryptoApi.randomUUID === 'function') {
    return cryptoApi.randomUUID() as RequestId;
  }
  const template = 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx';
  const uuid = template.replace(/[xy]/g, char => {
    const random = (Math.random() * 16) | 0;
    const value = char === 'x' ? random : (random & 0x3) | 0x8;
    return value.toString(16);
  });
  return uuid as RequestId;
}

export function createMusicianSignaling(deps: SignalingDeps = {}): BrowserMusicianSignaling {
  const fetchImpl =
    deps.fetchImpl ?? ((input: string, init: RequestInit) => globalThis.fetch(input, init));
  const WebSocketCtor = deps.webSocket ?? globalThis.WebSocket;
  const loc = deps.location ?? globalThis.location;
  const historyRef = deps.history ?? globalThis.history;
  const requestTimeoutMs = deps.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
  const epochProbeTimeoutMs = deps.epochProbeTimeoutMs ?? DEFAULT_EPOCH_PROBE_TIMEOUT_MS;

  const handlers = new Set<(event: ServerEvent) => void>();
  const pending = new Map<string, PendingRequest>();

  let socket: WebSocket | null = null;
  let ready = false;
  let connectPromise: Promise<void> | null = null;
  let hostEpoch = '';
  let sessionEpoch: SessionEpoch | null = null;
  let audioEpoch: string | null = null;
  let safetyGeneration = '0';
  let probeRequestId: string | null = null;
  let hostEpochWaiters: (() => void)[] = [];

  function emit(event: ServerEvent): void {
    for (const handler of handlers) handler(event);
  }

  /** Read the single-use token from the fragment without mutating anything. */
  function readToken(): string | null {
    const hash = loc.hash.startsWith('#') ? loc.hash.slice(1) : loc.hash;
    const token = new URLSearchParams(hash).get('t');
    return token !== null && token.length > 0 ? token : null;
  }

  /** Remove the token from the URL/history so it is not left exposed. */
  function clearToken(): void {
    const url = new URL(loc.href);
    const clean = `${url.pathname}${url.search}`;
    try {
      historyRef.replaceState(null, '', clean);
    } catch {
      // A non-navigable location is not fatal; the token has still been consumed.
    }
  }

  function failPending(reason: unknown): void {
    for (const [requestId, entry] of pending) {
      clearTimeout(entry.timer);
      pending.delete(requestId);
      entry.reject(reason);
    }
  }

  function handleFrame(raw: string): void {
    let event: ServerEvent;
    try {
      event = parseEnvelope(JSON.parse(raw)) as unknown as ServerEvent;
    } catch {
      // Unparseable frames are dropped; the transport never fabricates a partial event.
      return;
    }

    if (typeof event.hostEpoch === 'string') hostEpoch = event.hostEpoch;
    if (typeof event.sessionEpoch === 'string') sessionEpoch = event.sessionEpoch;

    // The epoch probe's own reply (a `STALE_EPOCH` error) is transport plumbing, not a
    // user-facing event; recognize it before any waiter clears the probe id.
    if (probeRequestId !== null && event.requestId === probeRequestId) {
      if (hostEpoch !== '' && hostEpochWaiters.length > 0) {
        const waiters = hostEpochWaiters;
        hostEpochWaiters = [];
        for (const waiter of waiters) waiter();
      }
      return;
    }

    // The host requires an exact `hostEpoch` on every frame, but `/pair` returns only the session.
    // Any reply that carries the real epoch releases waiters so later frames can be sent.
    if (hostEpoch !== '' && hostEpochWaiters.length > 0) {
      const waiters = hostEpochWaiters;
      hostEpochWaiters = [];
      for (const waiter of waiters) waiter();
    }

    switch (event.type) {
      case 'listen.armed': {
        const payload = event.payload as { audioEpoch?: string; safetyGeneration?: string };
        if (typeof payload.audioEpoch === 'string') audioEpoch = payload.audioEpoch;
        if (typeof payload.safetyGeneration === 'string') safetyGeneration = payload.safetyGeneration;
        break;
      }
      case 'session.snapshot': {
        const payload = event.payload as {
          context?: { audioEpoch?: string; safetyGeneration?: string };
        };
        if (typeof payload.context?.audioEpoch === 'string') audioEpoch = payload.context.audioEpoch;
        if (typeof payload.context?.safetyGeneration === 'string') {
          safetyGeneration = payload.context.safetyGeneration;
        }
        break;
      }
      case 'mix.ack': {
        const entry = pending.get(event.requestId);
        if (entry) {
          clearTimeout(entry.timer);
          pending.delete(event.requestId);
          entry.resolve(event.payload as MixAck);
        }
        break;
      }
      case 'error': {
        const payload = event.payload as ProtocolError;
        const entry = pending.get(event.requestId);
        if (entry) {
          clearTimeout(entry.timer);
          pending.delete(event.requestId);
          entry.reject(new SignalingError(payload.code, payload.message, payload.retryable));
        }
        break;
      }
      default:
        break;
    }

    emit(event);
  }

  function writeFrame(type: string, requestId: string, payload: unknown): void {
    if (!socket) throw new Error('Signaling is not connected');
    const envelope: EnvelopeV1<unknown> = {
      v: 1,
      type,
      requestId,
      hostEpoch,
      sessionEpoch,
      payload,
    };
    socket.send(JSON.stringify(envelope));
  }

  function openSocket(): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const scheme = loc.protocol === 'http:' ? 'ws:' : 'wss:';
      const next = new WebSocketCtor(`${scheme}//${loc.host}/api/v1/ws`);
      socket = next;
      let settled = false;

      next.onopen = () => {
        settled = true;
        ready = true;
        resolve();
      };
      next.onerror = () => {
        if (!settled) {
          settled = true;
          ready = false;
          reject(new Error('Signaling socket failed'));
        }
      };
      next.onclose = () => {
        if (!settled) {
          settled = true;
          reject(new Error('Signaling socket closed before it was ready'));
        }
        ready = false;
        socket = null;
        failPending(new Error('Signaling connection closed'));
      };
      next.onmessage = event => {
        if (typeof event.data === 'string') handleFrame(event.data);
      };
    });
  }

  async function pair(token: string): Promise<void> {
    const response = await fetchImpl('/api/v1/pair', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token }),
    });
    if (!response.ok) {
      throw new Error(`Pairing was rejected (status ${response.status})`);
    }
    let body: PairResponseBody = {};
    try {
      const parsed = await response.json();
      if (typeof parsed === 'object' && parsed !== null) body = parsed as PairResponseBody;
    } catch {
      body = {};
    }
    if (typeof body.sessionEpoch === 'string') sessionEpoch = body.sessionEpoch as SessionEpoch;
    if (typeof body.hostEpoch === 'string') hostEpoch = body.hostEpoch as HostEpoch;
  }

  /**
   * Learn the host epoch when `/pair` did not return it.
   *
   * The host rejects a frame whose `hostEpoch` does not match, replying with `STALE_EPOCH` in an
   * envelope that carries the real epoch. One bounded `heartbeat` probe (an ARCHITECTURE §12 client
   * type) is enough to adopt it. The probe is best-effort: a timeout resolves without an epoch and
   * the caller fails naturally rather than hanging.
   */
  async function probeHostEpoch(probeTimeoutMs: number): Promise<void> {
    if (hostEpoch !== '' || !socket || !ready) return;
    const requestId = randomRequestId();
    probeRequestId = requestId;
    await new Promise<void>(resolve => {
      let settled = false;
      const finish = (): void => {
        if (settled) return;
        settled = true;
        probeRequestId = null;
        clearTimeout(timer);
        resolve();
      };
      const timer = setTimeout(finish, probeTimeoutMs);
      hostEpochWaiters.push(finish);
      try {
        writeFrame('heartbeat', requestId, {});
      } catch {
        finish();
      }
    });
  }

  async function connect(): Promise<void> {
    if (ready && socket) return;
    if (connectPromise) return connectPromise;
    connectPromise = (async () => {
      try {
        const token = readToken();
        if (token !== null) {
          await pair(token);
          clearToken();
        }
        await openSocket();
        // Adopt the host epoch if `/pair` did not provide one, so control frames are accepted.
        await probeHostEpoch(epochProbeTimeoutMs);
      } finally {
        connectPromise = null;
      }
    })();
    return connectPromise;
  }

  return {
    connect,

    async requestMix(patch: MixPatch): Promise<MixAck> {
      await connect();
      const requestId = randomRequestId();
      return new Promise<MixAck>((resolve, reject) => {
        if (pending.size >= MAX_PENDING_REQUESTS) {
          const oldest = pending.keys().next().value;
          if (oldest !== undefined) {
            const stale = pending.get(oldest);
            pending.delete(oldest);
            if (stale) {
              clearTimeout(stale.timer);
              stale.reject(new Error('Too many pending mix requests'));
            }
          }
        }
        const timer = setTimeout(() => {
          pending.delete(requestId);
          reject(new Error('Mix request timed out'));
        }, requestTimeoutMs);
        pending.set(requestId, { resolve, reject, timer });
        try {
          writeFrame('mix.patch', requestId, patch);
        } catch (error) {
          clearTimeout(timer);
          pending.delete(requestId);
          reject(error);
        }
      });
    },

    async arm(arm: ListenArm): Promise<void> {
      await connect();
      writeFrame('listen.arm', randomRequestId(), arm);
    },

    async disarm(): Promise<void> {
      await connect();
      const disarm: ListenDisarm = {
        sessionEpoch: (sessionEpoch ?? '') as SessionEpoch,
        safetyGeneration: safetyGeneration as CounterString,
      };
      writeFrame('listen.disarm', randomRequestId(), disarm);
    },

    onEvent(handler) {
      handlers.add(handler);
      return () => {
        handlers.delete(handler);
      };
    },

    currentSessionEpoch() {
      return sessionEpoch;
    },

    currentAudioEpoch() {
      return audioEpoch;
    },

    async sendRtcOffer(sdp: string): Promise<void> {
      await connect();
      writeFrame('rtc.offer', randomRequestId(), { sdp });
    },

    sendRtcCandidate(candidate: RTCIceCandidate | null): void {
      if (!socket || !ready) return;
      writeFrame('rtc.candidate', randomRequestId(), {
        candidate: candidate ? candidate.candidate : null,
        sdpMid: candidate?.sdpMid ?? undefined,
        sdpMLineIndex: candidate?.sdpMLineIndex ?? undefined,
      });
    },
  };
}
