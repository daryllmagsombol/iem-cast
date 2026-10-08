import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { CounterString, MixAck, MixSnapshot, ServerEvent } from '../../protocol';
import type { ReceiverController, ReceiverSnapshot } from '../../receiver-ports';
import { createReceiverController } from '../../transport/ReceiverController';
import { MusicianRoot } from '../MusicianRoot';

afterEach(cleanup);
const mix: MixSnapshot = { catalogRevision: '1' as CounterString, mixRevision: '1' as CounterString, sources: [{ sourceId: 'vocal', gainDb: -12, muted: true }], masterDb: -18, masterMuted: true };
function fixture(phase: ReceiverSnapshot['phase'] = 'unpaired') {
  let snapshot: ReceiverSnapshot = { phase, catalog: { catalogRevision: '1' as CounterString, sources: [{ sourceId: 'vocal', physicalIndex: 0, label: 'Lead vocal', available: true, authorized: true, stereoPair: null, role: 'inputChannel' }] }, requested_mix: mix, accepted_mix: mix, applied_mix: null, master_local_muted: true, error: null, diagnostics: { network_rtt_ms: 'Unavailable', buffer_delay_ms: 'Waiting for sample', end_to_end: 'Not measured', last_updated: null } };
  let notify: (s: ReceiverSnapshot) => void = () => {};
  const controller: ReceiverController = { getSnapshot: () => snapshot, subscribe: h => { notify = h; return () => {}; }, connect: vi.fn(async () => { snapshot = { ...snapshot, phase: 'ready-muted' }; notify(snapshot); }), arm: vi.fn(async () => {}), requestMix: vi.fn(async patch => ({ acceptedRevision: '2' as CounterString, catalogRevision: patch.catalogRevision, canonicalSettings: { ...mix, ...patch, mixRevision: '2' as CounterString } })), personalMasterMute: vi.fn(), stop: vi.fn(), disconnect: vi.fn() };
  return { controller, update: (s: Partial<ReceiverSnapshot>) => { snapshot = { ...snapshot, ...s }; notify(snapshot); } };
}

test('missing controller has no inert Connect and offers a safe preview', () => {
  render(<MusicianRoot />);
  expect(screen.getByRole('heading', { name: 'Receiver unavailable' })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Connect' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Open simulated preview' })).toBeEnabled();
  expect(screen.getByText('Browser receiver')).toBeInTheDocument();
});

test('Prepare only connects; Start listening invokes arm synchronously in a separate gesture', async () => {
  const { controller } = fixture();
  render(<MusicianRoot controller={controller} />);
  fireEvent.click(screen.getByRole('button', { name: 'Prepare connection' }));
  expect(await screen.findByRole('button', { name: 'Start listening' })).toBeEnabled();
  expect(controller.arm).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Start listening' }));
  expect(controller.arm).toHaveBeenCalledTimes(1);
});

test('accepted settings are never presented as Applied; mute waits for matching DSP revision', async () => {
  const { controller, update } = fixture('armed');
  render(<MusicianRoot controller={controller} />);
  expect(screen.getAllByText('Not applied').length).toBeGreaterThan(0);
  expect(screen.queryByText(/Applied -12 dB/)).not.toBeInTheDocument();
  act(() => update({ applied_mix: mix }));
  fireEvent.click(screen.getByRole('button', { name: 'Unmute Lead vocal' }));
  await screen.findByText(/Accepted · Waiting for host application/);
  expect(screen.getByText(/Applied mute: Muted/)).toBeInTheDocument();
  act(() => update({ applied_mix: { ...mix, mixRevision: '2' as CounterString, sources: [{ sourceId: 'vocal', gainDb: -12, muted: false }] } }));
  expect(screen.getByText(/Applied mute: Not muted/)).toBeInTheDocument();
  expect(screen.queryByText(/Accepted · Waiting for host application/)).not.toBeInTheDocument();
});

test('a listener with no mix yet still gets a fader per catalog source', () => {
  // Regression: a freshly paired listener has no requested/accepted/applied mix, but the channel
  // strips were derived ONLY from the mix. With no mix the phone rendered zero faders, so the
  // listener could never create a first patch — a deadlock. Channels must come from the catalog.
  let snapshot: ReceiverSnapshot = {
    phase: 'ready-muted',
    catalog: {
      catalogRevision: '1' as CounterString,
      sources: [
        {
          sourceId: 'vocal',
          physicalIndex: 0,
          label: 'Lead vocal',
          available: true,
          authorized: true,
          stereoPair: null,
          role: 'inputChannel',
        },
      ],
    },
    requested_mix: null,
    accepted_mix: null,
    applied_mix: null,
    master_local_muted: true,
    error: null,
    diagnostics: {
      network_rtt_ms: 'Unavailable',
      buffer_delay_ms: 'Waiting for sample',
      end_to_end: 'Not measured',
      last_updated: null,
    },
  };
  const controller: ReceiverController = {
    getSnapshot: () => snapshot,
    subscribe: () => () => {},
    connect: vi.fn(async () => {}),
    arm: vi.fn(async () => {}),
    requestMix: vi.fn(),
    personalMasterMute: vi.fn(),
    stop: vi.fn(),
    disconnect: vi.fn(),
  };
  render(<MusicianRoot controller={controller} />);

  // The channel exists (so a gain can be requested) even though the host has sent no mix.
  expect(screen.getAllByText('Lead vocal').length).toBeGreaterThan(0);
  expect(screen.getByRole('slider', { name: /lead vocal/i })).toBeInTheDocument();
  // Regression: Start listening was gated on an existing mix, so a fresh listener could never
  // arm, which left the personal-output unmute permanently disabled. It must be available once the
  // connection is ready and a source exists.
  expect(screen.getByRole('button', { name: 'Start listening' })).toBeEnabled();
});

test('prepare rejection is visible rather than an unhandled promise', async () => {
  const { controller } = fixture();
  controller.connect = vi.fn().mockRejectedValue(new Error('Host is unavailable'));
  render(<MusicianRoot controller={controller} />);
  fireEvent.click(screen.getByRole('button', { name: 'Prepare connection' }));
  expect(await screen.findByText('Host is unavailable')).toBeInTheDocument();
  expect(controller.arm).not.toHaveBeenCalled();
});

test('accepted-only gain edit stays pending and a rejected edit restores the applied value', async () => {
  const { controller, update } = fixture('armed');
  render(<MusicianRoot controller={controller} />);
  act(() => update({ applied_mix: mix }));
  fireEvent.change(screen.getByRole('slider', { name: 'Lead vocal' }), { target: { value: '-24' } });
  await screen.findByText('Requested -24 dB · Pending');
  expect(screen.getByText('Applied -12 dB')).toBeInTheDocument();
  expect(screen.queryByText('Applied -24 dB')).not.toBeInTheDocument();
  controller.requestMix = vi.fn().mockRejectedValue(new Error('Source edit rejected'));
  fireEvent.change(screen.getByRole('slider', { name: 'Lead vocal' }), { target: { value: '-30' } });
  expect(await screen.findByRole('alert')).toHaveTextContent('Source edit rejected');
  expect(screen.getByRole('slider', { name: 'Lead vocal' })).toHaveValue('-12');
});

test('source loss requests local silence without auto-unmute or arm', () => {
  const { controller, update } = fixture('armed');
  render(<MusicianRoot controller={controller} />);
  // Channels are derived from the catalog, so losing the source removes its fader entirely.
  expect(screen.getByRole('slider', { name: 'Lead vocal' })).toBeInTheDocument();
  act(() => update({ catalog: { catalogRevision: '2' as CounterString, sources: [] } }));
  expect(screen.queryByRole('slider', { name: 'Lead vocal' })).not.toBeInTheDocument();
  expect(screen.getByText('No channels assigned')).toBeInTheDocument();
  expect(controller.personalMasterMute).toHaveBeenCalledWith(true);
  expect(controller.personalMasterMute).not.toHaveBeenCalledWith(false);
  expect(controller.arm).not.toHaveBeenCalled();
});

test('rejection restores latest accepted settings rather than older DSP settings', async () => {
  const { controller, update } = fixture('armed');
  render(<MusicianRoot controller={controller} />);
  act(() => update({ applied_mix: mix, accepted_mix: { ...mix, mixRevision: '2' as CounterString,
    sources: [{ sourceId: 'vocal', gainDb: -20, muted: true }] } }));
  controller.requestMix = vi.fn().mockRejectedValue(new Error('Mix was not accepted'));
  fireEvent.change(screen.getByRole('slider', { name: 'Lead vocal' }), { target: { value: '-30' } });
  expect(await screen.findByRole('alert')).toHaveTextContent('Mix was not accepted');
  expect(screen.getByRole('alert')).not.toHaveTextContent('Listening state must be checked');
  expect(screen.getByRole('slider', { name: 'Lead vocal' })).toHaveValue('-20');
  expect(screen.getByText('Applied -12 dB')).toBeInTheDocument();
});

test('starting during a pending edit does not suppress its accepted feedback', async () => {
  const { controller } = fixture('ready-muted');
  let resolve!: (value: Awaited<ReturnType<ReceiverController['requestMix']>>) => void;
  controller.requestMix = vi.fn(() => new Promise<Awaited<ReturnType<ReceiverController['requestMix']>>>(r => { resolve = r; }));
  render(<MusicianRoot controller={controller} />);
  fireEvent.change(screen.getByRole('slider', { name: 'Lead vocal' }), { target: { value: '-24' } });
  fireEvent.click(screen.getByRole('button', { name: 'Start listening' }));
  await act(async () => resolve({ acceptedRevision: '2' as CounterString,
    catalogRevision: mix.catalogRevision, canonicalSettings: { ...mix,
      mixRevision: '2' as CounterString, sources: [{ sourceId: 'vocal', gainDb: -24, muted: true }] } }));
  expect(screen.getByText('Accepted · Waiting for host application')).toBeInTheDocument();
  expect(screen.getByRole('slider', { name: 'Lead vocal' })).toHaveValue('-24');
});

test('rendered pending output unmute offers cancel and its delayed ack cannot open output', async () => {
  let emit: (event: ServerEvent) => void = () => {};
  const mute = vi.fn();
  let release!: (ack: MixAck) => void;
  const controller = createReceiverController({
    signaling: {
      onEvent: handler => { emit = handler; return () => {}; },
      connect: async () => {
        emit({ type: 'session.snapshot', payload: { phase: 'ready-muted', context: { audioEpoch: 'a' }, acceptedMix: mix } } as unknown as ServerEvent);
        emit({ type: 'catalog.snapshot', payload: fixture('armed').controller.getSnapshot().catalog } as ServerEvent);
      },
      requestMix: () => new Promise<MixAck>(resolve => { release = resolve; }),
      arm: async arm => emit({ type: 'listen.armed', payload: { armNonce: arm.armNonce, safetyGeneration: '1' } } as ServerEvent),
      disarm: async () => {},
    },
    media: { createRecvOnlyAudio: async () => {}, setJitterBufferTargetMs: async () => true,
      play: async () => {}, mute, stop: () => {} },
    activation: { isActive: () => true, onUserGesture: () => () => {} },
    stats: { refresh: async () => {}, inbound: () => undefined, selectedCandidateRtt: () => undefined },
    clock: () => 0,
  });
  await controller.arm();
  render(<MusicianRoot controller={controller} />);
  fireEvent.click(screen.getByRole('button', { name: 'Unmute personal output' }));
  expect(controller.getSnapshot().master_local_muted).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel output unmute' }));
  await act(async () => release({ acceptedRevision: '2' as CounterString, catalogRevision: mix.catalogRevision,
    canonicalSettings: { ...mix, masterMuted: false, mixRevision: '2' as CounterString } }));
  expect(mute).not.toHaveBeenCalledWith(false);
  expect(screen.getByRole('button', { name: 'Unmute personal output' })).toBeEnabled();
  expect(screen.getByText('Session armed · Personal output muted')).toBeInTheDocument();
  controller.disconnect();
});
