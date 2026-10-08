import { describe, expect, test, vi } from 'vitest';

import type {
  AudioMediaPort,
  InboundStats,
  ReceiverPorts,
  StatsPort,
} from '../../receiver-ports';
import type { MixAck, MixPatch, ServerEvent } from '../../protocol';
import type { CounterString } from '../../protocol';
import { createReceiverController } from '../ReceiverController';

/** Test helper: wire counters are branded strings, so cast literal values once. */
const counter = (value: string): CounterString => value as CounterString;

function patchGain(db: number): MixPatch {
  return {
    baseRevision: counter('1'),
    catalogRevision: counter('1'),
    sources: [{ sourceId: 'ch1', gainDb: db, muted: false }],
    masterDb: 0,
    masterMuted: false,
  };
}

const ack: MixAck = {
  acceptedRevision: counter('12'),
  catalogRevision: counter('1'),
  canonicalSettings: {
    catalogRevision: counter('1'),
    mixRevision: counter('12'),
    sources: [{ sourceId: 'ch1', gainDb: -9, muted: false }],
    masterDb: 0,
    masterMuted: false,
  },
};

interface FakeOptions {
  gesture?: boolean;
  armed?: boolean;
  /** Host pushes delivered while `connect()` is still in flight. */
  connectSnapshots?: ServerEvent[];
}

/** A `session.snapshot` carrying the host's real audio epoch in its context. */
function sessionSnapshotEvent(audioEpoch: string): ServerEvent {
  return {
    v: 1,
    type: 'session.snapshot',
    requestId: 'r',
    hostEpoch: 'h',
    sessionEpoch: 's',
    payload: {
      sessionEpoch: 's',
      context: { sessionEpoch: 's', audioEpoch, safetyGeneration: '0' },
      phase: 'ready-muted',
      catalogRevision: '1',
      requestedMix: null,
      acceptedMix: null,
      appliedMix: null,
    },
  } as unknown as ServerEvent;
}

/** A `catalog.snapshot` with one authorized, available source. */
function catalogSnapshotEvent(): ServerEvent {
  return {
    v: 1,
    type: 'catalog.snapshot',
    requestId: 'r',
    hostEpoch: 'h',
    sessionEpoch: 's',
    payload: {
      catalogRevision: '1',
      sources: [
        {
          sourceId: 'ch1',
          physicalIndex: 0,
          label: 'Channel 1',
          role: 'inputChannel',
          authorized: true,
          available: true,
          stereoPair: null,
        },
      ],
    },
  } as unknown as ServerEvent;
}

function fakePorts(options: FakeOptions = {}) {
  let handler: ((ev: ServerEvent) => void) | null = null;
  const media = {
    createRecvOnlyAudio: vi.fn(async () => {}),
    setJitterBufferTargetMs: vi.fn(async () => true),
    play: vi.fn(async () => {}),
    stop: vi.fn(() => {}),
    mute: vi.fn((_muted: boolean) => {}),
  } satisfies AudioMediaPort & Record<string, unknown>;

  const stats = {
    selectedCandidateRtt: () => undefined,
    inbound: (): InboundStats | undefined => undefined,
    refresh: async () => {},
  } satisfies StatsPort;

  /** Snapshots the host pushes the instant the socket opens, before connect() resolves. */
  const onConnect: ServerEvent[] = options.connectSnapshots ?? [];

  const ports: ReceiverPorts = {
    signaling: {
      connect: vi.fn(async () => {
        // Deliver the host's initial pushes *during* connect: the controller must already be
        // listening or they are lost and the phone stays "Not paired" with no sources.
        for (const event of onConnect) handler?.(event);
      }),
      requestMix: vi.fn(async () => ack),
      arm: vi.fn(async arm => {
        handler?.({
          v: 1,
          type: 'listen.armed',
          requestId: 'r',
          hostEpoch: 'h',
          sessionEpoch: 's',
          payload: {
            sessionEpoch: 's',
            safetyGeneration: '1',
            audioEpoch: 'a',
            armNonce: arm.armNonce,
          },
        } as unknown as ServerEvent);
      }),
      disarm: vi.fn(async () => {}),
      onEvent: (h: (ev: ServerEvent) => void) => {
        handler = h;
        return () => {
          handler = null;
        };
      },
    },
    media,
    activation: {
      onUserGesture: () => () => {},
      isActive: () => options.gesture ?? true,
    },
    clock: () => 1000,
    stats,
  };

  return { ports, media, emit: (ev: ServerEvent) => handler?.(ev) };
}

describe('receiver controller safety gate', () => {
  test.each(['stop', 'disconnect'] as const)('delayed connection cannot set up media or restore phase after %s', async action => {
    const { ports, media } = fakePorts();
    let release!: () => void;
    vi.mocked(ports.signaling.connect).mockImplementationOnce(() => new Promise<void>(r => { release = r; }));
    const controller = createReceiverController(ports);
    const pending = controller.connect().catch(() => undefined);
    controller[action]();
    const stopped = controller.getSnapshot();
    release();
    await pending;
    expect(media.createRecvOnlyAudio).not.toHaveBeenCalled();
    expect(media.setJitterBufferTargetMs).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toEqual(stopped);
    controller.disconnect();
  });

  test.each(['media', 'jitter'] as const)('cancellation during %s setup prevents later setup and phase mutation', async boundary => {
    const { ports, media } = fakePorts();
    let release!: () => void;
    if (boundary === 'media') media.createRecvOnlyAudio.mockImplementationOnce(() => new Promise<void>(r => { release = r; }));
    else media.setJitterBufferTargetMs.mockImplementationOnce(() => new Promise<boolean>(r => { release = () => r(true); }));
    const controller = createReceiverController(ports);
    const pending = controller.connect().catch(() => undefined);
    await vi.waitFor(() => expect(release).toBeDefined());
    controller.disconnect();
    const stopped = controller.getSnapshot();
    release();
    await pending;
    if (boundary === 'media') expect(media.setJitterBufferTargetMs).not.toHaveBeenCalled();
    expect(controller.getSnapshot()).toEqual(stopped);
  });

  test.each(['promise', 'push'] as const)('old canceled mix %s cannot overwrite newer authoritative session settings', async delivery => {
    const { ports, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    let release!: (ack: MixAck) => void;
    vi.mocked(ports.signaling.requestMix).mockImplementationOnce(() => new Promise<MixAck>(r => { release = r; }));
    const controller = createReceiverController(ports);
    await controller.connect();
    const pending = controller.requestMix(patchGain(-9)).catch(() => undefined);
    controller.stop();
    const newer = accept(patchGain(-3), '20').canonicalSettings;
    const session = sessionSnapshotEvent('a');
    emit({ ...session, payload: { ...session.payload, requestedMix: newer, acceptedMix: newer } } as ServerEvent);
    const authoritative = controller.getSnapshot();
    const observer = vi.fn();
    controller.subscribe(observer);
    if (delivery === 'push') emit({ type: 'mix.ack', requestId: 'old-request', sessionEpoch: 's', hostEpoch: 'h', payload: ack } as ServerEvent);
    release(ack);
    await pending;
    await new Promise(r => setTimeout(r, 0));
    expect(controller.getSnapshot()).toEqual(authoritative);
    expect(observer).not.toHaveBeenCalled();
    controller.disconnect();
  });

  test('unrelated error before enqueue survives pending mix and acceptance', async () => {
    const { ports, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    const controller = createReceiverController(ports);
    await controller.connect();
    emit({ type: 'error', requestId: 'other', payload: { message: 'Host device unavailable' } } as ServerEvent);
    let release!: (ack: MixAck) => void;
    vi.mocked(ports.signaling.requestMix).mockImplementationOnce(() => new Promise<MixAck>(r => { release = r; }));
    const pending = controller.requestMix(patchGain(-9));
    expect(controller.getSnapshot().error).toBe('Host device unavailable');
    release(ack);
    await pending;
    expect(controller.getSnapshot().error).toBe('Host device unavailable');
    controller.disconnect();
  });

  test('a new authoritative session with no mix clears old settings and ignores old completion', async () => {
    const { ports, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    const controller = createReceiverController(ports);
    await controller.connect();
    await controller.requestMix(patchGain(-9));
    let release!: (ack: MixAck) => void;
    vi.mocked(ports.signaling.requestMix).mockImplementationOnce(() => new Promise<MixAck>(r => { release = r; }));
    const pending = controller.requestMix(patchGain(-3)).catch(() => undefined);
    emit({ ...sessionSnapshotEvent('new-audio'), sessionEpoch: 'new-session' } as ServerEvent);
    expect(controller.getSnapshot().accepted_mix).toBeNull();
    expect(controller.getSnapshot().requested_mix).toBeNull();
    const authoritative = controller.getSnapshot();
    release(ack);
    await pending;
    expect(controller.getSnapshot()).toEqual(authoritative);
    controller.disconnect();
  });

  test('late timed-out confirmation updates host generation only, so retry uses current generation', async () => {
    vi.useFakeTimers();
    try {
      const { ports, emit, media } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
      vi.mocked(ports.signaling.arm).mockResolvedValue(undefined);
      const controller = createReceiverController(ports);
      const first = controller.arm().catch(() => undefined);
      await vi.advanceTimersByTimeAsync(1);
      const sent = vi.mocked(ports.signaling.arm).mock.calls[0][0];
      await vi.advanceTimersByTimeAsync(10000);
      await first;
      emit({ type: 'listen.armed', sessionEpoch: 's', hostEpoch: 'h', payload: { armNonce: sent.armNonce, safetyGeneration: '7' } } as ServerEvent);
      expect(controller.getSnapshot().phase).toBe('ready-muted');
      expect(media.mute).not.toHaveBeenCalledWith(false);
      const retry = controller.arm().catch(() => undefined);
      await vi.advanceTimersByTimeAsync(1);
      expect(vi.mocked(ports.signaling.arm).mock.calls[1][0].safetyGeneration).toBe('7');
      controller.disconnect();
      await retry;
    } finally { vi.useRealTimers(); }
  });

  test('authoritative session generation is used for arm', async () => {
    const session = sessionSnapshotEvent('a');
    const { ports } = fakePorts({ connectSnapshots: [{ ...session, payload: { ...session.payload,
      context: { audioEpoch: 'a', safetyGeneration: '6' } } } as ServerEvent] });
    const controller = createReceiverController(ports);
    await controller.arm();
    expect(vi.mocked(ports.signaling.arm).mock.calls[0][0].safetyGeneration).toBe('6');
    controller.disconnect();
  });

  test('host armed snapshot after timeout updates authority without granting local arm permission', async () => {
    vi.useFakeTimers();
    try {
      const { ports, emit, media } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
      vi.mocked(ports.signaling.arm).mockResolvedValue(undefined);
      const controller = createReceiverController(ports);
      const first = controller.arm().catch(() => undefined);
      await vi.advanceTimersByTimeAsync(10001);
      await first;
      const session = sessionSnapshotEvent('a');
      emit({ ...session, payload: { ...session.payload, phase: 'armed',
        context: { audioEpoch: 'a', safetyGeneration: '8' } } } as ServerEvent);
      expect(controller.getSnapshot().phase).toBe('ready-muted');
      controller.personalMasterMute(false);
      expect(media.mute).not.toHaveBeenCalledWith(false);
      const retry = controller.arm().catch(() => undefined);
      await vi.advanceTimersByTimeAsync(1);
      expect(vi.mocked(ports.signaling.arm).mock.calls[1][0].safetyGeneration).toBe('8');
      controller.disconnect();
      await retry;
    } finally { vi.useRealTimers(); }
  });

  function accept(patch: MixPatch, revision: string): MixAck {
    return { acceptedRevision: counter(revision), catalogRevision: patch.catalogRevision,
      canonicalSettings: { ...patch, mixRevision: counter(revision) } };
  }

  test('serializes and coalesces edits onto the latest accepted revision', async () => {
    const { ports } = fakePorts();
    let first!: (ack: MixAck) => void;
    const send = vi.mocked(ports.signaling.requestMix);
    send.mockImplementationOnce(() => new Promise(resolve => { first = resolve; }))
      .mockImplementation(async patch => accept(patch, '14'));
    const controller = createReceiverController(ports);
    const one = controller.requestMix(patchGain(-9));
    const two = controller.requestMix({ ...patchGain(-6), masterDb: -3 });
    const three = controller.requestMix({ ...patchGain(-4), masterDb: -3 });
    expect(send).toHaveBeenCalledTimes(1);
    first(accept(patchGain(-9), '13'));
    await Promise.all([one, two, three]);
    expect(send).toHaveBeenCalledTimes(2);
    expect(send.mock.calls[1][0]).toMatchObject({ baseRevision: '13', masterDb: -3,
      sources: [{ sourceId: 'ch1', gainDb: -4, muted: false }] });
  });

  test('arm waits for a pending mix acceptance, not its requested echo', async () => {
    const { ports } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    let resolve!: (ack: MixAck) => void;
    vi.mocked(ports.signaling.requestMix).mockImplementation(() => new Promise(r => { resolve = r; }));
    const controller = createReceiverController(ports);
    await controller.connect();
    const mix = controller.requestMix(patchGain(-9));
    const arm = controller.arm();
    await new Promise(r => setTimeout(r, 0));
    expect(ports.signaling.arm).not.toHaveBeenCalled();
    resolve(accept(patchGain(-9), '13'));
    await Promise.all([mix, arm]);
    expect(ports.signaling.arm).toHaveBeenCalledWith(expect.objectContaining({ appliedRevision: '13' }), expect.any(String), expect.any(AbortSignal));
    controller.disconnect();
  });

  test('rejected intent rolls requested state back to accepted state', async () => {
    const { ports } = fakePorts();
    const controller = createReceiverController(ports);
    await controller.requestMix(patchGain(-9));
    vi.mocked(ports.signaling.requestMix).mockRejectedValueOnce(new Error('Revision conflict'));
    await expect(controller.requestMix(patchGain(-3))).rejects.toThrow('Revision conflict');
    expect(controller.getSnapshot().requested_mix).toEqual(controller.getSnapshot().accepted_mix);
  });

  test('a later queued edit survives rejection of the in-flight edit', async () => {
    const { ports } = fakePorts();
    const controller = createReceiverController(ports);
    await controller.requestMix(patchGain(-9));
    let reject!: (error: Error) => void;
    const send = vi.mocked(ports.signaling.requestMix);
    send.mockImplementationOnce(() => new Promise((_, r) => { reject = r; }))
      .mockImplementation(async patch => accept(patch, '13'));
    const failed = controller.requestMix(patchGain(-6));
    const failure = expect(failed).rejects.toThrow('Rejected');
    const later = controller.requestMix({ ...patchGain(-3), masterDb: -4 });
    reject(new Error('Rejected'));
    await failure;
    await later;
    expect(send.mock.calls[2][0]).toMatchObject({ baseRevision: '12', masterDb: -4 });
    expect(controller.getSnapshot().requested_mix?.sources[0].gainDb).toBe(-3);
    expect(controller.getSnapshot().error).toBeNull();
  });

  test.each(['stop', 'disconnect'] as const)('canceling arm during mix wait with %s discards queued intent', async action => {
    const { ports } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    const controller = createReceiverController(ports);
    await controller.connect();
    let resolve!: (ack: MixAck) => void;
    const send = vi.mocked(ports.signaling.requestMix);
    send.mockImplementationOnce(() => new Promise(r => { resolve = r; }));
    const mix = controller.requestMix(patchGain(-9));
    const queued = controller.requestMix(patchGain(-3));
    const arm = controller.arm();
    const armResult = arm.then(() => 'resolved', () => 'canceled');
    const queuedResult = queued.then(() => 'resolved', () => 'canceled');
    const mixResult = mix.then(() => 'resolved', () => 'canceled');
    await new Promise(r => setTimeout(r, 0));
    controller[action]();
    // Teardown settles callers even though the already-sent host request is still delayed.
    expect(await armResult).toBe('canceled');
    expect(await queuedResult).toBe('canceled');
    expect(await mixResult).toBe('canceled');
    resolve(accept(patchGain(-9), '13'));
    await new Promise(r => setTimeout(r, 0));
    expect(send).toHaveBeenCalledTimes(1);
    expect(ports.signaling.arm).not.toHaveBeenCalled();
    expect(controller.getSnapshot().phase).not.toBe('armed');
    controller.disconnect();
  });

  test('arm stays pending after send until matching confirmation and repeated start cannot replace it', async () => {
    const { ports, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    vi.mocked(ports.signaling.arm).mockResolvedValue(undefined);
    const controller = createReceiverController(ports);
    let settled = false;
    const arm = controller.arm().then(() => { settled = true; });
    await vi.waitFor(() => expect(ports.signaling.arm).toHaveBeenCalledTimes(1));
    expect(settled).toBe(false);
    const sent = vi.mocked(ports.signaling.arm).mock.calls[0][0];
    const repeated = controller.arm().catch(() => undefined);
    await new Promise(r => setTimeout(r, 0));
    expect(ports.signaling.arm).toHaveBeenCalledTimes(1);
    emit({ type: 'listen.armed', payload: { armNonce: 'unrelated', safetyGeneration: '1' } } as ServerEvent);
    expect(settled).toBe(false);
    emit({ type: 'listen.armed', payload: { armNonce: sent.armNonce, safetyGeneration: '1' } } as ServerEvent);
    await Promise.all([arm, repeated]);
    expect(settled).toBe(true);
    expect(controller.getSnapshot().phase).toBe('armed');
    controller.disconnect();
  });

  test('arm confirmation has a bounded timeout and late confirmation cannot arm', async () => {
    vi.useFakeTimers();
    try {
      const { ports, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
      vi.mocked(ports.signaling.arm).mockResolvedValue(undefined);
      const controller = createReceiverController(ports);
      const result = controller.arm().then(() => 'resolved', e => e.message as string);
      await vi.advanceTimersByTimeAsync(1);
      const sent = vi.mocked(ports.signaling.arm).mock.calls[0][0];
      await vi.advanceTimersByTimeAsync(15000);
      expect(await result).toMatch(/timed out/i);
      emit({ type: 'listen.armed', payload: { armNonce: sent.armNonce, safetyGeneration: '1' } } as ServerEvent);
      expect(controller.getSnapshot().phase).not.toBe('armed');
      controller.disconnect();
    } finally { vi.useRealTimers(); }
  });

  test('stop rejects a sent arm awaiting confirmation and ignores its late confirmation', async () => {
    const { ports, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    vi.mocked(ports.signaling.arm).mockResolvedValue(undefined);
    const controller = createReceiverController(ports);
    const result = controller.arm().then(() => 'resolved', e => e.message as string);
    await vi.waitFor(() => expect(ports.signaling.arm).toHaveBeenCalledTimes(1));
    const sent = vi.mocked(ports.signaling.arm).mock.calls[0][0];
    controller.stop();
    expect(await result).toMatch(/canceled/i);
    emit({ type: 'listen.armed', payload: { armNonce: sent.armNonce, safetyGeneration: '1' } } as ServerEvent);
    expect(controller.getSnapshot().phase).toBe('ready-muted');
    controller.disconnect();
  });

  test('mix success does not clear an unrelated error received after mix rejection', async () => {
    const { ports, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    const controller = createReceiverController(ports);
    await controller.connect();
    let reject!: (error: Error) => void;
    let resolve!: (ack: MixAck) => void;
    const send = vi.mocked(ports.signaling.requestMix);
    send.mockImplementationOnce(() => new Promise((_, r) => { reject = r; }))
      .mockImplementationOnce(() => new Promise(r => { resolve = r; }));
    const one = controller.requestMix(patchGain(-9)).catch(() => undefined);
    const two = controller.requestMix(patchGain(-3));
    reject(new Error('Mix rejected'));
    await one;
    emit({ type: 'error', requestId: 'other-action', payload: { message: 'Another action failed' } } as ServerEvent);
    resolve(accept(patchGain(-3), '13'));
    await two;
    expect(controller.getSnapshot().error).toBe('Another action failed');
    controller.disconnect();
  });

  test('only a correlated host error rejects a pending arm', async () => {
    const { ports, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    vi.mocked(ports.signaling.arm).mockResolvedValue(undefined);
    const controller = createReceiverController(ports);
    let settled = false;
    const result = controller.arm().then(() => 'resolved', e => { settled = true; return e.message; });
    await vi.waitFor(() => expect(ports.signaling.arm).toHaveBeenCalledTimes(1));
    emit({ type: 'error', requestId: 'unrelated', payload: { message: 'Unrelated failure' } } as ServerEvent);
    expect(settled).toBe(false);
    const sent = vi.mocked(ports.signaling.arm).mock.calls[0][0];
    emit({ type: 'error', requestId: sent.armNonce, payload: { message: 'Arm was refused' } } as ServerEvent);
    expect(await result).toBe('Arm was refused');
    expect(controller.getSnapshot().phase).not.toBe('armed');
    controller.disconnect();
  });

  test('muting during delayed explicit unmute invalidates its permission to open output', async () => {
    const { ports, media, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a')] });
    vi.mocked(ports.signaling.arm).mockImplementation(async arm => {
      emit({ type: 'listen.armed', payload: { armNonce: arm.armNonce, safetyGeneration: '1' } } as ServerEvent);
    });
    const controller = createReceiverController(ports);
    await controller.arm();
    let resolve!: (ack: MixAck) => void;
    vi.mocked(ports.signaling.requestMix).mockImplementationOnce(() => new Promise(r => { resolve = r; }));
    controller.personalMasterMute(false);
    controller.personalMasterMute(true);
    resolve(ack);
    await new Promise(r => setTimeout(r, 0));
    expect(controller.getSnapshot().master_local_muted).toBe(true);
    expect(media.mute).not.toHaveBeenCalledWith(false);
    controller.disconnect();
  });

  test.each([false, true])('fresh arm and explicit output unmute releases host mute safely (reject=%s)', async reject => {
    const { ports, media, emit } = fakePorts({ connectSnapshots: [sessionSnapshotEvent('a'), catalogSnapshotEvent()] });
    let revision = 0;
    vi.mocked(ports.signaling.requestMix).mockImplementation(async patch => accept(patch, String(++revision)));
    vi.mocked(ports.signaling.arm).mockImplementation(async arm => {
      emit({ type: 'listen.armed', payload: { armNonce: arm.armNonce, safetyGeneration: '1' } } as ServerEvent);
    });
    const controller = createReceiverController(ports);
    await controller.arm();
    expect(controller.getSnapshot().accepted_mix?.masterMuted).toBe(true);
    await controller.requestMix({ ...patchGain(-9), masterMuted: true });
    expect(controller.getSnapshot().master_local_muted).toBe(true);
    if (reject) vi.mocked(ports.signaling.requestMix).mockRejectedValueOnce(new Error('Host refused output'));
    controller.personalMasterMute(false);
    await vi.waitFor(() => expect(ports.signaling.requestMix).toHaveBeenCalledTimes(3));
    await vi.waitFor(() => expect(controller.getSnapshot().master_local_muted).toBe(reject));
    expect(vi.mocked(ports.signaling.requestMix).mock.calls[2][0]).toMatchObject({ masterMuted: false, baseRevision: '2' });
    if (reject) {
      expect(media.mute).not.toHaveBeenCalledWith(false);
      expect(controller.getSnapshot().error).toContain('Host refused output');
      expect(controller.getSnapshot().requested_mix?.masterMuted).toBe(true);
    } else expect(controller.getSnapshot().accepted_mix?.masterMuted).toBe(false);
    controller.disconnect();
  });

  test('arming_requires_explicit_gesture', async () => {
    const { ports } = fakePorts({ gesture: false });
    const controller = createReceiverController(ports);
    await expect(controller.arm()).rejects.toThrow(/gesture/i);
  });

  test('arm_invokes_media_play_from_gesture_before_awaiting_host', async () => {
    const { ports, media } = fakePorts({ gesture: true });
    const controller = createReceiverController(ports);
    await controller.requestMix(patchGain(-9)).catch(() => undefined);
    const pending = controller.arm();
    // play() is invoked synchronously as part of the gesture sequence.
    expect(media.play).toHaveBeenCalled();
    await pending.catch(() => undefined);
  });

  test('mute_true_is_always_local_immediate_even_when_unarmed', () => {
    const { ports, media } = fakePorts({ armed: false });
    const controller = createReceiverController(ports);
    controller.personalMasterMute(true);
    expect(media.mute).toHaveBeenCalledWith(true);
    expect(controller.getSnapshot().master_local_muted).toBe(true);
  });

  test('unmute_false_without_armed_session_stays_muted', () => {
    const { ports, media } = fakePorts({ gesture: true });
    const controller = createReceiverController(ports);
    controller.personalMasterMute(false);
    expect(media.mute).toHaveBeenCalledWith(true);
    expect(controller.getSnapshot().master_local_muted).toBe(true);
  });

  test('stop_is_immediate_without_ack_and_invalidates_nonce', () => {
    const { ports, media } = fakePorts();
    const controller = createReceiverController(ports);
    controller.stop();
    expect(media.stop).toHaveBeenCalled();
    expect(media.mute).toHaveBeenCalledWith(true);
    expect(controller.getSnapshot().phase).toBe('ready-muted');
  });

  test('acknowledgement_never_opens_the_sticky_gate', () => {
    const { ports, emit } = fakePorts();
    const controller = createReceiverController(ports);
    emit({
      v: 1,
      type: 'mix.applied',
      requestId: 'r',
      hostEpoch: 'h',
      sessionEpoch: 's',
      payload: {
        appliedRevision: '12',
        audioEpoch: 'a',
        startSample: '0',
        settings: ack.canonicalSettings,
      },
    } as unknown as ServerEvent);
    expect(controller.getSnapshot().master_local_muted).toBe(true);
  });

  test('stale_arm_nonce_reply_is_ignored', () => {
    const { ports, emit } = fakePorts();
    const controller = createReceiverController(ports);
    // No arm attempt is pending, so any reply must be ignored.
    emit({
      v: 1,
      type: 'listen.armed',
      requestId: 'r',
      hostEpoch: 'h',
      sessionEpoch: 's',
      payload: {
        sessionEpoch: 's',
        safetyGeneration: '1',
        audioEpoch: 'a',
        armNonce: 'old-nonce',
      },
    } as unknown as ServerEvent);
    expect(controller.getSnapshot().phase).not.toBe('armed');
    expect(controller.getSnapshot().master_local_muted).toBe(true);
  });

  test('request_mix_resolves_on_accepted_not_on_dsp_applied', async () => {
    const { ports } = fakePorts();
    const controller = createReceiverController(ports);
    const result = await controller.requestMix(patchGain(-9));
    expect(result.acceptedRevision).toBe('12');
    // Accepted is recorded; applied is still absent.
    expect(controller.getSnapshot().accepted_mix).not.toBeNull();
    expect(controller.getSnapshot().applied_mix).toBeNull();
  });

  test('initial_snapshots_pushed_during_connect_are_not_lost', async () => {
    // The host pushes `session.snapshot` and `catalog.snapshot` the instant the socket opens.
    // If the controller subscribes only after connect() resolves, those pushes are dropped and
    // the phone is left with no catalog ("No sources available") forever.
    const { ports } = fakePorts({
      gesture: true,
      connectSnapshots: [sessionSnapshotEvent('audio-epoch-abc'), catalogSnapshotEvent()],
    });
    const controller = createReceiverController(ports);

    await controller.connect();

    const snap = controller.getSnapshot();
    expect(snap.catalog.sources).toHaveLength(1);
    expect(snap.catalog.sources[0].available).toBe(true);
  });

  test('arm_sends_the_host_audio_epoch_not_a_catalog_revision', async () => {
    // Regression: `arm()` used to send `catalogRevision` as `audioEpoch`. The host compares the
    // audio epoch against its own and rejects a mismatch with STALE_EPOCH, so arming failed.
    const { ports } = fakePorts({
      gesture: true,
      connectSnapshots: [sessionSnapshotEvent('audio-epoch-abc'), catalogSnapshotEvent()],
    });
    const armSpy = ports.signaling.arm as unknown as ReturnType<typeof vi.fn>;
    const controller = createReceiverController(ports);

    await controller.requestMix(patchGain(-9)).catch(() => undefined);
    await controller.arm().catch(() => undefined);

    expect(armSpy).toHaveBeenCalled();
    const sent = armSpy.mock.calls[0][0] as { audioEpoch: string };
    expect(sent.audioEpoch).toBe('audio-epoch-abc');
    // The catalog revision must never be passed off as an audio epoch.
    expect(sent.audioEpoch).not.toBe('1');
  });

  test('a fresh listener can arm without having moved any fader', async () => {
    // Regression: `arm()` required an accepted/requested mix, but a freshly paired listener has
    // neither until they touch a control. Tapping Start listening without moving a fader threw
    // "No accepted mix is available to arm" — a deadlock for the normal happy path.
    const { ports } = fakePorts({
      gesture: true,
      connectSnapshots: [sessionSnapshotEvent('audio-epoch-abc'), catalogSnapshotEvent()],
    });
    const armSpy = ports.signaling.arm as unknown as ReturnType<typeof vi.fn>;
    const mixSpy = ports.signaling.requestMix as unknown as ReturnType<typeof vi.fn>;
    const controller = createReceiverController(ports);

    await controller.connect();
    // No requestMix call: the listener simply pressed Start listening.
    await expect(controller.arm()).resolves.toBeUndefined();
    // Arming must establish a mix first: the host only produces frames for a listener it has a
    // mix for, so without this the session would be armed but silent.
    expect(mixSpy).toHaveBeenCalledTimes(1);
    const patch = mixSpy.mock.calls[0][0] as { sources: { sourceId: string; muted: boolean }[] };
    expect(patch.sources).toHaveLength(1);
    expect(patch.sources[0].muted).toBe(true); // neutral + safe: nothing audible until unmuted
    expect(armSpy).toHaveBeenCalledTimes(1);
    const sent = armSpy.mock.calls[0][0] as { audioEpoch: string; appliedRevision: string };
    expect(sent.audioEpoch).toBe('audio-epoch-abc');
    // The arm must reference the revision the host just accepted, not an invented one.
    expect(sent.appliedRevision).toBe('12');
  });
});
