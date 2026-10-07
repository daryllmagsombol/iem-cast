import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { ReceiverView } from '../ReceiverView';
import type { ChannelStripState } from '../../ui/ChannelStrip';
import type { SourceInfo } from '../../protocol';
import type { DiagnosticsSnapshot } from '../../receiver-ports';

const catalog: SourceInfo[] = [
  {
    sourceId: '33333333-3333-4333-8333-333333333333',
    physicalIndex: 0,
    label: 'Lead vocal',
    role: 'inputChannel',
    authorized: true,
    available: true,
    stereoPair: null,
  },
];

const channel: ChannelStripState = {
  sourceId: '33333333-3333-4333-8333-333333333333',
  label: 'Lead vocal',
  requestedDb: -12,
  appliedDb: -12,
  muted: false,
};

const diagnostics: DiagnosticsSnapshot = {
  network_rtt_ms: 'Unavailable',
  buffer_delay_ms: 'Waiting for sample',
  end_to_end: 'Not measured',
  last_updated: null,
};

test('receiver_keeps_master_stop_available_while_listening', () => {
  render(
    <ReceiverView
      phase="armed"
      catalog={catalog}
      channels={[channel]}
      masterRequestedDb={-12}
      masterAppliedDb={-12}
      masterMuted={false}
      onMasterMuteChange={() => {}}
      onMasterGainChange={() => {}}
      onChannelGainChange={() => {}}
      onChannelMuteChange={() => {}}
      onStop={() => {}}
      diagnostics={diagnostics}
      meters={{}}
      error={null}
      wakeLockWarning={null}
    />,
  );
  expect(screen.getByRole('button', { name: /stop listening/i })).toBeEnabled();
});

test('disconnected_state_is_announced_as_output_silenced_alert', () => {
  render(
    <ReceiverView
      phase="interrupted"
      catalog={catalog}
      channels={[channel]}
      masterRequestedDb={-12}
      masterAppliedDb={-12}
      masterMuted
      onMasterMuteChange={() => {}}
      onMasterGainChange={() => {}}
      onChannelGainChange={() => {}}
      onChannelMuteChange={() => {}}
      onStop={() => {}}
      diagnostics={diagnostics}
      meters={{}}
      error="peer disconnected"
      wakeLockWarning={null}
    />,
  );
  expect(screen.getByRole('alert')).toHaveTextContent(/output silenced/i);
});

test('missing_telemetry_meters_render_unavailable', () => {
  render(
    <ReceiverView
      phase="armed"
      catalog={catalog}
      channels={[channel]}
      masterRequestedDb={-12}
      masterAppliedDb={-12}
      masterMuted={false}
      onMasterMuteChange={() => {}}
      onMasterGainChange={() => {}}
      onChannelGainChange={() => {}}
      onChannelMuteChange={() => {}}
      onStop={() => {}}
      diagnostics={diagnostics}
      meters={{}}
      error={null}
      wakeLockWarning={null}
    />,
  );
  expect(screen.getAllByText(/unavailable/i).length).toBeGreaterThan(0);
});
