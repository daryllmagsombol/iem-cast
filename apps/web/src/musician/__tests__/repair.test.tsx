import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { CounterString, MixSnapshot } from '../../protocol';
import type { ReceiverController, ReceiverSnapshot } from '../../receiver-ports';
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

test('source loss disables edits and requests local silence without auto-unmute or arm', () => {
  const { controller, update } = fixture('armed');
  render(<MusicianRoot controller={controller} />);
  act(() => update({ catalog: { catalogRevision: '2' as CounterString, sources: [] } }));
  expect(screen.getByRole('slider', { name: 'vocal' })).toBeDisabled();
  expect(controller.personalMasterMute).toHaveBeenCalledWith(true);
  expect(controller.personalMasterMute).not.toHaveBeenCalledWith(false);
  expect(controller.arm).not.toHaveBeenCalled();
});
