import { useState } from 'react';
import type { HostBridge } from '../desktop-bridge/contracts';
import { DeviceView } from './DeviceView';
import { PairingView } from './PairingView';
import { SourcesView } from './SourcesView';

type AdminSection = 'device' | 'sources' | 'pairing';

export interface AdminRootProps {
  /**
   * Frozen operator bridge (Task 1). Optional only so the INT-owned `main.tsx`
   * entrypoint compiles before it is wired; a real bridge is required at runtime.
   */
  bridge?: HostBridge;
}

const SECTIONS: Array<{ id: AdminSection; label: string }> = [
  { id: 'device', label: 'Capture device' },
  { id: 'sources', label: 'Sources' },
  { id: 'pairing', label: 'Pairing' },
];

export function AdminRoot({ bridge }: AdminRootProps) {
  const [section, setSection] = useState<AdminSection>('device');

  if (!bridge) {
    return (
      <div className="min-h-dvh bg-canvas text-text">
        <main className="mx-auto w-full max-w-5xl px-4 py-6">
          <h1 className="text-title text-text">IEM Cast operator</h1>
          <p className="mt-2 rounded-panel border border-boundary bg-surface p-4 text-body text-text-secondary">
            The operator host bridge is not connected. Launch the desktop app to manage capture,
            sources, and pairing.
          </p>
        </main>
      </div>
    );
  }

  return (
    <div className="min-h-dvh bg-canvas text-text">
      <header className="border-b border-boundary bg-surface">
        <div className="mx-auto flex w-full max-w-5xl flex-col gap-3 px-4 py-3 lg:flex-row lg:items-center lg:justify-between">
          <h1 className="text-section text-text">IEM Cast operator</h1>
          <nav aria-label="Operator sections" className="flex flex-wrap gap-2">
            {SECTIONS.map((item) => (
              <button
                key={item.id}
                type="button"
                aria-current={section === item.id ? 'page' : undefined}
                onClick={() => setSection(item.id)}
                className={`min-h-target-compact rounded-control border px-3 text-label transition-colors duration-100 ${
                  section === item.id
                    ? 'border-accent bg-accent text-on-accent'
                    : 'border-boundary bg-surface text-text hover:bg-surface-raised'
                }`}
              >
                {item.label}
              </button>
            ))}
          </nav>
        </div>
      </header>

      <main className="mx-auto w-full max-w-5xl px-4 py-6">
        {section === 'device' ? <DeviceView bridge={bridge} /> : null}
        {section === 'sources' ? <SourcesView bridge={bridge} /> : null}
        {section === 'pairing' ? <PairingView bridge={bridge} /> : null}
      </main>
    </div>
  );
}
