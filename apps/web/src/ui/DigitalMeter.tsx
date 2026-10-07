import { useId } from 'react';

export interface DigitalMeterProps {
  /** Digital peak in dBFS, or `undefined` when telemetry is unavailable. */
  value: number | undefined;
  /** Optional identity label, e.g. "Lead vocal input" or "Personal output L". */
  label?: string;
  /** Optional stale marker; stale values render "Unavailable". */
  stale?: boolean;
  /**
   * Adds a visible SIMULATED text badge. Only the GitHub Pages sample-data demo
   * sets this; real receiver/admin telemetry must not claim to be simulated.
   */
  simulated?: boolean;
}

const SEGMENT_COUNT = 12;
// -60 dBFS .. 0 dBFS across the segments.
const FLOOR_DBFS = -60;
const CEILING_DBFS = 0;

function normalize(valueDb: number): number {
  const clamped = Math.min(CEILING_DBFS, Math.max(FLOOR_DBFS, valueDb));
  return (clamped - FLOOR_DBFS) / (CEILING_DBFS - FLOOR_DBFS);
}

export function DigitalMeter({ value, label, stale = false, simulated = false }: DigitalMeterProps) {
  const labelId = useId();
  const unavailable = value === undefined || stale;
  const finiteValue = unavailable ? undefined : Math.round(value * 10) / 10;
  const litCount = finiteValue === undefined ? 0 : Math.round(normalize(finiteValue) * SEGMENT_COUNT);
  const clipping = finiteValue !== undefined && finiteValue >= 0;

  const readout = finiteValue === undefined ? 'Unavailable' : `${finiteValue.toFixed(1)} dBFS`;
  const level = finiteValue === undefined ? '' : clipping ? 'Clipping' : finiteValue >= -6 ? 'High level' : 'Normal level';

  return (
    <figure className="flex flex-col gap-1" role="group" aria-labelledby={labelId}>
      <figcaption id={labelId} className="text-caption text-text-secondary">
        {label ?? 'Digital level'}
        {simulated ? <span className="ml-1 text-caption text-warning">SIMULATED</span> : null}
      </figcaption>
      <div className="flex h-3 gap-0.5 overflow-hidden rounded-badge border border-boundary bg-surface p-0.5">
        {Array.from({ length: SEGMENT_COUNT }, (_, index) => {
          const position = index / (SEGMENT_COUNT - 1);
          const segmentColor = position >= 0.92 ? 'bg-danger' : position >= 0.75 ? 'bg-warning' : 'bg-success';
          return (
            <span
              key={index}
              aria-hidden="true"
              className={`h-full flex-1 ${index < litCount ? segmentColor : 'bg-surface'}`}
            />
          );
        })}
      </div>
      <p className="font-mono text-caption tabular-nums text-text">
        <span aria-hidden="true">Digital peak </span>
        {readout}
        {level ? <span className="ml-2 text-text-secondary">{level}</span> : null}
      </p>
      {finiteValue !== undefined ? (
        <div
          className="sr-only"
          role="meter"
          aria-label={label ?? 'Digital level'}
          aria-valuenow={finiteValue}
          aria-valuemin={FLOOR_DBFS}
          aria-valuemax={CEILING_DBFS}
          aria-valuetext={`${finiteValue.toFixed(1)} dBFS`}
        />
      ) : null}
    </figure>
  );
}
