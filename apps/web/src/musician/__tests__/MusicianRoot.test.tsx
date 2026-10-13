import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import { MusicianRoot } from '../MusicianRoot';
import type { ReceiverController, ReceiverSnapshot } from '../../receiver-ports';
import type { MixPatch } from '../../protocol';

/**
 * Capture the props `MusicianRoot` forwards to `ReceiverView` so we can assert the pan/pair
 * callbacks and the seeded template without depending on designer-rendered visuals (Task 6).
 */
const viewProps: { current: import('../ReceiverView').ReceiverViewProps | null } = { current: null };
vi.mock('../ReceiverView', () => ({
  ReceiverView: (props: import('../ReceiverView').ReceiverViewProps) => {
    viewProps.current = props;
    return <div data-testid="receiver-view" />;
  },
}));

function mockController(snapshot: ReceiverSnapshot): ReceiverController {
  return {
    subscribe: () => () => {},
    getSnapshot: () => snapshot,
    connect: async () => {},
    requestMix: async () => ({ acceptedRevision: '1', catalogRevision: '1', canonicalSettings: {} }) as never,
    arm: async () => {},
    personalMasterMute: () => {},
    stop: () => {},
    disconnect: () => {},
  };
}

function diag() {
  return {
    network_rtt_ms: 'Unavailable' as const,
    buffer_delay_ms: 'Unavailable' as const,
    buffer_target_delay_ms: 'Unavailable' as const,
    buffer_minimum_delay_ms: 'Unavailable' as const,
    packets_lost_delta: 'Unavailable' as const,
    concealment_percent: 'Unavailable' as const,
    jitter_buffer_preference: { state: 'Not requested' as const, requested_ms: null },
    end_to_end: 'Not measured' as const,
    last_updated: null,
  };
}

/** A two-source catalog with distinct default pans for seeding and pair edits. */
function catalogSources() {
  return [
    { sourceId: 'left', physicalIndex: 0, label: 'Left', role: 'inputChannel' as const, authorized: true, available: true, stereoPair: 'right', stereoSide: 'left' as const, defaultPan: 0.35 },
    { sourceId: 'right', physicalIndex: 1, label: 'Right', role: 'inputChannel' as const, authorized: true, available: true, stereoPair: 'left', stereoSide: 'right' as const, defaultPan: 0.35 },
  ];
}

function snapshot(overrides: Partial<ReceiverSnapshot> = {}): ReceiverSnapshot {
  return {
    phase: 'armed',
    catalog: { catalogRevision: '1' as never, sources: catalogSources() },
    requested_mix: null,
    accepted_mix: null,
    applied_mix: null,
    master_local_muted: true,
    diagnostics: diag(),
    error: null,
    ...overrides,
  };
}

test('unpaired_root_shows_join_gate_and_never_arms_implicitly', () => {
  render(
    <MusicianRoot
      controller={mockController({
        phase: 'unpaired',
        catalog: { catalogRevision: '0' as never, sources: [] },
        requested_mix: null,
        accepted_mix: null,
        applied_mix: null,
        master_local_muted: true,
        diagnostics: diag(),
        error: null,
      })}
    />,
  );
  expect(screen.getByText(/waiting for pairing/i)).toBeInTheDocument();
  expect(screen.queryByText(/^listening$/i)).not.toBeInTheDocument();
});

test('first edit seeds the shared template defaults (nonzero pan) and adjusts only the edit', async () => {
  const calls: MixPatch[] = [];
  const controller = mockController(snapshot());
  controller.requestMix = (patch => {
    calls.push(patch);
    return Promise.resolve({ acceptedRevision: '2' as never, catalogRevision: patch.catalogRevision,
      canonicalSettings: { ...patch, mixRevision: '2' as never } });
  }) as ReceiverController['requestMix'];

  render(<MusicianRoot controller={controller} />);
  const props = viewProps.current!;
  // No mix yet: the rendered template must carry the catalog default pan and safe mute/gain.
  expect(props.channels.map(c => c.requestedDb)).toEqual([-60, -60]);
  expect(props.channels.every(c => c.muted)).toBe(true);

  // Simulate a first mono pan edit through the forwarded callback, then a gain edit.
  props.onChannelPanChange('left', -0.25);
  await screen.findByTestId('receiver-view');
  await vi.waitFor(() => expect(calls.length).toBeGreaterThan(0));
  const patch = calls.at(-1)!;
  // The explicit pan edit lands on the left member only; the right keeps the seeded default.
  expect(patch.sources).toEqual([
    { sourceId: 'left', gainDb: -60, muted: true, pan: -0.25 },
    { sourceId: 'right', gainDb: -60, muted: true, pan: 0.35 },
  ]);
});

test('onChannelPanChange updates only that source pan, leaving gain/mute/master/arm untouched', async () => {
  const calls: MixPatch[] = [];
  const controller = mockController(snapshot({
    accepted_mix: { catalogRevision: '1' as never, mixRevision: '1' as never, masterDb: -18, masterMuted: false,
      sources: [
        { sourceId: 'left', gainDb: -12, muted: false, pan: 0 },
        { sourceId: 'right', gainDb: -6, muted: true, pan: 0 },
      ] },
  }));
  controller.requestMix = (patch => {
    calls.push(patch);
    return Promise.resolve({ acceptedRevision: '2' as never, catalogRevision: patch.catalogRevision,
      canonicalSettings: { ...patch, mixRevision: '2' as never } });
  }) as ReceiverController['requestMix'];

  render(<MusicianRoot controller={controller} />);
  viewProps.current!.onChannelPanChange('left', 0.5);
  await vi.waitFor(() => expect(calls.length).toBe(1));
  const patch = calls[0];
  expect(patch.masterDb).toBe(-18);
  expect(patch.masterMuted).toBe(false);
  expect(patch.sources).toEqual([
    { sourceId: 'left', gainDb: -12, muted: false, pan: 0.5 },
    { sourceId: 'right', gainDb: -6, muted: true, pan: 0 },
  ]);
});

test('onPairBalanceChange updates both members atomically, preserving independent gain and mute', async () => {
  const calls: MixPatch[] = [];
  const controller = mockController(snapshot({
    accepted_mix: { catalogRevision: '1' as never, mixRevision: '1' as never, masterDb: -18, masterMuted: true,
      sources: [
        { sourceId: 'left', gainDb: -12, muted: false, pan: 0 },
        { sourceId: 'right', gainDb: -6, muted: true, pan: 0 },
      ] },
  }));
  controller.requestMix = (patch => {
    calls.push(patch);
    return Promise.resolve({ acceptedRevision: '2' as never, catalogRevision: patch.catalogRevision,
      canonicalSettings: { ...patch, mixRevision: '2' as never } });
  }) as ReceiverController['requestMix'];

  render(<MusicianRoot controller={controller} />);
  viewProps.current!.onPairBalanceChange('left', 'right', -0.75);
  await vi.waitFor(() => expect(calls.length).toBe(1));
  expect(calls[0].sources).toEqual([
    { sourceId: 'left', gainDb: -12, muted: false, pan: -0.75 },
    { sourceId: 'right', gainDb: -6, muted: true, pan: -0.75 },
  ]);
});

test('a single-member gain edit preserves both members pan', async () => {
  const calls: MixPatch[] = [];
  const controller = mockController(snapshot({
    accepted_mix: { catalogRevision: '1' as never, mixRevision: '1' as never, masterDb: -18, masterMuted: true,
      sources: [
        { sourceId: 'left', gainDb: -12, muted: false, pan: 0.4 },
        { sourceId: 'right', gainDb: -6, muted: true, pan: 0.4 },
      ] },
  }));
  controller.requestMix = (patch => {
    calls.push(patch);
    return Promise.resolve({ acceptedRevision: '2' as never, catalogRevision: patch.catalogRevision,
      canonicalSettings: { ...patch, mixRevision: '2' as never } });
  }) as ReceiverController['requestMix'];

  render(<MusicianRoot controller={controller} />);
  viewProps.current!.onChannelGainChange('left', -3);
  await vi.waitFor(() => expect(calls.length).toBe(1));
  expect(calls[0].sources).toEqual([
    { sourceId: 'left', gainDb: -3, muted: false, pan: 0.4 },
    { sourceId: 'right', gainDb: -6, muted: true, pan: 0.4 },
  ]);
});

test('an invalid pair intent emits no half edit', async () => {
  const calls: MixPatch[] = [];
  const controller = mockController(snapshot({
    accepted_mix: { catalogRevision: '1' as never, mixRevision: '1' as never, masterDb: -18, masterMuted: true,
      sources: [
        { sourceId: 'left', gainDb: -12, muted: false, pan: 0 },
        { sourceId: 'right', gainDb: -6, muted: true, pan: 0 },
      ] },
  }));
  controller.requestMix = (patch => {
    calls.push(patch);
    return Promise.resolve({ acceptedRevision: '2' as never, catalogRevision: patch.catalogRevision,
      canonicalSettings: { ...patch, mixRevision: '2' as never } });
  }) as ReceiverController['requestMix'];

  render(<MusicianRoot controller={controller} />);
  const props = viewProps.current!;
  // Same id, a non-reciprocal peer, and an unknown peer are all rejected before any edit.
  props.onPairBalanceChange('left', 'left', 0.5);
  props.onPairBalanceChange('left', 'unknown', 0.5);
  await new Promise(r => setTimeout(r, 0));
  expect(calls).toHaveLength(0);
});
