import type { ListenerPhase, SourceInfo } from '../protocol';
import type { DiagnosticsSnapshot } from '../receiver-ports';
import { Button } from '../ui/Button';
import { ChannelStrip, type ChannelStripState } from '../ui/ChannelStrip';
import { LatencyDiagnostics } from '../ui/LatencyDiagnostics';
import { Notice } from '../ui/Notice';
import { StatusBadge } from '../ui/StatusBadge';
import { Toggle } from '../ui/Toggle';

export interface ReceiverViewProps {
  phase: ListenerPhase;
  catalog: SourceInfo[];
  channels: ChannelStripState[];
  masterRequestedDb: number;
  masterAppliedDb: number;
  masterMuted: boolean;
  /** Local master mute; acts immediately without confirmation. */
  onMasterMuteChange(muted: boolean): void;
  onMasterGainChange(db: number): void;
  onChannelGainChange(sourceId: string, db: number): void;
  onChannelMuteChange(sourceId: string, muted: boolean): void;
  onStop(): void;
  diagnostics: DiagnosticsSnapshot;
  /** Meter values keyed by sourceId, in dBFS. Missing entries render Unavailable. */
  meters: Record<string, number | undefined>;
  error: string | null;
  wakeLockWarning: string | null;
}

export function ReceiverView({
  phase,
  catalog,
  channels,
  masterRequestedDb,
  masterAppliedDb,
  masterMuted,
  onMasterMuteChange,
  onMasterGainChange,
  onChannelGainChange,
  onChannelMuteChange,
  onStop,
  diagnostics,
  meters,
  error,
  wakeLockWarning,
}: ReceiverViewProps) {
  const armed = phase === 'armed';
  const availableIds = new Set(catalog.filter((source) => source.available).map((source) => source.sourceId));

  return (
    <div
      className="mx-auto flex w-full max-w-xl flex-col gap-6 px-3 sm:px-4"
      style={{
        paddingTop: 'calc(var(--iem-safe-top) + var(--iem-space-5))',
        paddingBottom: 'calc(var(--iem-size-target) * 2 + var(--iem-safe-bottom) + var(--iem-space-5))',
      }}
    >
      <header className="flex flex-col gap-2">
        <h1 className="text-title text-text">Personal monitor</h1>
        <StatusBadge tone={armed ? 'success' : 'warning'}>
          {armed ? 'Listening' : 'Connected \u00b7 Not listening'}
        </StatusBadge>
        <p className="text-body text-text-secondary">Keep this page visible and the screen on.</p>
      </header>

      {error ? (
        <Notice tone="error" title="Connection lost \u00b7 Output silenced" live="alert">
          <p>{error}</p>
        </Notice>
      ) : null}

      {wakeLockWarning ? (
        <Notice tone="warning" title="Screen wake lock unavailable">
          <p>{wakeLockWarning}</p>
        </Notice>
      ) : null}

      <section aria-labelledby="channels-heading" className="flex flex-col gap-4">
        <h2 id="channels-heading" className="text-section text-text">
          Channels
        </h2>
        {channels.length === 0 ? (
          <Notice tone="empty" title="No channels assigned">
            <p>Your personal mix has no channels yet. Ask the operator to authorize a source.</p>
          </Notice>
        ) : (
          <ul className="flex list-none flex-col gap-4 p-0">
            {channels.map((channel) => (
              <li key={channel.sourceId}>
                <ChannelStrip
                  state={channel}
                  linkLabel={linkLabelFor(catalog, channel.sourceId)}
                  meterValueDbfs={meters[channel.sourceId]}
                  onGainChange={(db) => onChannelGainChange(channel.sourceId, db)}
                  onMuteChange={(muted) => onChannelMuteChange(channel.sourceId, muted)}
                />
                {!availableIds.has(channel.sourceId) ? (
                  <p className="mt-1 text-caption text-warning">
                    This source is currently unavailable on the host.
                  </p>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </section>

      <LatencyDiagnostics snapshot={diagnostics} />

      {/* Master dock: immediate local mute and Stop listening. */}
      <div
        className="sticky bottom-0 z-10 -mx-3 mt-2 border-t border-boundary bg-surface px-3 sm:-mx-4 sm:px-4"
        style={{ paddingBottom: 'calc(var(--iem-safe-bottom) + var(--iem-space-3))' }}
      >
        <div className="flex flex-col gap-3 pt-3">
          <div className="flex items-center justify-between gap-3">
            <label htmlFor="master-attenuation" className="text-label text-text">
              Master
            </label>
            <span className="font-mono text-readout tabular-nums text-text">
              {masterMuted ? 'Muted (\u2212\u221E dB)' : `${Math.round(masterRequestedDb)} dB`}
            </span>
          </div>
          <input
            id="master-attenuation"
            type="range"
            className="h-2 w-full cursor-pointer accent-accent"
            min={-60}
            max={0}
            step={1}
            value={Math.min(0, Math.max(-60, masterRequestedDb))}
            onChange={(event) => onMasterGainChange(Number(event.target.value))}
            aria-valuetext={`Personal master, ${
              masterRequestedDb < 0 ? `minus ${Math.abs(Math.round(masterRequestedDb))} decibels` : '0 decibels'
            }`}
          />
          <p className="font-mono text-caption tabular-nums text-text-secondary">
            {masterRequestedDb !== masterAppliedDb
              ? `Requested ${Math.round(masterRequestedDb)} dB \u00b7 Pending`
              : `Applied ${Math.round(masterAppliedDb)} dB`}
          </p>
          <Toggle
            label="Personal master mute"
            checked={masterMuted}
            onChange={onMasterMuteChange}
            onLabel="Muted"
            offLabel="Not muted"
          />
          <Button variant="destructive" onClick={onStop} disabled={phase === 'unpaired' || phase === 'revoked'}>
            Stop listening
          </Button>
        </div>
      </div>
    </div>
  );
}

function linkLabelFor(catalog: SourceInfo[], sourceId: string): string | undefined {
  const entry = catalog.find((source) => source.sourceId === sourceId);
  if (!entry) return undefined;
  if (entry.stereoPair) {
    const pair = catalog.find((source) => source.sourceId === entry.stereoPair);
    return `Linked stereo pair with ${pair?.label ?? 'paired channel'}`;
  }
  return 'Mono source, centered';
}
