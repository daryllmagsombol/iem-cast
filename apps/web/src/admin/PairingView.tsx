import { useEffect, useState } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import type { HostBridge, PairingCredential } from '../desktop-bridge/contracts';
import { Button } from '../ui/Button';
import { Notice } from '../ui/Notice';

export interface PairingViewProps {
  bridge: HostBridge;
}

interface IssuedCredential {
  credential: PairingCredential;
  issuedAt: number;
}

export function PairingView({ bridge }: PairingViewProps) {
  const [issued, setIssued] = useState<IssuedCredential | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!issued) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [issued]);

  async function generate() {
    setPending(true);
    setError(null);
    try {
      const credential = await bridge.issuePairingCredential();
      setIssued({ credential, issuedAt: Date.now() });
      setNow(Date.now());
    } catch {
      setError('Could not issue a pairing credential.');
    } finally {
      setPending(false);
    }
  }

  const remainingSeconds =
    issued === null
      ? null
      : Math.max(0, issued.credential.expiresInSeconds - Math.floor((now - issued.issuedAt) / 1000));

  const expired = remainingSeconds !== null && remainingSeconds <= 0;

  return (
    <div className="flex flex-col gap-5">
      <header className="flex flex-col gap-1">
        <h1 className="text-title text-text">Pair a phone</h1>
        <p className="text-body text-text-secondary">
          Each phone needs its own single-use code. The link is shown once and is never written to logs.
        </p>
      </header>

      {error ? (
        <Notice tone="error" title="Pairing failed" live="alert">
          <p>{error}</p>
        </Notice>
      ) : null}

      <div className="flex flex-wrap items-start gap-5">
        <div className="flex min-h-target flex-col gap-3">
          <Button variant="primary" pending={pending} onClick={() => void generate()}>
            Generate pairing code
          </Button>
          <p className="text-caption text-text-secondary">
            A fresh code is created for every phone; the first two phones cannot share one code.
          </p>
        </div>

        {issued ? (
          <section
            aria-label="Pairing credential"
            className="flex flex-col items-center gap-3 rounded-panel border border-boundary bg-surface p-4"
          >
            <div className="rounded-control bg-white p-3" role="img" aria-label="Pairing QR code">
              <QRCodeSVG value={issued.credential.joinUrl} size={160} level="M" />
            </div>
            <p className="font-mono text-readout tabular-nums text-text" role="timer" aria-live="off">
              {expired ? 'Expires in 0 s' : `Expires in ${remainingSeconds} s`}
            </p>
            <p className="max-w-xs text-center text-caption break-all text-text-secondary">
              Open the pairing link on the phone, or scan this code. It works once.
            </p>
          </section>
        ) : (
          <Notice tone="empty" title="No active pairing code">
            <p>Generate a code when a musician is ready to join.</p>
          </Notice>
        )}
      </div>
    </div>
  );
}
