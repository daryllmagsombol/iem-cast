import type { ReactNode } from 'react';

export type NoticeTone = 'empty' | 'warning' | 'error' | 'reconnect';

export interface NoticeProps {
  tone?: NoticeTone;
  title: string;
  children?: ReactNode;
  /** Optional actionable control, e.g. retry or setup guidance. */
  action?: ReactNode;
  /** `status` for non-urgent, `alert` for errors. */
  live?: 'status' | 'alert' | 'off';
}

const TONE_CLASSES: Record<NoticeTone, string> = {
  empty: 'text-text-secondary',
  warning: 'text-warning',
  error: 'text-danger',
  reconnect: 'text-text',
};

export function Notice({ tone = 'empty', title, children, action, live = 'status' }: NoticeProps) {
  const isAlert = live === 'alert';
  return (
    <section
      role={isAlert ? 'alert' : 'status'}
      aria-live={live === 'off' ? 'off' : 'polite'}
      className={`flex flex-col gap-2 rounded-panel border border-boundary bg-surface p-4 ${TONE_CLASSES[tone]}`}
    >
      <h2 className="text-label text-current">{title}</h2>
      {children ? <div className="text-body text-text-secondary">{children}</div> : null}
      {action ? <div className="flex flex-wrap gap-2">{action}</div> : null}
    </section>
  );
}
