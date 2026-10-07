import { useCallback, useEffect, useMemo, useState } from 'react';
import type { CounterString, MixPatch } from '../protocol';
import type { ReceiverController, ReceiverSnapshot } from '../receiver-ports';
import type { ChannelStripState } from '../ui/ChannelStrip';
import { JoinView } from './JoinView';
import { ReceiverView } from './ReceiverView';

export interface MusicianRootProps {
  /**
   * Frozen `ReceiverController` (Task 1 interface; Task 6 implementation).
   * Optional only so the INT-owned `main.tsx` entrypoint compiles before it is
   * wired; a real controller is required at runtime.
   */
  controller?: ReceiverController;
}

const EMPTY_SNAPSHOT: ReceiverSnapshot = {
  phase: 'unpaired',
  catalog: { catalogRevision: '0' as CounterString, sources: [] },
  requested_mix: null,
  accepted_mix: null,
  applied_mix: null,
  master_local_muted: true,
  diagnostics: {
    network_rtt_ms: 'Unavailable',
    buffer_delay_ms: 'Unavailable',
    end_to_end: 'Not measured',
    last_updated: null,
  },
  error: null,
};

/**
 * Builds a full MixPatch from the current requested mix, changing one source entry.
 * Every other source and the master values are preserved.
 */
function patchWithSource(
  current: ReceiverSnapshot['requested_mix'],
  sourceId: string,
  change: { gainDb?: number; muted?: boolean },
): MixPatch | null {
  if (!current) return null;
  return {
    baseRevision: current.mixRevision,
    catalogRevision: current.catalogRevision,
    sources: current.sources.map((entry) => ({
      sourceId: entry.sourceId,
      gainDb: entry.sourceId === sourceId && change.gainDb !== undefined ? change.gainDb : entry.gainDb,
      muted: entry.sourceId === sourceId && change.muted !== undefined ? change.muted : entry.muted,
    })),
    masterDb: current.masterDb,
    masterMuted: current.masterMuted,
  };
}

/**
 * Inert controller used only when no controller is injected (e.g. the INT-owned
 * entrypoint renders `<MusicianRoot />` before it is wired). It never arms,
 * never plays audio, and reports the empty disconnected snapshot.
 */
const INERT_CONTROLLER: ReceiverController = {
  subscribe: () => () => {},
  getSnapshot: () => EMPTY_SNAPSHOT,
  connect: async () => {},
  requestMix: async () => {
    throw new Error('Receiver controller is not connected');
  },
  arm: async () => {},
  personalMasterMute: () => {},
  stop: () => {},
  disconnect: () => {},
};

export function MusicianRoot({ controller }: MusicianRootProps) {
  const active = controller ?? INERT_CONTROLLER;
  const [snapshot, setSnapshot] = useState<ReceiverSnapshot>(() => active.getSnapshot());
  const [masterRequestedDb, setMasterRequestedDb] = useState(-12);

  useEffect(() => active.subscribe(setSnapshot), [active]);

  const channels: ChannelStripState[] = useMemo(() => {
    const requested = snapshot.requested_mix;
    if (!requested) return [];

    const labelById = new Map(snapshot.catalog.sources.map((source) => [source.sourceId, source.label]));
    const appliedById = new Map((snapshot.applied_mix?.sources ?? []).map((entry) => [entry.sourceId, entry]));
    const acceptedById = new Map(
      (snapshot.accepted_mix?.sources ?? []).map((entry) => [entry.sourceId, entry]),
    );

    return requested.sources.map((entry) => ({
      sourceId: entry.sourceId,
      label: labelById.get(entry.sourceId) ?? entry.sourceId,
      requestedDb: entry.gainDb,
      appliedDb:
        appliedById.get(entry.sourceId)?.gainDb ??
        acceptedById.get(entry.sourceId)?.gainDb ??
        entry.gainDb,
      muted: entry.muted,
    }));
  }, [snapshot]);

  const handleChannelGain = useCallback(
    (sourceId: string, db: number) => {
      const patch = patchWithSource(snapshot.requested_mix, sourceId, { gainDb: db });
      if (patch) void active.requestMix(patch);
    },
    [active, snapshot.requested_mix],
  );

  const handleChannelMute = useCallback(
    (sourceId: string, muted: boolean) => {
      const patch = patchWithSource(snapshot.requested_mix, sourceId, { muted });
      if (patch) void active.requestMix(patch);
    },
    [active, snapshot.requested_mix],
  );

  const handleMasterGain = useCallback(
    (db: number) => {
      setMasterRequestedDb(db);
      const current = snapshot.requested_mix;
      if (!current) return;
      const patch: MixPatch = {
        baseRevision: current.mixRevision,
        catalogRevision: current.catalogRevision,
        sources: current.sources.map((entry) => ({
          sourceId: entry.sourceId,
          gainDb: entry.gainDb,
          muted: entry.muted,
        })),
        masterDb: db,
        masterMuted: current.masterMuted,
      };
      void active.requestMix(patch);
    },
    [active, snapshot.requested_mix],
  );

  const handleStartListening = useCallback(() => {
    if (snapshot.phase === 'unpaired' || snapshot.phase === 'paired') {
      void active.connect().then(() => active.arm());
      return;
    }
    void active.arm();
  }, [active, snapshot.phase]);

  const handleStop = useCallback(() => active.stop(), [active]);

  const showReceiver = snapshot.phase !== 'unpaired' && snapshot.phase !== 'revoked';
  const hasSources = snapshot.catalog.sources.some((source) => source.available);

  if (!showReceiver) {
    return (
      <JoinView
        phase={snapshot.phase}
        hasSources={hasSources}
        connecting={snapshot.phase === 'negotiating'}
        error={snapshot.error}
        onStartListening={handleStartListening}
        onCancel={handleStop}
      />
    );
  }

  return (
    <ReceiverView
      phase={snapshot.phase}
      catalog={snapshot.catalog.sources}
      channels={channels}
      masterRequestedDb={masterRequestedDb}
      masterAppliedDb={snapshot.applied_mix?.masterDb ?? masterRequestedDb}
      masterMuted={snapshot.master_local_muted}
      onMasterMuteChange={(muted) => active.personalMasterMute(muted)}
      onMasterGainChange={handleMasterGain}
      onChannelGainChange={handleChannelGain}
      onChannelMuteChange={handleChannelMute}
      onStop={handleStop}
      diagnostics={snapshot.diagnostics}
      meters={{}}
      error={snapshot.error}
      wakeLockWarning={null}
    />
  );
}
