import { createRoot } from 'react-dom/client';
import { ReceiverView } from '../../musician/ReceiverView';
import type { ChannelStripState } from '../../ui/ChannelStrip';
import type { SourceInfo } from '../../protocol';
import type { DiagnosticsSnapshot } from '../../receiver-ports';

/** Realistic long labels exercise wrapping/overflow without any host or audio wiring. */
const LONG = 'A deliberately long browser-test vocal source name';

const diagnostics: DiagnosticsSnapshot = {
  network_rtt_ms: 'Unavailable',
  buffer_delay_ms: 'Unavailable',
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

function channel(sourceId: string, label: string, pan = 0): ChannelStripState {
  return { sourceId, label, requestedDb: -12, appliedDb: -12, muted: false, pan };
}

/** Standalone fixture: no controller, media, signaling, or persistence. */
export function mountPanReceiver() {
  const existing = document.getElementById('root');
  if (!existing) throw new Error('Missing fixture root');
  const node = document.createElement('div');
  existing.replaceWith(node);
  const root = createRoot(node);
  const events: Array<{ kind: string; args: unknown[] }> = [];
  // Pair present but NOT reciprocal, so it must render as a disabled group and never as mono pan.
  const invalidCatalog = [
    source({ sourceId: 'orphanL', label: `${LONG} L`, stereoPair: 'orphanR', stereoSide: 'left' }),
    source({ sourceId: 'orphanR', label: `${LONG} R`, stereoPair: null, stereoSide: null }),
  ];
  const catalog: SourceInfo[] = [
    source({ sourceId: 'vocal', label: LONG, role: 'masterLr', defaultPan: 0 }),
    source({ sourceId: 'keysL', label: 'Keys L', stereoPair: 'keysR', stereoSide: 'left', defaultPan: 0.4 }),
    source({ sourceId: 'keysR', label: 'Keys R', stereoPair: 'keysL', stereoSide: 'right', defaultPan: 0.4 }),
    // VALID reciprocal both-masterLr pair: must render as a grouped pair with one balance.
    source({ sourceId: 'masterL', label: 'Master L', role: 'masterLr', stereoPair: 'masterR', stereoSide: 'left', defaultPan: 0.1 }),
    source({ sourceId: 'masterR', label: 'Master R', role: 'masterLr', stereoPair: 'masterL', stereoSide: 'right', defaultPan: 0.1 }),
    ...invalidCatalog,
  ];
  const channels = [channel('vocal', LONG), channel('keysL', 'Keys L', 0.4), channel('keysR', 'Keys R', 0.4), channel('masterL', 'Master L', 0.1), channel('masterR', 'Master R', 0.1), channel('orphanL', `${LONG} L`), channel('orphanR', `${LONG} R`)];

  function render(phase: 'armed' | 'ready-muted') {
    root.render(
      <ReceiverView
        phase={phase}
        catalog={catalog}
        channels={channels}
        masterRequestedDb={-12}
        masterMuted
        onMasterMuteChange={() => {}}
        onMasterGainChange={() => {}}
        onChannelGainChange={(id, db) => events.push({ kind: 'gain', args: [id, db] })}
        onChannelMuteChange={(id, m) => events.push({ kind: 'mute', args: [id, m] })}
        onChannelPanChange={(id, pan) => events.push({ kind: 'pan', args: [id, pan] })}
        onPairBalanceChange={(l, r, pan) => events.push({ kind: 'balance', args: [l, r, pan] })}
        onStop={() => events.push({ kind: 'stop', args: [] })}
        diagnostics={diagnostics}
        meters={{}}
        error={null}
        wakeLockWarning={null}
      />,
    );
  }
  render('armed');
  return { events, setPhase: render, dispose: () => root.unmount() };
}
