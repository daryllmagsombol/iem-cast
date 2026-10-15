import { useId, type KeyboardEvent } from 'react';

export interface PanControlProps {
  readonly label: string;
  /** Controlled normalized position: -1 = full left, 0 = center, 1 = full right. */
  readonly value: number;
  onChange(value: number): void;
  onReset(): void;
  readonly disabled?: boolean;
  /** Complete accessible name including source/pair and purpose, e.g. "Keys balance". */
  readonly fullLabel?: string;
}

function boundedPercent(value: number): number {
  return Math.min(100, Math.max(-100, Math.round(value)));
}

function positionText(percent: number, spoken = false): string {
  if (percent === 0) return 'Center';
  const direction = percent < 0 ? 'left' : 'right';
  if (Math.abs(percent) === 100) return `Full ${direction}`;
  return `${Math.abs(percent)}${spoken ? ' percent' : '%'} ${direction}`;
}

const buttonClass = 'inline-flex min-h-target min-w-target shrink-0 items-center justify-center rounded-control border border-boundary bg-surface px-3 text-label text-text transition-colors duration-100 hover:bg-surface-raised disabled:cursor-not-allowed disabled:text-text-secondary';

/** Position only: no gain, mute, persistence, or playback behavior. Percentages stay inside the UI. */
export function PanControl({ label, value, onChange, onReset, disabled = false, fullLabel }: PanControlProps) {
  const rangeId = useId();
  const descriptionId = useId();
  const available = Number.isFinite(value);
  const inactive = disabled || !available;
  const percent = available ? boundedPercent(value * 100) : 0;
  const accessibleName = fullLabel ?? label;

  function changePercent(next: number) {
    if (inactive || !Number.isFinite(next)) return;
    const bounded = boundedPercent(next);
    if (bounded / 100 !== value) onChange(bounded / 100);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (inactive || event.altKey || event.ctrlKey || event.metaKey) return;
    let next: number;
    switch (event.key) {
      case 'ArrowLeft':
      case 'ArrowDown': next = percent - 1; break;
      case 'ArrowRight':
      case 'ArrowUp': next = percent + 1; break;
      case 'PageDown': next = percent - 10; break;
      case 'PageUp': next = percent + 10; break;
      case 'Home': next = -100; break;
      case 'End': next = 100; break;
      default: return;
    }
    // Consistent range steps across browsers; prevent a second native increment or page scroll.
    event.preventDefault();
    changePercent(next);
  }

  return (
    <div className="flex min-w-0 flex-col gap-2">
      <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-2">
        <label htmlFor={rangeId} className="min-w-0 text-label text-text [overflow-wrap:anywhere]">{label}</label>
        <span className="font-mono text-readout tabular-nums text-text" data-testid="pan-readout">
          {available ? positionText(percent) : 'Unavailable'}
        </span>
      </div>
      <p id={descriptionId} className="sr-only">
        Left, center, right. Arrow keys move one percent; Page Up and Page Down move ten percent. Home moves fully left; End moves fully right.
      </p>
      <div className="flex min-w-0 items-start gap-2">
        <button type="button" className={buttonClass} aria-label={`Move ${accessibleName} left`}
          disabled={inactive || percent <= -100} onClick={() => changePercent(percent - 1)}>
          <span aria-hidden="true">L</span>
        </button>
        <div className="flex min-w-0 flex-1 flex-col">
          <input id={rangeId} type="range" min={-100} max={100} step={1} value={percent}
            className="iem-range min-h-target w-full min-w-0 cursor-pointer accent-accent disabled:cursor-not-allowed"
            disabled={inactive} aria-label={accessibleName}
            aria-valuetext={available ? positionText(percent, true) : 'Unavailable'}
            aria-describedby={descriptionId}
            onChange={event => changePercent(Number(event.target.value))} onKeyDown={handleKeyDown} />
          <div aria-hidden="true" className="flex justify-between font-mono text-caption text-text-secondary">
            <span>L</span><span>C</span><span>R</span>
          </div>
        </div>
        <button type="button" className={buttonClass} aria-label={`Move ${accessibleName} right`}
          disabled={inactive || percent >= 100} onClick={() => changePercent(percent + 1)}>
          <span aria-hidden="true">R</span>
        </button>
      </div>
      <div className="flex justify-end">
        <button type="button" className={buttonClass} aria-label={`Center ${accessibleName}`}
          disabled={inactive || value === 0} onClick={() => { if (!inactive && value !== 0) onReset(); }}>
          Center
        </button>
      </div>
    </div>
  );
}
