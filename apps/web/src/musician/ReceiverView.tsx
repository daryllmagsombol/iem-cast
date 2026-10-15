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

/** A usable source is present, available, and authorized. */
function isUsableSource(source: SourceInfo | undefined): source is SourceInfo {
  return !!source && source.available && source.authorized;
}

/** The peer-resolution facts for one channel. */
interface LinkInfo {
  source: SourceInfo | undefined;
  peerId: string | null;
  peer: SourceInfo | undefined;
  peerChannel: ChannelStripState | undefined;
}

/** Resolve a channel's source, its declared peer id, peer, and peer channel. */
function linkInfo(
  channel: ChannelStripState,
  sourceById: Map<string, SourceInfo>,
  channels: ChannelStripState[],
): LinkInfo {
  const source = sourceById.get(channel.sourceId);
  const peerId = source?.stereoPair ?? null;
  const peer = peerId ? sourceById.get(peerId) : undefined;
  const peerChannel = peerId ? channels.find(c => c.sourceId === peerId) : undefined;
  return { source, peerId, peer, peerChannel };
}

/** True only for a reciprocal, usable, not-already-rendered stereo pair. */
function isValidPair(
  info: LinkInfo,
  peerAlreadySeen: boolean,
): info is LinkInfo & { source: SourceInfo; peer: SourceInfo; peerChannel: ChannelStripState } {
  const { source, peer, peerChannel } = info;
  return (
    !!source &&
    !!peer &&
    !!peerChannel &&
    peer.stereoPair === source.sourceId &&
    !peerAlreadySeen &&
    source.role === peer.role &&
    source.stereoSide !== null &&
    peer.stereoSide !== null &&
    source.stereoSide !== peer.stereoSide
  );
}

/** The peer/channel a failed pair may still reuse, unless a prior unit already rendered them. */
function reusableMember(
  peer: SourceInfo | undefined,
  peerChannel: ChannelStripState | undefined,
  seen: Set<string>,
): { peer: SourceInfo | null; peerChannel: ChannelStripState | null } {
  if (peer && peerChannel && !seen.has(peer.sourceId)) {
    return { peer, peerChannel };
  }
  return { peer: null, peerChannel: null };
}

/** Left/right member label; falls back to the plain label when the side is unknown. */
function sideLabel(source: SourceInfo): string {
  if (!source.stereoSide) return source.label;
  return `${source.stereoSide === 'left' ? 'Left' : 'Right'} · ${source.label}`;
}

type ChannelGroup =
  | {
      kind: 'pair';
      left: SourceInfo;
      right: SourceInfo;
      leftChannel: ChannelStripState;
      rightChannel: ChannelStripState;
      disabled: boolean;
    }
  | {
      kind: 'invalid-pair';
      source: SourceInfo;
      peer: SourceInfo | null;
      channel: ChannelStripState;
      peerChannel: ChannelStripState | null;
    }
  | { kind: 'mono'; source: SourceInfo | undefined; channel: ChannelStripState };

interface GroupContext {
  sourceById: Map<string, SourceInfo>;
  channels: ChannelStripState[];
  seen: Set<string>;
  editable: boolean;
}

/**
 * Classify one channel: an unpaired source (or a source with no metadata) stays mono; a source that
 * declares a `stereoPair` is a valid reciprocal pair, or a disabled group. Adjacency is never used
 * to infer a pair, and an incomplete pair never degrades into independent mono pan controls.
 */
function groupForChannel(channel: ChannelStripState, ctx: GroupContext): ChannelGroup {
  const info = linkInfo(channel, ctx.sourceById, ctx.channels);
  const { source, peerId } = info;
  if (!peerId || !source) return { kind: 'mono', source, channel };
  const peerAlreadySeen = !!info.peer && ctx.seen.has(info.peer.sourceId);
  if (!isValidPair(info, peerAlreadySeen)) {
    // Skip a peer that a prior unit already rendered so it cannot be duplicated.
    const reusable = reusableMember(info.peer, info.peerChannel, ctx.seen);
    if (reusable.peerChannel) ctx.seen.add(reusable.peerChannel.sourceId);
    return {
      kind: 'invalid-pair',
      source,
      peer: reusable.peer,
      channel,
      peerChannel: reusable.peerChannel,
    };
  }
  const { peer, peerChannel } = info;
  ctx.seen.add(peer.sourceId);
  const left = source.stereoSide === 'left' ? source : peer;
  const right = source.stereoSide === 'left' ? peer : source;
  const leftChannel = left.sourceId === channel.sourceId ? channel : peerChannel;
  const rightChannel = right.sourceId === channel.sourceId ? channel : peerChannel;
  return {
    kind: 'pair',
    left,
    right,
    leftChannel,
    rightChannel,
    disabled: !ctx.editable || !(isUsableSource(left) && isUsableSource(right)),
  };
}

/** Stable React key for a rendered group. */
function groupKey(group: ChannelGroup): string {
  if (group.kind === 'pair') return group.left.sourceId;
  if (group.kind === 'invalid-pair') return group.source.sourceId;
  return group.channel.sourceId;
}

/** A valid reciprocal pair: two member strips plus one shared balance. */
function PairGroupView({
  group,
  viewProps,
  armed,
}: {
  readonly group: Extract<ChannelGroup, { kind: 'pair' }>;
  readonly viewProps: ReceiverViewProps;
  readonly armed: boolean;
}) {
  const { left, right, leftChannel, rightChannel, disabled } = group;
  const position = leftChannel.pan ?? rightChannel.pan ?? 0;
  const unavailable =
    !left.available || !left.authorized || !right.available || !right.authorized;
  return (
    <section
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
        meterValueDbfs={viewProps.meters[left.sourceId]}
        onGainChange={db => viewProps.onChannelGainChange(left.sourceId, db)}
        onMuteChange={m => viewProps.onChannelMuteChange(left.sourceId, m)}
      />
      <ChannelStrip
        state={rightChannel}
        fullLabel={right.label}
        disabled={disabled}
        canUnmute={armed}
        linkLabel={`Right · ${right.label}`}
        meterValueDbfs={viewProps.meters[right.sourceId]}
        onGainChange={db => viewProps.onChannelGainChange(right.sourceId, db)}
        onMuteChange={m => viewProps.onChannelMuteChange(right.sourceId, m)}
      />
      <div className="flex flex-col gap-1">
        <PanControl
          label="Balance"
          fullLabel={`${left.label} / ${right.label} balance`}
          value={position}
          disabled={disabled}
          onChange={value => viewProps.onPairBalanceChange(left.sourceId, right.sourceId, value)}
          onReset={() => viewProps.onPairBalanceChange(left.sourceId, right.sourceId, 0)}
        />
        <p className="text-caption text-text-secondary" data-testid="pair-balance-helper">
          Center keeps left and right unchanged. Moving toward one side turns down the other side.
        </p>
        {unavailable && <p className="text-warning">Source unavailable — edits disabled.</p>}
      </div>
    </section>
  );
}

/** An incomplete/non-reciprocal stereo claim: disabled, never degrading into mono pan. */
function InvalidPairGroupView({
  group,
  viewProps,
  armed,
}: {
  readonly group: Extract<ChannelGroup, { kind: 'invalid-pair' }>;
  readonly viewProps: ReceiverViewProps;
  readonly armed: boolean;
}) {
  const { source, peer, channel, peerChannel } = group;
  const disabled = true;
  return (
    <section
      className="iem-stack rounded-panel border border-boundary p-3"
      aria-label={`${source.label} incomplete stereo pair`}
    >
      <h3 className="text-label text-text">Linked stereo pair</h3>
      <ChannelStrip
        state={channel}
        disabled={disabled}
        canUnmute={armed}
        linkLabel={sideLabel(source)}
        meterValueDbfs={viewProps.meters[source.sourceId]}
        onGainChange={db => viewProps.onChannelGainChange(source.sourceId, db)}
        onMuteChange={m => viewProps.onChannelMuteChange(source.sourceId, m)}
      />
      {peer && peerChannel ? (
        <ChannelStrip
          state={peerChannel}
          disabled={disabled}
          canUnmute={armed}
          linkLabel={sideLabel(peer)}
          meterValueDbfs={viewProps.meters[peer.sourceId]}
          onGainChange={db => viewProps.onChannelGainChange(peer.sourceId, db)}
          onMuteChange={m => viewProps.onChannelMuteChange(peer.sourceId, m)}
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

/** A standalone source strip; an unpaired MasterLr keeps its fixed split and gets no pan. */
function MonoGroupView({
  group,
  viewProps,
  armed,
  editable,
}: {
  readonly group: Extract<ChannelGroup, { kind: 'mono' }>;
  readonly viewProps: ReceiverViewProps;
  readonly armed: boolean;
  readonly editable: boolean;
}) {
  const { source, channel } = group;
  const unavailable = !source?.available || !source.authorized;
  const noPan = !source || source.role === 'masterLr';
  return (
    <div>
      <ChannelStrip
        state={channel}
        disabled={!editable || unavailable}
        canUnmute={armed}
        linkLabel={noPan ? undefined : 'Mono source'}
        meterValueDbfs={viewProps.meters[channel.sourceId]}
        onGainChange={db => viewProps.onChannelGainChange(channel.sourceId, db)}
        onMuteChange={m => viewProps.onChannelMuteChange(channel.sourceId, m)}
        pan={
          noPan
            ? undefined
            : {
                label: 'Pan',
                value: channel.pan ?? 0,
                fullLabel: `${channel.label} pan`,
                onChange: value => viewProps.onChannelPanChange(channel.sourceId, value),
                onReset: () => viewProps.onChannelPanChange(channel.sourceId, 0),
              }
        }
      />
      {unavailable && <p className="text-warning">Source unavailable — edits disabled.</p>}
    </div>
  );
}

/** Dispatch a classified group to its matching presentational component. */
function ReceiverGroup({
  group,
  viewProps,
  armed,
  editable,
}: {
  readonly group: ChannelGroup;
  readonly viewProps: ReceiverViewProps;
  readonly armed: boolean;
  readonly editable: boolean;
}) {
  if (group.kind === 'pair') {
    return <PairGroupView group={group} viewProps={viewProps} armed={armed} />;
  }
  if (group.kind === 'invalid-pair') {
    return <InvalidPairGroupView group={group} viewProps={viewProps} armed={armed} />;
  }
  return <MonoGroupView group={group} viewProps={viewProps} armed={armed} editable={editable} />;
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
    const seen = new Set<string>();
    return channels
      .map(channel => {
        // A peer already consumed by an earlier unit must never render a second time.
        if (seen.has(channel.sourceId)) return null;
        seen.add(channel.sourceId);
        return groupForChannel(channel, { sourceById, channels, seen, editable });
      })
      .filter((group): group is ChannelGroup => group !== null);
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
        {groups.map(group => (
          <ReceiverGroup
            key={groupKey(group)}
            group={group}
            viewProps={props}
            armed={armed}
            editable={editable}
          />
        ))}
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
