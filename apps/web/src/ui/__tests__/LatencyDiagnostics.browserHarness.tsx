// Browser-only fixture. Never imported by a product entrypoint; no signaling or media ports.
import { createRoot } from 'react-dom/client';
import type { DiagnosticsSnapshot } from '../../receiver-ports';
import { ReceiverView } from '../../musician/ReceiverView';

export function mountDiagnostics() {
  const container = document.getElementById('root');
  if (!container) throw new Error('Missing browser fixture root');
  // Do not reuse the product entrypoint's React root.
  const fixture = document.createElement('div');
  container.replaceWith(fixture);
  const root = createRoot(fixture);
  const label = 'A deliberately long browser-test vocal source name';
  const snapshot: DiagnosticsSnapshot = {
    network_rtt_ms: 42,
    buffer_delay_ms: 152,
    buffer_target_delay_ms: 160,
    buffer_minimum_delay_ms: 38,
    packets_lost_delta: 0,
    concealment_percent: 1.25,
    jitter_buffer_preference: { state: 'Accepted', requested_ms: 0 },
    end_to_end: 'Not measured',
    last_updated: 1_700_000_000_000,
  };
  function update(overrides: Partial<DiagnosticsSnapshot> = {}) {
    root.render(
      <ReceiverView
        phase="ready-muted"
        catalog={[{ sourceId: 'fixture-source', physicalIndex: 0, label, role: 'inputChannel', available: true, authorized: true, stereoPair: null, stereoSide: null, defaultPan: 0 }]}
        channels={[{ sourceId: 'fixture-source', label, requestedDb: -12, muted: true }]}
        masterRequestedDb={-18}
        masterMuted
        onMasterMuteChange={() => {}}
        onMasterGainChange={() => {}}
        onChannelGainChange={() => {}}
        onChannelMuteChange={() => {}}
        onChannelPanChange={() => {}}
        onPairBalanceChange={() => {}}
        onStop={() => {}}
        diagnostics={{ ...snapshot, ...overrides }}
        meters={{}}
        error={null}
        wakeLockWarning={null}
      />,
    );
  }
  update();
  return { update, dispose: () => root.unmount() };
}
