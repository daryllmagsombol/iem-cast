import { useId, type ButtonHTMLAttributes, type ReactNode } from 'react';

export type ButtonVariant = 'primary' | 'secondary' | 'destructive';
export type ButtonSize = 'default' | 'compact';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  /** `default` = 48px target, `compact` = 44px operator density. */
  size?: ButtonSize;
  /** Shows an inline pending state; disabled semantics are preserved. */
  pending?: boolean;
  /** Visible explanation for disabled/pending state, also exposed via aria-describedby. */
  explanation?: string;
  children: ReactNode;
}

const VARIANT_CLASSES: Record<ButtonVariant, string> = {
  // Primary keeps the accent fill on hover/press and adds an on-accent inset outline.
  primary:
    'border-accent bg-accent text-on-accent hover:shadow-[inset_0_0_0_2px_var(--iem-color-on-accent)] active:shadow-[inset_0_0_0_3px_var(--iem-color-on-accent)]',
  secondary: 'border-boundary bg-surface text-text hover:bg-surface-raised active:bg-surface-raised',
  destructive: 'border-danger bg-surface text-danger hover:bg-surface-raised active:bg-surface-raised',
};

const SIZE_CLASSES: Record<ButtonSize, string> = {
  default: 'min-h-target min-w-target px-4',
  compact: 'min-h-target-compact min-w-target-compact px-3',
};

export function Button({
  variant = 'secondary',
  size = 'default',
  pending = false,
  explanation,
  disabled = false,
  type = 'button',
  className,
  children,
  ...rest
}: ButtonProps) {
  const explanationId = useId();
  const isDisabled = disabled || pending;

  const classes = [
    'inline-flex items-center justify-center gap-2 rounded-control border text-label',
    'transition-colors duration-100',
    'disabled:cursor-not-allowed disabled:border-boundary disabled:bg-surface disabled:text-text-secondary',
    VARIANT_CLASSES[variant],
    SIZE_CLASSES[size],
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <span className="inline-flex flex-col items-start gap-1">
      <button
        {...rest}
        type={type}
        className={classes}
        disabled={isDisabled}
        aria-busy={pending || undefined}
        aria-describedby={explanation ? explanationId : undefined}
      >
        {children}
        {pending ? <span className="font-normal">Pending</span> : null}
      </button>
      {explanation ? (
        <span id={explanationId} className="text-caption text-text-secondary">
          {explanation}
        </span>
      ) : null}
    </span>
  );
}
