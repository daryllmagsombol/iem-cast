import { useId } from 'react';
import { DigitalMeter } from './DigitalMeter';
import { GainControl } from './GainControl';

export interface ChannelStripState {
  sourceId: string;
  label: string;
  requestedDb: number;
  appliedDb?: number;
  muted: boolean;
  appliedMuted?: boolean;
  pending?: boolean;
  accepted?: boolean;
}

export interface ChannelStripProps {
  state: ChannelStripState;
  onGainChange(db: number): void;
  onMuteChange(muted: boolean): void;
  /** Optional stereo-link description, e.g. "Linked stereo pair". */
  linkLabel?: string;
  /** Optional digital peak in dBFS; `undefined` shows Unavailable. */
  meterValueDbfs?: number;
  /** Full accessible name when the visible label is ellipsized. */
  fullLabel?: string;
  /** Adds SIMULATED badges (Pages demo only). */
  simulated?: boolean;
  disabled?: boolean;
  canUnmute?: boolean;
}

export function ChannelStrip({
  state,
  onGainChange,
  onMuteChange,
  linkLabel,
  meterValueDbfs,
  fullLabel,
  simulated = false,
  disabled = false,
  canUnmute = true,
}: ChannelStripProps) {
  const muteId = useId();
  const pending = state.pending ?? (state.appliedDb !== undefined && state.requestedDb !== state.appliedDb);
  const accessibleName = fullLabel ?? state.label;

  return (
    <article
      className="flex flex-col gap-3 rounded-panel border border-boundary bg-surface p-4"
      aria-label={accessibleName}
    >
      <header className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h3 className="min-w-0 truncate text-label text-text" title={accessibleName}>
          {state.label}
        </h3>
        <span className="text-caption text-text-secondary">Channel</span>
      </header>
      <details className="iem-source-details"><summary>Source details</summary><p>{accessibleName}</p><p className="text-caption text-text-secondary">Source identity: {state.sourceId}</p></details>

      {linkLabel ? <p className="text-caption text-text-secondary">{linkLabel}</p> : null}

      <DigitalMeter value={meterValueDbfs} label={`${state.label} input`} simulated={simulated} />

      <GainControl
        label={state.label}
        valueDb={state.requestedDb}
        muted={state.muted}
        onChange={onGainChange}
        simulated={simulated}
        disabled={disabled}
      />

      <div className="flex flex-col gap-1">
        <p className="font-mono text-caption tabular-nums text-text">
          {pending ? `Requested ${Math.round(state.requestedDb)} dB · Pending` : state.appliedDb === undefined ? 'Not applied' : `Applied ${Math.round(state.appliedDb)} dB`}
        </p>
        {pending && state.appliedDb !== undefined ? (
          <p className="font-mono text-caption tabular-nums text-text-secondary">
            Applied {Math.round(state.appliedDb)} dB
          </p>
        ) : null}
        {state.accepted && pending && <p className="text-caption text-text-secondary">Accepted · Waiting for host application</p>}
        {pending && <p className="text-caption text-text-secondary">Requested mute: {state.muted ? 'Muted' : 'Not muted'}</p>}
        {state.appliedMuted !== undefined && <p className="text-caption text-text-secondary">Applied mute: {state.appliedMuted ? 'Muted' : 'Not muted'}</p>}
      </div>

      <div className="iem-channel-action flex min-h-target items-center justify-between gap-3 border-t border-boundary pt-3">
        <label htmlFor={muteId} className="min-w-0 flex-1 text-label text-text">
          Mute {state.label}
        </label>
        <button
          id={muteId}
          type="button"
          aria-pressed={state.muted}
          aria-label={`${state.muted ? 'Unmute' : 'Mute'} ${state.label}`}
          disabled={disabled || (state.muted && !canUnmute)}
          onClick={() => onMuteChange(!state.muted)}
          className={`inline-flex shrink-0 whitespace-nowrap min-h-target min-w-target items-center justify-center rounded-control border px-3 text-label transition-colors duration-100 ${
            state.muted
              ? 'border-warning bg-surface-raised text-warning'
              : 'border-boundary bg-surface text-text hover:bg-surface-raised'
          }`}
        >
          {state.muted ? 'Unmute' : 'Mute'}
        </button>
      </div>
      {state.muted && !canUnmute && <p className="text-caption text-text-secondary">Start listening before unmuting this channel.</p>}
    </article>
  );
}
