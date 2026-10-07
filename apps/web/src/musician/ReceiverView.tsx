import { useLayoutEffect, useRef, useState } from 'react';
import type { ListenerPhase, SourceInfo } from '../protocol';
import type { DiagnosticsSnapshot } from '../receiver-ports';
import { Button } from '../ui/Button';
import { ChannelStrip, type ChannelStripState } from '../ui/ChannelStrip';
import { LatencyDiagnostics } from '../ui/LatencyDiagnostics';
import { GainControl } from '../ui/GainControl';
import { ThemeChoice } from '../ui/ThemeChoice';

export interface ReceiverViewProps {
  phase: ListenerPhase;
  catalog: SourceInfo[];
  channels: ChannelStripState[];
  masterRequestedDb: number;
  masterAppliedDb?: number;
  masterMuted: boolean;
  masterPending?: boolean;
  onMasterMuteChange(muted: boolean): void;
  onMasterGainChange(db: number): void;
  onChannelGainChange(sourceId: string, db: number): void;
  onChannelMuteChange(sourceId: string, muted: boolean): void;
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
          {error} · Listening state must be checked before restarting.
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
        {channels.map(channel => {
          const source = catalog.find(s => s.sourceId === channel.sourceId);
          const unavailable = !source?.available || !source.authorized;
          return (
            <div key={channel.sourceId}>
              <ChannelStrip
                state={channel}
                disabled={!editable || unavailable}
                canUnmute={armed}
                linkLabel={source?.stereoPair ? 'Linked stereo pair' : 'Mono source, centered'}
                meterValueDbfs={props.meters[channel.sourceId]}
                onGainChange={db => props.onChannelGainChange(channel.sourceId, db)}
                onMuteChange={m => props.onChannelMuteChange(channel.sourceId, m)}
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
              onClick={() => onMasterMuteChange(!masterMuted)}
              disabled={masterMuted && !armed}
            >
              {masterMuted ? 'Unmute personal output' : 'Mute personal output'}
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
