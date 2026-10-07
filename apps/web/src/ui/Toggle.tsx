import { useId, type ReactNode } from 'react';

export interface ToggleProps {
  /** Visible label; also the control's accessible name. */
  label: string;
  checked: boolean;
  onChange(checked: boolean): void;
  /** Explicit visible state wording, e.g. "Muted" / "Not muted". */
  onLabel?: string;
  offLabel?: string;
  description?: ReactNode;
  disabled?: boolean;
  id?: string;
}

/**
 * Native checkbox with switch semantics. State is conveyed by the native checked
 * state (`role="switch"` + aria-checked) and restated in visible text.
 */
export function Toggle({
  label,
  checked,
  onChange,
  onLabel = 'On',
  offLabel = 'Off',
  description,
  disabled = false,
  id,
}: ToggleProps) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const descriptionId = useId();

  return (
    <div className="flex min-h-target items-center justify-between gap-3">
      <span className="min-w-0">
        <label htmlFor={inputId} className="block text-label text-text">
          {label}
        </label>
        {description ? (
          <span id={descriptionId} className="block text-caption text-text-secondary">
            {description}
          </span>
        ) : null}
      </span>
      <span className="inline-flex shrink-0 items-center gap-2">
        <span className={checked ? 'text-caption text-accent' : 'text-caption text-text-secondary'} aria-hidden="true">
          {checked ? onLabel : offLabel}
        </span>
        <span className="relative inline-flex">
          <input
            id={inputId}
            type="checkbox"
            role="switch"
            checked={checked}
            disabled={disabled}
            onChange={(event) => onChange(event.target.checked)}
            aria-describedby={description ? descriptionId : undefined}
            className="peer inline-flex h-6 w-11 cursor-pointer appearance-none rounded-full border border-boundary bg-surface-raised checked:border-accent checked:bg-accent disabled:cursor-not-allowed disabled:opacity-100 disabled:border-boundary disabled:bg-surface"
          />
          <span
            aria-hidden="true"
            className="pointer-events-none absolute top-1/2 left-1 h-4 w-4 -translate-y-1/2 rounded-full bg-surface transition-transform duration-100 peer-checked:translate-x-5 peer-disabled:bg-text-secondary"
          />
        </span>
      </span>
    </div>
  );
}
