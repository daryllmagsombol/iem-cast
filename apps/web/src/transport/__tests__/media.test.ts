import { describe, expect, test, vi } from 'vitest';

import type { BrowserMusicianSignaling } from '../signaling';
import { createBrowserMediaPort } from '../media';

/** A fake `RTCRtpTransceiver` recording the direction it was created with. */
class FakeTransceiver {
  direction: string;
  readonly receiver: { jitterBufferTarget?: number };
  stop = vi.fn();

  constructor(direction: string, receiver: { jitterBufferTarget?: number }) {
    this.direction = direction;
    this.receiver = receiver;
  }
}

/** A fake `RTCPeerConnection` that records transceivers and drives readiness events. */
class FakePeerConnection {
  static instances: FakePeerConnection[] = [];
  /** Receiver object used for the next created transceiver; `{}` means no jitterBufferTarget. */
  static nextReceiver: { jitterBufferTarget?: number } = {};

  readonly transceivers: FakeTransceiver[] = [];
  connectionState: RTCPeerConnectionState = 'new';
  iceConnectionState: RTCIceConnectionState = 'new';
  localDescription: { sdp: string } | null = null;

  constructor() {
    FakePeerConnection.instances.push(this);
  }

  onconnectionstatechange: (() => void) | null = null;
  oniceconnectionstatechange: (() => void) | null = null;
  onicecandidate: ((event: { candidate: unknown }) => void) | null = null;
  ontrack: ((event: { streams: unknown[] }) => void) | null = null;

  addTransceiver(kind: string, init: { direction: string }): FakeTransceiver {
    expect(kind).toBe('audio');
    const transceiver = new FakeTransceiver(init.direction, FakePeerConnection.nextReceiver);
    this.transceivers.push(transceiver);
    return transceiver;
  }

  async createOffer(): Promise<{ type: 'offer'; sdp: string }> {
    return { type: 'offer', sdp: 'v=0\r\no=offer\r\n' };
  }

  async setLocalDescription(description: { sdp: string }): Promise<void> {
    this.localDescription = { sdp: description.sdp };
  }

  async setRemoteDescription(_description: unknown): Promise<void> {
    // no-op
  }

  async addIceCandidate(_candidate: unknown): Promise<void> {
    // no-op
  }

  async getStats(): Promise<Map<string, unknown>> {
    return new Map();
  }

  close = vi.fn(() => {
    this.connectionState = 'closed';
  });

  /** Simulate the browser reaching a live connection. */
  connect(): void {
    this.connectionState = 'connected';
    this.onconnectionstatechange?.();
  }
}

function fakeSignaling() {
  const offer = vi.fn(async () => {});
  const candidate = vi.fn();
  const signaling = {
    connect: vi.fn(async () => {}),
    requestMix: vi.fn(),
    arm: vi.fn(),
    disarm: vi.fn(),
    onEvent: vi.fn(() => () => {}),
    currentSessionEpoch: () => null,
    currentAudioEpoch: () => null,
    sendRtcOffer: offer,
    sendRtcCandidate: candidate,
  } as unknown as BrowserMusicianSignaling;
  return { signaling, offer, candidate };
}

function mediaDeps() {
  return {
    rtcPeerConnection: FakePeerConnection as unknown as typeof RTCPeerConnection,
    document: globalThis.document,
    connectionTimeoutMs: 5000,
  };
}

describe('browser media port', () => {
  test.each(['offer', 'description'] as const)('stop during delayed %s work cannot send a canceled RTC offer', async boundary => {
    let release!: () => void;
    const deferred = new Promise<void>(r => { release = r; });
    const spy = boundary === 'offer'
      ? vi.spyOn(FakePeerConnection.prototype, 'createOffer').mockImplementationOnce(async () => {
        await deferred; return { type: 'offer', sdp: 'canceled' };
      })
      : vi.spyOn(FakePeerConnection.prototype, 'setLocalDescription').mockImplementationOnce(async () => { await deferred; });
    const { signaling, offer } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    try {
      const pending = media.createRecvOnlyAudio().catch(() => undefined);
      await vi.waitFor(() => expect(spy).toHaveBeenCalled());
      media.stop();
      release();
      await pending;
      expect(offer).not.toHaveBeenCalled();
      expect(media.peerConnection()).toBeNull();
    } finally { spy.mockRestore(); }
  });

  test('creates exactly one recvonly audio transceiver and never calls getUserMedia', async () => {
    FakePeerConnection.instances = [];
    const getUserMedia = vi.fn();
    (globalThis.navigator as unknown as { mediaDevices: unknown }).mediaDevices = { getUserMedia };
    const { signaling, offer } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());

    const readiness = media.createRecvOnlyAudio();
    const pc = FakePeerConnection.instances.at(-1)!;
    expect(pc.transceivers).toHaveLength(1);
    expect(pc.transceivers[0].direction).toBe('recvonly');
    await vi.waitFor(() => expect(offer).toHaveBeenCalledWith('v=0\r\no=offer\r\n', undefined));
    expect(getUserMedia).not.toHaveBeenCalled();

    pc.connect();
    await expect(readiness).resolves.toBeUndefined();
  });

  test('stop closes the peer connection and clears playback state', async () => {
    FakePeerConnection.instances = [];
    const { signaling, offer } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    const readiness = media.createRecvOnlyAudio();
    const pc = FakePeerConnection.instances.at(-1)!;
    await vi.waitFor(() => expect(offer).toHaveBeenCalled());

    media.stop();

    expect(pc.close).toHaveBeenCalled();
    expect(media.peerConnection()).toBeNull();
    // A stop while connecting resolves rather than leaving an unhandled rejection.
    await expect(readiness).resolves.toBeUndefined();
  });

  test('a receiver without the property is Unsupported rather than throwing', async () => {
    FakePeerConnection.instances = [];
    FakePeerConnection.nextReceiver = {};
    const { signaling, offer } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    const readiness = media.createRecvOnlyAudio();
    await vi.waitFor(() => expect(offer).toHaveBeenCalled());
    // The fake receiver has no jitterBufferTarget, so the feature-detect returns false.
    await expect(media.setJitterBufferTargetMs(0)).resolves.toBe(false);
    expect(media.jitterBufferStatus?.()).toEqual({ state: 'Unsupported', requested_ms: 0 });
    media.stop();
    await expect(readiness).resolves.toBeUndefined();
  });

  test('an absent browser status is Unavailable, not an invented unsupported claim', () => {
    const { signaling } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    expect(media.jitterBufferStatus?.()).toEqual({ state: 'Unavailable', requested_ms: null });
  });

  test('a request with no receiver is Unavailable, not an invented unsupported claim', async () => {
    const { signaling } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    // No transceiver/receiver exists yet: nothing was observed, so no capability claim is made.
    await expect(media.setJitterBufferTargetMs(0)).resolves.toBe(false);
    expect(media.jitterBufferStatus?.()).toEqual({ state: 'Unavailable', requested_ms: null });
  });

  test('a feature-detect has-trap that throws is Unavailable, not an invented Unsupported', async () => {
    FakePeerConnection.instances = [];
    // `'x' in receiver` does not invoke getters; only a Proxy `has` trap can throw here. That is an
    // unknown capability, not an observed absence, so it must not surface as Unsupported.
    const throwingHas = new Proxy(
      {},
      {
        has() {
          throw new Error('SecurityError');
        },
      },
    );
    FakePeerConnection.nextReceiver = throwingHas as { jitterBufferTarget?: number };
    const { signaling, offer } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    const readiness = media.createRecvOnlyAudio();
    await vi.waitFor(() => expect(offer).toHaveBeenCalled());
    await expect(media.setJitterBufferTargetMs(0)).resolves.toBe(false);
    expect(media.jitterBufferStatus?.()).toEqual({ state: 'Unavailable', requested_ms: null });
    media.stop();
    await expect(readiness).resolves.toBeUndefined();
  });

  test('a supported receiver accepts the preference and records Accepted (never a forced cap)', async () => {
    FakePeerConnection.instances = [];
    FakePeerConnection.nextReceiver = {};
    const { signaling, offer } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    const readiness = media.createRecvOnlyAudio();
    const pc = FakePeerConnection.instances.at(-1)!;
    await vi.waitFor(() => expect(offer).toHaveBeenCalled());
    // Simulate a receiver that surfaces the property.
    Object.defineProperty(pc.transceivers[0].receiver, 'jitterBufferTarget', {
      configurable: true,
      writable: true,
      value: 500,
    });
    await expect(media.setJitterBufferTargetMs(0)).resolves.toBe(true);
    expect(media.jitterBufferStatus?.()).toEqual({ state: 'Accepted', requested_ms: 0 });
    media.stop();
    await expect(readiness).resolves.toBeUndefined();
  });

  test('an assignment whose setter/getter path throws is Rejected, not Unsupported', async () => {
    FakePeerConnection.instances = [];
    // Only a getter exists, so the `in` check passes but the strict-mode assignment throws.
    const throwing: { jitterBufferTarget?: number } = {};
    Object.defineProperty(throwing, 'jitterBufferTarget', {
      configurable: true,
      get() {
        throw new Error('SecurityError');
      },
    });
    FakePeerConnection.nextReceiver = throwing;
    const { signaling, offer } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    const readiness = media.createRecvOnlyAudio();
    await vi.waitFor(() => expect(offer).toHaveBeenCalled());
    await expect(media.setJitterBufferTargetMs(0)).resolves.toBe(false);
    expect(media.jitterBufferStatus?.()).toEqual({ state: 'Rejected', requested_ms: 0 });
    media.stop();
    await expect(readiness).resolves.toBeUndefined();
  });

  test('an assignment that throws is Rejected, and readback does not claim the actual buffer', async () => {
    FakePeerConnection.instances = [];
    const readonly: { jitterBufferTarget?: number } = {};
    Object.defineProperty(readonly, 'jitterBufferTarget', {
      configurable: true,
      get: () => 300,
      set() {
        throw new Error('NotAllowedError');
      },
    });
    FakePeerConnection.nextReceiver = readonly;
    const { signaling, offer } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    const readiness = media.createRecvOnlyAudio();
    await vi.waitFor(() => expect(offer).toHaveBeenCalled());
    await expect(media.setJitterBufferTargetMs(0)).resolves.toBe(false);
    expect(media.jitterBufferStatus?.()).toEqual({ state: 'Rejected', requested_ms: 0 });
    media.stop();
    await expect(readiness).resolves.toBeUndefined();
  });
});
