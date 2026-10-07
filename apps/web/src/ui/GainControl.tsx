import { useEffect, useId, useState } from 'react';

export interface GainControlProps {
  label: string;
  valueDb: number;
  minDb?: number;
  maxDb?: number;
  muted?: boolean;
  onChange(db: number): void;
  /** Adds a visible SIMULATED badge to the readout. Only the Pages demo sets this. */
  simulated?: boolean;
  disabled?: boolean;
}

function formatDbPhrase(db: number): string {
  const rounded = Math.round(db * 10) / 10;
  const magnitude = Math.abs(rounded);
  if (rounded < 0) return `minus ${magnitude} decibels`;
  if (rounded > 0) return `${magnitude} decibels`;
  return '0 decibels';
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * Attenuation-only gain control: native range + readout + decrease/increase buttons
 * + validated numeric entry. No double-tap reset and no scroll-wheel gain change.
 */
export function GainControl({
  label,
  valueDb,
  minDb = -60,
  maxDb = 0,
  muted = false,
  onChange,
  simulated = false,
  disabled = false,
}: GainControlProps) {
  const rangeId = useId();
  const numberId = useId();
  const [entry, setEntry] = useState<string>(String(valueDb));
  const [entryError, setEntryError] = useState<string | null>(null);

  const step = 1;
  const boundedValue = clamp(valueDb, minDb, maxDb);
  const valueText = `${label}, ${formatDbPhrase(boundedValue)}`;

  // Keep the numeric field in step with external value changes.
  useEffect(() => {
    setEntry(String(boundedValue));
    setEntryError(null);
  }, [boundedValue]);

  function commit(db: number) {
    const next = clamp(db, minDb, maxDb);
    onChange(next);
    setEntry(String(next));
    setEntryError(null);
  }

  function commitEntry(raw: string) {
    setEntry(raw);
    if (raw.trim() === '') {
      setEntryError(`Enter a level between ${minDb} and ${maxDb} dB.`);
      return;
    }
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) {
      setEntryError(`Enter a number between ${minDb} and ${maxDb} dB.`);
      return;
    }
    const rounded = clamp(Math.round(parsed), minDb, maxDb);
    if (rounded !== parsed) {
      setEntryError(`Levels are whole dB between ${minDb} and ${maxDb}; using ${rounded} dB.`);
    } else {
      setEntryError(null);
    }
    onChange(rounded);
    setEntry(String(rounded));
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <label htmlFor={rangeId} className="text-label text-text">
          {label}
        </label>
        <span className="font-mono text-readout tabular-nums text-text" data-testid="gain-readout">
          {muted ? 'Muted (\u2212\u221E dB)' : `${Math.round(boundedValue)} dB`}
          {simulated ? <span className="ml-1 font-sans text-caption text-warning">SIMULATED</span> : null}
        </span>
      </div>

      <div className="flex min-h-target items-center gap-2">
        <button
          type="button"
          className="inline-flex min-h-target min-w-target items-center justify-center rounded-control border border-boundary bg-surface text-text transition-colors duration-100 hover:bg-surface-raised"
          onClick={() => commit(boundedValue - step)}
          disabled={disabled || boundedValue <= minDb}
          aria-label={`Decrease ${label} gain`}
        >
          <span aria-hidden="true">&minus;</span>
        </button>
        <input
          id={rangeId}
          type="range"
          className="iem-range min-w-0 flex-1 cursor-pointer accent-accent disabled:cursor-not-allowed"
          disabled={disabled}
          min={minDb}
          max={maxDb}
          step={step}
          value={boundedValue}
          onChange={(event) => commit(Number(event.target.value))}
          aria-valuetext={valueText}
        />
        <button
          type="button"
          className="inline-flex min-h-target min-w-target items-center justify-center rounded-control border border-boundary bg-surface text-text transition-colors duration-100 hover:bg-surface-raised"
          onClick={() => commit(boundedValue + step)}
          disabled={disabled || boundedValue >= maxDb}
          aria-label={`Increase ${label} gain`}
        >
          <span aria-hidden="true">+</span>
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <label htmlFor={numberId} className="text-caption text-text-secondary">
          {label} level in decibels
        </label>
        <input
          id={numberId}
          type="number"
          inputMode="numeric"
          className="min-h-target w-24 rounded-control border border-boundary bg-surface-raised px-2 font-mono text-readout tabular-nums text-text"
          disabled={disabled}
          min={minDb}
          max={maxDb}
          step={step}
          value={entry}
          onChange={(event) => commitEntry(event.target.value)}
          aria-invalid={entryError ? true : undefined}
          aria-describedby={entryError ? `${numberId}-error` : undefined}
        />
      </div>
      {entryError ? (
        <p id={`${numberId}-error`} className="text-caption text-danger">
          {entryError}
        </p>
      ) : null}
    </div>
  );
}
