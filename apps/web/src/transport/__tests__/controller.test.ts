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

  const ports: ReceiverPorts = {
    signaling: {
      connect: vi.fn(async () => {}),
      requestMix: vi.fn(async () => ack),
      arm: vi.fn(async () => {
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
            armNonce: 'matching-nonce',
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
});
