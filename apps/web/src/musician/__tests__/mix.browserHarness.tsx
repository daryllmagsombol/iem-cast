import { createRoot } from 'react-dom/client';
import type { CounterString, ListenArm, MixAck, MixPatch, ServerEvent } from '../../protocol';
import { createReceiverController } from '../../transport/ReceiverController';
import { MusicianRoot } from '../MusicianRoot';

/** Real UI/controller, with a delayed host and inert media for browser acceptance tests. */
export async function mountMixReceiver() {
  let handler: (event: ServerEvent) => void = () => {};
  let revision = 0;
  let inFlight = 0;
  let rejectNext = false;
  const calls: MixPatch[] = [];
  const output: boolean[] = [];
  const arms: ListenArm[] = [];
  const pending: (() => void)[] = [];
  const push = (type: string, payload: unknown) => handler({ type, payload } as ServerEvent);
  const controller = createReceiverController({
    signaling: {
      onEvent: h => { handler = h; return () => {}; },
      connect: async () => {
        push('session.snapshot', { phase: 'ready-muted', context: { audioEpoch: 'browser-audio' } });
        push('catalog.snapshot', { catalogRevision: '1', sources: [{ sourceId: 'vocal',
          label: 'Lead vocal', physicalIndex: 0, available: true, authorized: true,
          role: 'inputChannel', stereoPair: null }] });
      },
      requestMix: patch => {
        calls.push(patch);
        inFlight++;
        return new Promise<MixAck>((resolve, reject) => pending.push(() => {
          inFlight--;
          if (rejectNext) { rejectNext = false; reject(new Error('Host refused this mix')); return; }
          if (patch.baseRevision !== String(revision)) { reject(new Error('Revision conflict')); return; }
          const acceptedRevision = String(++revision) as CounterString;
          resolve({ acceptedRevision, catalogRevision: patch.catalogRevision,
            canonicalSettings: { ...patch, mixRevision: acceptedRevision } });
        }));
      },
      arm: async arm => { arms.push(arm); },
      disarm: async () => {},
    },
    media: { createRecvOnlyAudio: async () => {}, setJitterBufferTargetMs: async () => true,
      play: async () => {}, stop: () => {}, mute: muted => { output.push(muted); } },
    activation: { isActive: () => navigator.userActivation.isActive, onUserGesture: () => () => {} },
    stats: { refresh: async () => {}, inbound: () => undefined, selectedCandidateRtt: () => undefined },
    clock: () => performance.now(),
  });
  await controller.connect();
  const node = document.createElement('div');
  document.body.replaceChildren(node);
  const root = createRoot(node);
  root.render(<MusicianRoot controller={controller} />);
  return { calls, output, arms, get inFlight() { return inFlight; },
    confirmArm: () => push('listen.armed', { armNonce: arms.at(-1)?.armNonce, safetyGeneration: '1' }),
    acknowledge: () => pending.shift()?.(), reject: () => { rejectNext = true; pending.shift()?.(); },
    snapshot: controller.getSnapshot, dispose: () => { controller.disconnect(); root.unmount(); } };
}
