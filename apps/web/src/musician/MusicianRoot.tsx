import { useEffect, useRef, useState } from 'react';
import type { MixPatch, MixSnapshot, CounterString } from '../protocol';
import type { ReceiverController, ReceiverSnapshot } from '../receiver-ports';
import { Button } from '../ui/Button';
import { SimulatedPreview } from '../preview/SimulatedPreview';
import { JoinView } from './JoinView';
import { ReceiverView } from './ReceiverView';

export interface MusicianRootProps {
  controller?: ReceiverController;
}

export function MusicianRoot({ controller }: MusicianRootProps) {
  const [preview, setPreview] = useState(false);
  if (preview) return <SimulatedPreview onExit={() => setPreview(false)} />;
  if (!controller) {
    return (
      <main className="iem-unavailable iem-panel iem-stack">
        <p className="text-caption text-text-secondary">IEM Cast · Phone receiver</p>
        <h1 className="text-title">Receiver unavailable</h1>
        <span className="iem-environment">Browser receiver</span>
        <p>A live receiver connection is not configured in this build. No audio is playing.</p>
        <p className="text-text-secondary">Use a secure pairing link from a working host when available. You can explore fictional controls without connecting to hardware.</p>
        <Button variant="primary" onClick={() => setPreview(true)}>Open simulated preview</Button>
      </main>
    );
  }
  return <ConnectedMusician controller={controller} />;
}

interface Intent {
  patch: MixPatch;
  acceptedRevision?: CounterString;
  serial: number;
}

function ConnectedMusician({ controller }: { controller: ReceiverController }) {
  const [snapshot, setSnapshot] = useState<ReceiverSnapshot>(() => controller.getSnapshot());
  const [intent, setIntent] = useState<Intent | null>(null);
  const [rollback, setRollback] = useState<MixSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const serial = useRef(0);
  const latest = useRef(snapshot);
  latest.current = snapshot;

  useEffect(() => {
    setSnapshot(controller.getSnapshot());
    return controller.subscribe(next => {
      const prior = latest.current;
      latest.current = next;
      setSnapshot(next);
      const unavailable =
        next.phase === 'interrupted' ||
        next.phase === 'revoked' ||
        next.phase === 'unpaired';
      const sourceLost = prior.catalog.sources.some(s =>
        s.available &&
        !next.catalog.sources.some(n =>
          n.sourceId === s.sourceId && n.available && n.authorized,
        ),
      );
      if ((unavailable && prior.phase !== next.phase) || sourceLost) {
        serial.current++;
        setIntent(null);
        setRollback(null);
        setBusy(false);
        controller.personalMasterMute(true);
      }
    });
  }, [controller]);

  const base = snapshot.requested_mix ?? snapshot.accepted_mix ?? snapshot.applied_mix;
  const applied = snapshot.applied_mix;
  const match =
    !!intent?.acceptedRevision &&
    applied?.mixRevision === intent.acceptedRevision &&
    applied.catalogRevision === intent.patch.catalogRevision &&
    applied.masterDb === intent.patch.masterDb &&
    applied.masterMuted === intent.patch.masterMuted &&
    intent.patch.sources.every(s =>
      applied.sources.some(a =>
        a.sourceId === s.sourceId && a.gainDb === s.gainDb && a.muted === s.muted,
      ),
    );
  const pending = !!intent && !match;
  const display = pending ? intent.patch : rollback ?? base;
  const editable = snapshot.phase === 'armed' || snapshot.phase === 'ready-muted';
  const errorMessage = (e: unknown) =>
    e instanceof Error ? e.message : 'The host could not complete this action.';

  async function request(patch: MixPatch) {
    if (!editable) return;
    const id = ++serial.current;
    const previous = latest.current.applied_mix ?? latest.current.accepted_mix;
    setIntent({ patch, serial: id });
    setRollback(null);
    setError(null);
    try {
      const ack = await controller.requestMix(patch);
      if (id !== serial.current) return;
      setIntent({
        patch: { ...patch, ...ack.canonicalSettings, baseRevision: ack.acceptedRevision },
        serial: id,
        acceptedRevision: ack.acceptedRevision,
      });
    } catch (e) {
      if (id === serial.current) {
        setError(errorMessage(e));
        setIntent(null);
        setRollback(previous);
      }
    }
  }
  function edit(change: Partial<Pick<MixSnapshot, 'sources' | 'masterDb' | 'masterMuted'>>) {
    if (!display || !base) return;
    void request({
      baseRevision: snapshot.accepted_mix?.mixRevision ?? base.mixRevision,
      catalogRevision: snapshot.catalog.catalogRevision,
      sources: display.sources,
      masterDb: display.masterDb,
      masterMuted: display.masterMuted,
      ...change,
    });
  }
  function start() {
    const id = ++serial.current;
    setError(null);
    setBusy(true);
    // Invoke arm inside this click, BEFORE any await. Never chain connect().then(arm()).
    try {
      void controller
        .arm()
        .catch(e => {
          if (id === serial.current) setError(errorMessage(e));
        })
        .finally(() => {
          if (id === serial.current) setBusy(false);
        });
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }
  function prepare() {
    const id = ++serial.current;
    setError(null);
    setBusy(true);
    void controller
      .connect()
      .catch(e => {
        if (id === serial.current) setError(errorMessage(e));
      })
      .finally(() => {
        if (id === serial.current) setBusy(false);
      });
  }
  function stop() {
    serial.current++;
    setIntent(null);
    setRollback(null);
    setBusy(false);
    controller.stop();
    setSnapshot(controller.getSnapshot());
  }

  const hasSources = snapshot.catalog.sources.some(s => s.available && s.authorized);
  if (
    snapshot.phase === 'unpaired' ||
    snapshot.phase === 'paired' ||
    snapshot.phase === 'negotiating' ||
    snapshot.phase === 'revoked'
  ) {
    return (
      <JoinView
        phase={snapshot.phase}
        hasSources={hasSources}
        connecting={busy}
        error={error ?? snapshot.error}
        onStartListening={prepare}
        onCancel={() => {
          serial.current++;
          controller.disconnect();
        }}
      />
    );
  }
  const channels = (display?.sources ?? []).map(s => {
    const a = applied?.sources.find(n => n.sourceId === s.sourceId);
    return {
      sourceId: s.sourceId,
      label: snapshot.catalog.sources.find(n => n.sourceId === s.sourceId)?.label ?? s.sourceId,
      requestedDb: s.gainDb,
      appliedDb: a?.gainDb,
      appliedMuted: a?.muted,
      muted: s.muted,
      pending,
      accepted: pending && !!intent?.acceptedRevision,
    };
  });

  return (
    <ReceiverView
      phase={snapshot.phase}
      catalog={snapshot.catalog.sources}
      channels={channels}
      masterRequestedDb={display?.masterDb ?? -60}
      masterAppliedDb={applied?.masterDb}
      masterMuted={snapshot.master_local_muted}
      masterPending={pending}
      onMasterMuteChange={m => {
        controller.personalMasterMute(m);
        setSnapshot(controller.getSnapshot());
      }}
      onMasterGainChange={db => edit({ masterDb: db })}
      onChannelGainChange={(id, db) =>
        edit({
          sources: display?.sources.map(s =>
            s.sourceId === id ? { ...s, gainDb: db } : s,
          ) ?? [],
        })
      }
      onChannelMuteChange={(id, muted) => {
        if (!muted && snapshot.phase !== 'armed') return;
        edit({
          sources: display?.sources.map(s =>
            s.sourceId === id ? { ...s, muted } : s,
          ) ?? [],
        });
      }}
      onStop={stop}
      onStart={start}
      onPrepare={prepare}
      starting={busy}
      hasSources={hasSources && !!base}
      diagnostics={snapshot.diagnostics}
      meters={{}}
      error={error ?? snapshot.error}
      wakeLockWarning={null}
    />
  );
}
