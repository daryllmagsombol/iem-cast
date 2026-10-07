import type { ListenerPhase } from '../protocol';
import { Button } from '../ui/Button';
import { Notice } from '../ui/Notice';
import { StatusBadge } from '../ui/StatusBadge';

export interface JoinViewProps {
  phase: ListenerPhase;
  /** False when the host catalog has no available sources; Start listening stays unavailable. */
  hasSources: boolean;
  /** True while an explicit connect is in flight. */
  connecting: boolean;
  /** Present once a pairing credential has been consumed; used for recovery instructions. */
  joinUrl?: string;
  error: string | null;
  onStartListening(): void;
  onCancel(): void;
}

function phaseStatus(phase: ListenerPhase): { label: string; tone: 'success' | 'warning' | 'danger' | 'neutral' } {
  switch (phase) {
    case 'unpaired':
      return { label: 'Not paired', tone: 'neutral' };
    case 'paired':
      return { label: 'Paired', tone: 'neutral' };
    case 'negotiating':
      return { label: 'Connecting', tone: 'neutral' };
    case 'ready-muted':
      return { label: 'Connected \u00b7 Not listening', tone: 'warning' };
    case 'armed':
      return { label: 'Listening', tone: 'success' };
    case 'interrupted':
      return { label: 'Interrupted', tone: 'warning' };
    case 'revoked':
      return { label: 'Session ended', tone: 'danger' };
    default:
      return { label: 'Not connected', tone: 'neutral' };
  }
}

/**
 * Join / connected-but-muted gate. Joining starts muted; the first audible step is an
 * explicit gesture (`onStartListening`), which must remain unavailable without sources.
 */
export function JoinView({
  phase,
  hasSources,
  connecting,
  joinUrl,
  error,
  onStartListening,
  onCancel,
}: JoinViewProps) {
  const status = phaseStatus(phase);
  const connected = phase === 'ready-muted' || phase === 'armed';
  const busy = phase === 'negotiating' || connecting;

  return (
    <div className="mx-auto flex w-full max-w-xl flex-col gap-5 px-3 py-5 sm:px-4">
      <header className="flex flex-col gap-2">
        <h1 className="text-title text-text">Personal monitor</h1>
        <StatusBadge tone={status.tone}>{status.label}</StatusBadge>
        <p className="text-body text-text-secondary">
          Keep this page visible and the screen on. Wired earphones only.
        </p>
      </header>

      {error ? (
        <Notice tone="error" title="Something needs attention" live="alert">
          <p>{error}</p>
        </Notice>
      ) : null}

      {!hasSources ? (
        <Notice tone="empty" title="No sources available">
          <p>Ask the operator to publish at least one source before you start listening.</p>
        </Notice>
      ) : null}

      {phase === 'unpaired' ? (
        <Notice tone="empty" title="Waiting for pairing">
          <p>
            Scan the operator&rsquo;s pairing code or open the join link on this device. Pairing is
            single use and expires.
          </p>
          {joinUrl ? (
            <p className="font-mono text-caption break-all text-text-secondary">{joinUrl}</p>
          ) : null}
        </Notice>
      ) : null}

      {phase === 'interrupted' || phase === 'revoked' ? (
        <Notice tone="warning" title="Check wired earphones \u00b7 Start listening again">
          <p>The session stopped and must be armed again explicitly.</p>
        </Notice>
      ) : null}

      <div className="flex flex-col gap-2">
        {connected ? (
          <Button
            variant="primary"
            disabled={!hasSources || phase !== 'ready-muted'}
            pending={busy}
            explanation={
              !hasSources
                ? 'Start listening is unavailable until the operator publishes a source.'
                : phase === 'armed'
                  ? 'Already listening.'
                  : undefined
            }
            onClick={onStartListening}
          >
            {phase === 'armed' ? 'Listening' : 'Start listening'}
          </Button>
        ) : (
          <Button variant="secondary" onClick={onStartListening} pending={busy}>
            {busy ? 'Connecting' : 'Connect'}
          </Button>
        )}
        <Button variant="secondary" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
