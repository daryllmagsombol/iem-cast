import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ListenerPhase, SourceInfo } from '../protocol';
import type { DiagnosticsSnapshot } from '../receiver-ports';
import { Button } from '../ui/Button';
import { ChannelStrip, type ChannelStripState } from '../ui/ChannelStrip';
import { LatencyDiagnostics } from '../ui/LatencyDiagnostics';
import { GainControl } from '../ui/GainControl';
import { PanControl } from '../ui/PanControl';
import { ThemeChoice } from '../ui/ThemeChoice';

export interface ReceiverViewProps {
  phase: ListenerPhase;
  catalog: SourceInfo[];
  channels: ChannelStripState[];
  masterRequestedDb: number;
  masterAppliedDb?: number;
  masterMuted: boolean;
  masterUnmutePending?: boolean;
  masterPending?: boolean;
  onMasterMuteChange(muted: boolean): void;
  onMasterGainChange(db: number): void;
  onChannelGainChange(sourceId: string, db: number): void;
  onChannelMuteChange(sourceId: string, muted: boolean): void;
  /**
   * Mono pan: change ONLY this source's pan (normalized `[-1, 1]`). Gains, mutes, master, and arm
   * are not touched. Consumed by the grouped-strip UI later (designer Task 6).
   */
  onChannelPanChange(sourceId: string, pan: number): void;
  /**
   * Stereo balance: change BOTH pair members' pan atomically in one edit (normalized `[-1, 1]`),
   * preserving each member's independent gain and mute. Consumed by the designer Task 6.
   */
  onPairBalanceChange(leftSourceId: string, rightSourceId: string, pan: number): void;
  onStop(): void;
  onStart?(): void;
  onPrepare?(): void;
  starting?: boolean;
  hasSources?: boolean;
  diagnostics: DiagnosticsSnapshot;
  meters: Record<string, number | undefined>;
  error: string | null;
  wakeLockWarning: string | null;
}

export function ReceiverView(props: ReceiverViewProps) {
  const {
    phase,
    catalog,
    channels,
    masterRequestedDb,
    masterAppliedDb,
    masterMuted,
    onMasterMuteChange,
    onMasterGainChange,
    onStop,
    diagnostics,
    error,
  } = props;
  const armed = phase === 'armed';
  const editable = armed || phase === 'ready-muted';
  const dock = useRef<HTMLDivElement>(null);
  const [clearance, setClearance] = useState(160);
  useLayoutEffect(() => {
    if (!dock.current || typeof ResizeObserver === 'undefined') return;
    const measure = () => {
      const height = dock.current?.getBoundingClientRect().height ?? 160;
      setClearance(height);
      document.documentElement.style.setProperty('--iem-dock-clearance', `${height}px`);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(dock.current);
    return () => {
      observer.disconnect();
      document.documentElement.style.removeProperty('--iem-dock-clearance');
    };
  }, []);
  const sourceById = useMemo(
    () => new Map(catalog.map(source => [source.sourceId, source])),
    [catalog],
  );
  // Present channels as ordered units: each valid reciprocal pair becomes ONE group (two member
  // strips plus a single shared balance); everything else stays a standalone strip. Adjacency is
  // never used to infer a pair — only authoritative reciprocal `stereoPair`/`stereoSide` metadata.
  const groups = useMemo(() => {
    const usable = (source: SourceInfo | undefined) =>
      !!source && source.available && source.authorized;
    const seen = new Set<string>();
    return channels.map(channel => {
      if (seen.has(channel.sourceId)) return null;
      seen.add(channel.sourceId);
      const source = sourceById.get(channel.sourceId);
      const peerId = source?.stereoPair ?? null;
      const peer = peerId ? sourceById.get(peerId) : undefined;
      const peerChannel = peerId ? channels.find(c => c.sourceId === peerId) : undefined;
      const reciprocal = !!peerId && peer?.stereoPair === channel.sourceId;
      const validPair =
        reciprocal &&
        !!source &&
        !!peer &&
        !!peerChannel &&
        source.stereoSide !== null &&
        peer.stereoSide !== null &&
        source.stereoSide !== peer.stereoSide;
      // `stereoPair` is set but the pair is not complete/valid: keep it a disabled group so it can
      // never silently degrade into two independent mono pan controls.
      if (peerId && source && source.role !== 'masterLr') {
        if (!validPair) {
          if (peerChannel) seen.add(peerChannel.sourceId);
          return { kind: 'invalid-pair' as const, source, peer: peer ?? null, channel, peerChannel: peerChannel ?? null };
        }
        seen.add(peer!.sourceId);
        const left = source.stereoSide === 'left' ? source : peer!;
        const right = source.stereoSide === 'left' ? peer! : source;
        const leftChannel = left.sourceId === channel.sourceId ? channel : peerChannel!;
        const rightChannel = right.sourceId === channel.sourceId ? channel : peerChannel!;
        const pairUsable = usable(left) && usable(right);
        return {
          kind: 'pair' as const,
          left,
          right,
          leftChannel,
          rightChannel,
          disabled: !editable || !pairUsable,
        };
      }
      return { kind: 'mono' as const, source, channel };
    }).filter((group): group is NonNullable<typeof group> => group !== null);
  }, [channels, sourceById, editable]);

  return (
    <div
      className="iem-receiver"
      style={{ paddingBottom: `calc(${clearance}px + var(--iem-space-5))` }}
      onFocusCapture={event => {
        const target = event.target;
        if (!(target instanceof HTMLElement) || dock.current?.contains(target)) return;
        const details = dock.current?.querySelector('details');
        if (details?.open && window.innerHeight < 480) details.open = false;
        requestAnimationFrame(() => {
          const bottom = target.getBoundingClientRect().bottom;
          const top = dock.current?.getBoundingClientRect().top ?? window.innerHeight;
          if (bottom > top - 16) window.scrollBy(0, bottom - top + 16);
        });
      }}
    >
      <header className="iem-row">
        <div>
          <p className="text-caption text-text-secondary">IEM Cast · Personal mix</p>
          <h1 className="text-title">Personal monitor</h1>
          <span className="iem-environment">Browser receiver</span>
        </div>
        <ThemeChoice />
      </header>
      <p role="status" className="iem-hint">
        {armed
          ? (masterMuted ? 'Session armed · Personal output muted' : 'Listening')
          : phase === 'interrupted'
            ? 'Connection interrupted · Not listening'
            : 'Connected · Not listening'}
      </p>
      <p className="text-caption text-text-secondary">Keep this page visible and the screen on. Wired earphones only.</p>
      {error && (
        <p role="alert" className="iem-hint text-danger">
          {error}
        </p>
      )}
      {props.wakeLockWarning && (
        <p className="iem-hint text-warning">{props.wakeLockWarning}</p>
      )}
      {phase === 'interrupted' && (
        <Button
          onClick={props.onPrepare}
          pending={props.starting}
          disabled={!props.onPrepare}
        >
          Prepare connection
        </Button>
      )}
      {!armed && (
        <Button
          variant="primary"
          pending={props.starting}
          disabled={!props.hasSources || !props.onStart || phase !== 'ready-muted'}
          explanation={
            !props.hasSources
              ? 'An available source and synchronized mix are required.'
              : phase === 'interrupted'
                ? 'Prepare the connection again before listening.'
                : undefined
          }
          onClick={props.onStart}
        >
          Start listening
        </Button>
      )}
      <section className="iem-stack" aria-labelledby="channels-heading">
        <h2 id="channels-heading" className="text-section">Channels</h2>
        {!channels.length && <p className="iem-hint">No channels assigned</p>}
        {groups.map(group => {
          if (group.kind === 'pair') {
            const { left, right, leftChannel, rightChannel, disabled } = group;
            const position = leftChannel.pan ?? rightChannel.pan ?? 0;
            const unavailable = !left.available || !left.authorized || !right.available || !right.authorized;
            return (
              <section
                key={left.sourceId}
                className="iem-stack rounded-panel border border-boundary p-3"
                aria-label={`${left.label} and ${right.label} stereo pair`}
              >
                <h3 className="text-label text-text">Linked stereo pair</h3>
                <ChannelStrip
                  state={leftChannel}
                  fullLabel={left.label}
                  disabled={disabled}
                  canUnmute={armed}
                  linkLabel={`Left · ${left.label}`}
                  meterValueDbfs={props.meters[left.sourceId]}
                  onGainChange={db => props.onChannelGainChange(left.sourceId, db)}
                  onMuteChange={m => props.onChannelMuteChange(left.sourceId, m)}
                />
                <ChannelStrip
                  state={rightChannel}
                  fullLabel={right.label}
                  disabled={disabled}
                  canUnmute={armed}
                  linkLabel={`Right · ${right.label}`}
                  meterValueDbfs={props.meters[right.sourceId]}
                  onGainChange={db => props.onChannelGainChange(right.sourceId, db)}
                  onMuteChange={m => props.onChannelMuteChange(right.sourceId, m)}
                />
                <div className="flex flex-col gap-1">
                  <PanControl
                    label="Balance"
                    fullLabel={`${left.label} / ${right.label} balance`}
                    value={position}
                    disabled={disabled}
                    onChange={value => props.onPairBalanceChange(left.sourceId, right.sourceId, value)}
                    onReset={() => props.onPairBalanceChange(left.sourceId, right.sourceId, 0)}
                  />
                  <p className="text-caption text-text-secondary" data-testid="pair-balance-helper">
                    Center keeps left and right unchanged. Moving toward one side turns down the other side.
                  </p>
                  {unavailable && <p className="text-warning">Source unavailable — edits disabled.</p>}
                </div>
              </section>
            );
          }
          if (group.kind === 'invalid-pair') {
            const { source, peer, channel, peerChannel } = group;
            const disabled = true;
            return (
              <section
                key={source.sourceId}
                className="iem-stack rounded-panel border border-boundary p-3"
                aria-label={`${source.label} incomplete stereo pair`}
              >
                <h3 className="text-label text-text">Linked stereo pair</h3>
                <ChannelStrip
                  state={channel}
                  disabled={disabled}
                  canUnmute={armed}
                  linkLabel={source.stereoSide ? `${source.stereoSide === 'left' ? 'Left' : 'Right'} · ${source.label}` : source.label}
                  meterValueDbfs={props.meters[source.sourceId]}
                  onGainChange={db => props.onChannelGainChange(source.sourceId, db)}
                  onMuteChange={m => props.onChannelMuteChange(source.sourceId, m)}
                />
                {peer && peerChannel ? (
                  <ChannelStrip
                    state={peerChannel}
                    disabled={disabled}
                    canUnmute={armed}
                    linkLabel={peer.stereoSide ? `${peer.stereoSide === 'left' ? 'Left' : 'Right'} · ${peer.label}` : peer.label}
                    meterValueDbfs={props.meters[peer.sourceId]}
                    onGainChange={db => props.onChannelGainChange(peer.sourceId, db)}
                    onMuteChange={m => props.onChannelMuteChange(peer.sourceId, m)}
                  />
                ) : null}
                <div className="flex flex-col gap-1">
                  <PanControl
                    label="Balance"
                    fullLabel={`${source.label} / ${peer?.label ?? 'paired channel'} balance`}
                    value={channel.pan ?? peerChannel?.pan ?? 0}
                    disabled={disabled}
                    onChange={() => {}}
                    onReset={() => {}}
                  />
                  <p className="text-caption text-text-secondary">Stereo balance unavailable.</p>
                </div>
              </section>
            );
          }
          const { source, channel } = group;
          const unavailable = !source?.available || !source.authorized;
          const noPan = !source || source.role === 'masterLr';
          return (
            <div key={channel.sourceId}>
              <ChannelStrip
                state={channel}
                disabled={!editable || unavailable}
                canUnmute={armed}
                linkLabel={noPan ? undefined : 'Mono source'}
                meterValueDbfs={props.meters[channel.sourceId]}
                onGainChange={db => props.onChannelGainChange(channel.sourceId, db)}
                onMuteChange={m => props.onChannelMuteChange(channel.sourceId, m)}
                pan={
                  noPan
                    ? undefined
                    : {
                        label: 'Pan',
                        value: channel.pan ?? 0,
                        fullLabel: `${channel.label} pan`,
                        onChange: value => props.onChannelPanChange(channel.sourceId, value),
                        onReset: () => props.onChannelPanChange(channel.sourceId, 0),
                      }
                }
              />
              {unavailable && (
                <p className="text-warning">Source unavailable — edits disabled.</p>
              )}
            </div>
          );
        })}
      </section>
      <LatencyDiagnostics snapshot={diagnostics} />
      <div ref={dock} className="iem-master-dock">
        <div className="iem-dock-inner">
          <div className="iem-row">
            <strong className="text-label">Master</strong>
            <span className="font-mono text-caption">
              {masterMuted ? 'Muted (−∞ dB)' : `${masterRequestedDb} dB`}
            </span>
          </div>
          <div className="iem-dock-actions">
            <Button
              onClick={() => onMasterMuteChange(props.masterUnmutePending || !masterMuted)}
              disabled={masterMuted && !armed}
            >
              {props.masterUnmutePending ? 'Cancel output unmute' : masterMuted ? 'Unmute personal output' : 'Mute personal output'}
            </Button>
            <Button variant="destructive" onClick={onStop}>Stop listening</Button>
          </div>
          <details className="iem-master-details">
            <summary>Master attenuation & details</summary>
            <GainControl
              label="Personal Master attenuation"
              valueDb={masterRequestedDb}
              onChange={onMasterGainChange}
              muted={masterMuted}
              disabled={!editable}
            />
            <p className="text-caption">
              {masterAppliedDb === undefined ? 'Not applied' : `Applied ${masterAppliedDb} dB`}{props.masterPending ? ' · Pending' : ''}
            </p>
          </details>
        </div>
      </div>
    </div>
  );
}
