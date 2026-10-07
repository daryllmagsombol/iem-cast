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
    const transceiver = new FakeTransceiver(init.direction, {});
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
    await vi.waitFor(() => expect(offer).toHaveBeenCalledWith('v=0\r\no=offer\r\n'));
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

  test('setJitterBufferTargetMs reports unsupported rather than throwing', async () => {
    FakePeerConnection.instances = [];
    const { signaling, offer } = fakeSignaling();
    const media = createBrowserMediaPort(signaling, mediaDeps());
    const readiness = media.createRecvOnlyAudio();
    await vi.waitFor(() => expect(offer).toHaveBeenCalled());
    // The fake receiver has no jitterBufferTarget, so the feature-detect returns false.
    await expect(media.setJitterBufferTargetMs(0)).resolves.toBe(false);
    media.stop();
    await expect(readiness).resolves.toBeUndefined();
  });
});
