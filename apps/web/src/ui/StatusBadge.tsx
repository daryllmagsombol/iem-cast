import type { ReactNode } from 'react';

export type StatusTone = 'success' | 'warning' | 'danger' | 'neutral';

export interface StatusBadgeProps {
  /** Visible status text. */
  children: ReactNode;
  tone?: StatusTone;
  /** Short status glyph; always accompanied by text. */
  icon?: ReactNode;
}

const TONE_CLASSES: Record<StatusTone, string> = {
  success: 'text-success',
  warning: 'text-warning',
  danger: 'text-danger',
  neutral: 'text-text-secondary',
};

const DEFAULT_ICON: Record<StatusTone, string> = {
  success: '\u25CF',
  warning: '\u25B2',
  danger: '\u25A0',
  neutral: '\u25CB',
};

export function StatusBadge({ children, tone = 'neutral', icon }: StatusBadgeProps) {
  return (
    <span className={`inline-flex items-center gap-1.5 rounded-badge border border-boundary bg-surface px-2 py-1 text-caption ${TONE_CLASSES[tone]}`}>
      <span aria-hidden="true" className="leading-none">
        {icon ?? DEFAULT_ICON[tone]}
      </span>
      <span>{children}</span>
    </span>
  );
}
