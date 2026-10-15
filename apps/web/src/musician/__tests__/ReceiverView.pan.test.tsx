import '@testing-library/jest-dom/vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { ReceiverView } from '../ReceiverView';
import type { ChannelStripState } from '../../ui/ChannelStrip';
import type { SourceInfo } from '../../protocol';
import type { DiagnosticsSnapshot } from '../../receiver-ports';

const diagnostics: DiagnosticsSnapshot = {
  network_rtt_ms: 'Unavailable',
  buffer_delay_ms: 'Waiting for sample',
  buffer_target_delay_ms: 'Unavailable',
  buffer_minimum_delay_ms: 'Unavailable',
  packets_lost_delta: 'Unavailable',
  concealment_percent: 'Unavailable',
  jitter_buffer_preference: { state: 'Not requested', requested_ms: null },
  end_to_end: 'Not measured',
  last_updated: null,
};

function source(overrides: Partial<SourceInfo> & { sourceId: string; label: string }): SourceInfo {
  return {
    physicalIndex: 0,
    role: 'inputChannel',
    authorized: true,
    available: true,
    stereoPair: null,
    stereoSide: null,
    defaultPan: 0,
    ...overrides,
  };
}

function channel(sourceId: string, label: string, overrides: Partial<ChannelStripState> = {}): ChannelStripState {
  return { sourceId, label, requestedDb: -12, appliedDb: -12, muted: false, pan: 0, ...overrides };
}

function renderView(overrides: Partial<Parameters<typeof ReceiverView>[0]> = {}) {
  const props = {
    phase: 'armed' as const,
    catalog: [] as SourceInfo[],
    channels: [] as ChannelStripState[],
    masterRequestedDb: -12,
    masterMuted: false,
    onMasterMuteChange: () => {},
    onMasterGainChange: () => {},
    onChannelGainChange: () => {},
    onChannelMuteChange: () => {},
    onChannelPanChange: () => {},
    onPairBalanceChange: () => {},
    onStop: () => {},
    diagnostics,
    meters: {},
    error: null,
    wakeLockWarning: null,
    ...overrides,
  };
  const result = render(<ReceiverView {...props} />);
  return { ...result, props };
}

test('mono_source_renders_its_own_pan_calling_onChannelPanChange_with_normalized_value', () => {
  const onChannelPanChange = vi.fn();
  const catalog = [source({ sourceId: 'vocal', label: 'Lead vocal', defaultPan: 0.2 })];
  renderView({ catalog, channels: [channel('vocal', 'Lead vocal', { pan: 0.2 })], onChannelPanChange });

  const pan = screen.getByRole('slider', { name: 'Lead vocal pan' });
  expect(pan).toHaveValue('20');
  fireEvent.change(pan, { target: { value: '-30' } });
  expect(onChannelPanChange).toHaveBeenCalledWith('vocal', -0.3);
});

test('valid_reciprocal_pair_renders_two_member_strips_and_exactly_one_shared_balance', () => {
  const onChannelPanChange = vi.fn();
  const onPairBalanceChange = vi.fn();
  const catalog = [
    source({ sourceId: 'L', label: 'Keys L', stereoPair: 'R', stereoSide: 'left', defaultPan: -0.4 }),
    source({ sourceId: 'R', label: 'Keys R', stereoPair: 'L', stereoSide: 'right', defaultPan: -0.4 }),
  ];
  renderView({
    catalog,
    channels: [channel('L', 'Keys L', { pan: -0.4 }), channel('R', 'Keys R', { pan: -0.4 })],
    onChannelPanChange,
    onPairBalanceChange,
  });

  // Two independent member strips keep their own gain slider and mute control.
  expect(screen.getAllByRole('slider', { name: 'Keys L' })).toHaveLength(1);
  expect(screen.getAllByRole('slider', { name: 'Keys R' })).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Mute Keys L' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Mute Keys R' })).toBeInTheDocument();
  // Labels are authoritative side names, not adjacency heuristics.
  expect(screen.getByText('Left · Keys L')).toBeInTheDocument();
  expect(screen.getByText('Right · Keys R')).toBeInTheDocument();
  expect(screen.getByText('Linked stereo pair')).toBeInTheDocument();

  // Exactly ONE shared balance, named for the pair, and no per-member balance/pan.
  expect(screen.queryByRole('slider', { name: /Keys L pan|Keys R pan/i })).not.toBeInTheDocument();
  const balance = screen.getByRole('slider', { name: 'Keys L / Keys R balance' });
  expect(balance).toHaveValue('-40');
  fireEvent.change(balance, { target: { value: '60' } });
  expect(onPairBalanceChange).toHaveBeenCalledExactlyOnceWith('L', 'R', 0.6);
  expect(onChannelPanChange).not.toHaveBeenCalled();

  expect(screen.getByTestId('pair-balance-helper')).toHaveTextContent(
    'Center keeps left and right unchanged. Moving toward one side turns down the other side.',
  );
});

test('invalid_or_incomplete_pair_disables_the_shared_balance_and_never_degrades_to_mono_pan', () => {
  const onPairBalanceChange = vi.fn();
  const onChannelPanChange = vi.fn();
  // L claims R, but R does not reciprocate (incomplete link) — not a valid pair.
  const catalog = [
    source({ sourceId: 'L', label: 'Keys L', stereoPair: 'R', stereoSide: 'left' }),
    source({ sourceId: 'R', label: 'Keys R', stereoPair: null, stereoSide: null }),
  ];
  renderView({
    catalog,
    channels: [channel('L', 'Keys L'), channel('R', 'Keys R')],
    onPairBalanceChange,
    onChannelPanChange,
  });

  const balance = screen.getByRole('slider', { name: 'Keys L / Keys R balance' });
  expect(balance).toBeDisabled();
  fireEvent.change(balance, { target: { value: '80' } });
  expect(onPairBalanceChange).not.toHaveBeenCalled();
  // The pair must never be silently split into independent mono pan controls.
  expect(screen.queryByRole('slider', { name: 'Keys L pan' })).not.toBeInTheDocument();
  expect(screen.queryByRole('slider', { name: 'Keys R pan' })).not.toBeInTheDocument();
});

test('unavailable_pair_member_disables_the_shared_balance', () => {
  const catalog = [
    source({ sourceId: 'L', label: 'Keys L', stereoPair: 'R', stereoSide: 'left' }),
    source({ sourceId: 'R', label: 'Keys R', stereoPair: 'L', stereoSide: 'right', available: false }),
  ];
  renderView({ catalog, channels: [channel('L', 'Keys L'), channel('R', 'Keys R')] });
  expect(screen.getByRole('slider', { name: 'Keys L / Keys R balance' })).toBeDisabled();
});

test('unpaired_and_legacy_masterlr_sources_render_no_pan_or_balance', () => {
  const catalog = [
    source({ sourceId: 'vocal', label: 'Lead vocal', role: 'masterLr', stereoSide: null }),
  ];
  renderView({ catalog, channels: [channel('vocal', 'Lead vocal')] });
  expect(screen.queryByTestId('pan-readout')).not.toBeInTheDocument();
  expect(screen.queryByRole('slider', { name: /balance|pan/i })).not.toBeInTheDocument();
});

test('editing_is_only_available_under_the_existing_armed_or_ready_muted_rule', () => {
  const catalog = [source({ sourceId: 'vocal', label: 'Lead vocal' })];
  renderView({ phase: 'ready-muted', catalog, channels: [channel('vocal', 'Lead vocal')] });
  expect(screen.getByRole('slider', { name: 'Lead vocal pan' })).toBeEnabled();
});

test('pair_group_preserves_independent_gain_edits_without_touching_balance', () => {
  const onChannelGainChange = vi.fn();
  const onPairBalanceChange = vi.fn();
  const catalog = [
    source({ sourceId: 'L', label: 'Keys L', stereoPair: 'R', stereoSide: 'left' }),
    source({ sourceId: 'R', label: 'Keys R', stereoPair: 'L', stereoSide: 'right' }),
  ];
  renderView({
    catalog,
    channels: [channel('L', 'Keys L'), channel('R', 'Keys R')],
    onChannelGainChange,
    onPairBalanceChange,
  });
  const leftGain = screen.getByRole('slider', { name: 'Keys L' });
  fireEvent.change(leftGain, { target: { value: '-24' } });
  expect(onChannelGainChange).toHaveBeenCalledWith('L', -24);
  expect(onPairBalanceChange).not.toHaveBeenCalled();
});

test('valid_reciprocal_both_masterlr_pair_renders_the_group_and_one_balance_wired_to_pair_callback', () => {
  const onPairBalanceChange = vi.fn();
  const onChannelPanChange = vi.fn();
  const catalog = [
    source({ sourceId: 'mL', label: 'Master L', role: 'masterLr', stereoPair: 'mR', stereoSide: 'left' }),
    source({ sourceId: 'mR', label: 'Master R', role: 'masterLr', stereoPair: 'mL', stereoSide: 'right' }),
  ];
  renderView({
    catalog,
    channels: [channel('mL', 'Master L', { pan: 0.3 }), channel('mR', 'Master R', { pan: 0.3 })],
    onPairBalanceChange,
    onChannelPanChange,
  });

  // A paired MasterLr uses the balance law: a grouped pair with one shared balance, never mono pan.
  expect(screen.getByText('Linked stereo pair')).toBeInTheDocument();
  expect(screen.getByRole('slider', { name: 'Master L / Master R balance' })).toHaveValue('30');
  expect(screen.queryByRole('slider', { name: 'Master L pan' })).not.toBeInTheDocument();
  expect(screen.queryByRole('slider', { name: 'Master R pan' })).not.toBeInTheDocument();

  fireEvent.change(screen.getByRole('slider', { name: 'Master L / Master R balance' }), { target: { value: '50' } });
  expect(onPairBalanceChange).toHaveBeenCalledWith('mL', 'mR', 0.5);
  expect(onChannelPanChange).not.toHaveBeenCalled();
});

test('reciprocal_pair_with_mismatched_roles_is_a_disabled_invalid_pair', () => {
  const onPairBalanceChange = vi.fn();
  const catalog = [
    source({ sourceId: 'L', label: 'Keys L', role: 'inputChannel', stereoPair: 'R', stereoSide: 'left' }),
    source({ sourceId: 'R', label: 'Keys R', role: 'masterLr', stereoPair: 'L', stereoSide: 'right' }),
  ];
  renderView({
    catalog,
    channels: [channel('L', 'Keys L'), channel('R', 'Keys R')],
    onPairBalanceChange,
  });
  const balance = screen.getByRole('slider', { name: 'Keys L / Keys R balance' });
  expect(balance).toBeDisabled();
  fireEvent.change(balance, { target: { value: '70' } });
  expect(onPairBalanceChange).not.toHaveBeenCalled();
  expect(screen.getByText('Stereo balance unavailable.')).toBeInTheDocument();
});

test('a_present_pair_member_is_never_rendered_twice', () => {
  const catalog = [
    source({ sourceId: 'L', label: 'Keys L', stereoPair: 'R', stereoSide: 'left' }),
    source({ sourceId: 'R', label: 'Keys R', stereoPair: 'L', stereoSide: 'right' }),
  ];
  renderView({ catalog, channels: [channel('L', 'Keys L'), channel('R', 'Keys R')] });
  // One group with exactly one label heading per member, not a duplicated mono strip as well.
  expect(screen.getAllByText('Linked stereo pair')).toHaveLength(1);
  expect(screen.getAllByRole('slider', { name: 'Keys L' })).toHaveLength(1);
  expect(screen.getAllByRole('slider', { name: 'Keys R' })).toHaveLength(1);
});

test('legacy_masterlr_with_a_dangling_pair_renders_the_disabled_invalid_pair_group', () => {
  const onPairBalanceChange = vi.fn();
  // MasterLr points at a peer that does not reciprocate — incomplete, so it must fail closed and
  // show the disabled group (accepted behavior change), never a mono pan.
  const catalog = [
    source({ sourceId: 'mL', label: 'Master L', role: 'masterLr', stereoPair: 'mR', stereoSide: 'left' }),
    source({ sourceId: 'mR', label: 'Master R', role: 'masterLr', stereoPair: null, stereoSide: null }),
  ];
  renderView({
    catalog,
    channels: [channel('mL', 'Master L'), channel('mR', 'Master R')],
    onPairBalanceChange,
  });
  expect(screen.getByText('Stereo balance unavailable.')).toBeInTheDocument();
  const balance = screen.getByRole('slider', { name: 'Master L / Master R balance' });
  expect(balance).toBeDisabled();
  expect(screen.queryByRole('slider', { name: 'Master L pan' })).not.toBeInTheDocument();
  expect(screen.queryByRole('slider', { name: 'Master R pan' })).not.toBeInTheDocument();
});

