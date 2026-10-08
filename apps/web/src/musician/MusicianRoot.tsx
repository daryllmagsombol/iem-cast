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
  const actionSerial = useRef(0);
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
        actionSerial.current++;
        setIntent(null);
        setRollback(null);
        setBusy(false);
        controller.personalMasterMute(true);
      }
    });
  }, [controller]);

  const base = snapshot.requested_mix ?? snapshot.accepted_mix ?? snapshot.applied_mix;
  const applied = snapshot.applied_mix;
  // A listener starts with no host mix. Rather than block every edit, synthesize a neutral local
  // mix from the catalog (all sources muted at a safe attenuation, master attenuated); the first
  // patch then carries the listener's real intent. Muted-by-default keeps nothing unexpectedly
  // audible before an explicit unmute.
  const neutralMix: MixSnapshot = base ?? {
    catalogRevision: snapshot.catalog.catalogRevision,
    mixRevision: '0' as CounterString,
    sources: snapshot.catalog.sources
      .filter(source => source.available && source.authorized)
      .map(source => ({ sourceId: source.sourceId, gainDb: -60, muted: true })),
    masterDb: -60,
    masterMuted: true,
  };
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
  const display = pending ? intent.patch : rollback ?? neutralMix;
  const editable = snapshot.phase === 'armed' || snapshot.phase === 'ready-muted';
  const errorMessage = (e: unknown) =>
    e instanceof Error ? e.message : 'The host could not complete this action.';

  async function request(patch: MixPatch) {
    if (!editable) return;
    const id = ++serial.current;
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
        setRollback(latest.current.accepted_mix ?? latest.current.applied_mix);
      }
    }
  }
  function edit(change: Partial<Pick<MixSnapshot, 'sources' | 'masterDb' | 'masterMuted'>>) {
    if (!display) return;
    void request({
      // Revisions are decimal strings; the first patch of a session uses "0" (the host's initial
      // revision) when no mix has been accepted yet.
      baseRevision: (snapshot.accepted_mix?.mixRevision ?? base?.mixRevision ?? '0') as CounterString,
      catalogRevision: snapshot.catalog.catalogRevision,
      sources: display.sources,
      masterDb: display.masterDb,
      masterMuted: display.masterMuted,
      ...change,
    });
  }
  function start() {
    const id = ++actionSerial.current;
    setError(null);
    setBusy(true);
    // Invoke arm inside this click, BEFORE any await. Never chain connect().then(arm()).
    try {
      void controller
        .arm()
        .catch(e => {
          if (id === actionSerial.current) setError(errorMessage(e));
        })
        .finally(() => {
          if (id === actionSerial.current) setBusy(false);
        });
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }
  function prepare() {
    const id = ++actionSerial.current;
    setError(null);
    setBusy(true);
    void controller
      .connect()
      .catch(e => {
        if (id === actionSerial.current) setError(errorMessage(e));
      })
      .finally(() => {
        if (id === actionSerial.current) setBusy(false);
      });
  }
  function stop() {
    serial.current++;
    actionSerial.current++;
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
  // Channels come from the CATALOG, not from the mix: a freshly paired listener has no mix yet,
  // and deriving faders from the mix left zero controls, so the listener could never create a
  // first patch. Gain/mute defaults are neutral until the host sends real values.
  const mixBySource = new Map((display?.sources ?? []).map(s => [s.sourceId, s]));
  const channels = snapshot.catalog.sources
    .filter(source => source.available && source.authorized)
    .map(source => {
      const mixed = mixBySource.get(source.sourceId);
      const a = applied?.sources.find(n => n.sourceId === source.sourceId);
      return {
        sourceId: source.sourceId,
        label: source.label,
        // Default to muted with a safe attenuation so nothing is unexpectedly audible before the
        // listener sets a level and unmutes explicitly.
        requestedDb: mixed?.gainDb ?? -60,
        appliedDb: a?.gainDb,
        appliedMuted: a?.muted,
        muted: mixed?.muted ?? true,
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
      masterUnmutePending={snapshot.master_unmute_pending}
      masterPending={pending}
      onMasterMuteChange={m => {
        serial.current++;
        setIntent(null);
        setRollback(null);
        setError(null);
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
      // A listener may arm before touching any fader: no mix exists yet, but the host accepts the
      // arm and audio can flow. Requiring an existing mix here disabled Start listening forever
      // for a fresh listener, which also left the personal-output unmute permanently disabled.
      hasSources={hasSources}
      diagnostics={snapshot.diagnostics}
      meters={{}}
      error={error ?? snapshot.error}
      wakeLockWarning={null}
    />
  );
}
