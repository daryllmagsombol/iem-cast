import { useEffect, useState } from 'react';
import type { HostBridge } from '../desktop-bridge/contracts';
import type { DeviceInfo, InterfaceInfo } from '../protocol';
import { DeviceView } from './DeviceView';
import { SourcesView } from './SourcesView';
import { PairingView } from './PairingView';
import { Button } from '../ui/Button';
import { ThemeChoice } from '../ui/ThemeChoice';
import { SimulatedPreview } from '../preview/SimulatedPreview';

export interface AdminRootProps {
  bridge?: HostBridge;
}

const steps = [
  'Capture device',
  'Network & certificates',
  'Host controls',
  'Sources',
  'Pairing & local monitor',
];

export function AdminRoot({ bridge }: AdminRootProps) {
  const [preview, setPreview] = useState(false);
  const [device, setDevice] = useState<DeviceInfo | null>(null);
  const [interfaces, setInterfaces] = useState<InterfaceInfo[]>([]);
  const [interfaceIp, setInterfaceIp] = useState('');
  const [interfaceError, setInterfaceError] = useState<string | null>(null);
  const [scan, setScan] = useState(0);
  const [certificate, setCertificate] = useState('');
  const [key, setKey] = useState('');
  const [currentStep, setCurrentStep] = useState(0);
  useEffect(() => {
    if (!bridge) return;
    let current = true;
    setInterfaceError(null);
    bridge
      .listInterfaces()
      .then(list => {
        if (current) {
          setInterfaces(list);
          setInterfaceIp('');
        }
      })
      .catch(e => {
        if (current) {
          setInterfaces([]);
          setInterfaceError(
            e instanceof Error ? e.message : 'Could not list network interfaces.',
          );
        }
      });
    return () => {
      current = false;
    };
  }, [bridge, scan]);

  if (preview) return <SimulatedPreview onExit={() => setPreview(false)} />;

  return (
    <div className="iem-shell">
      <a className="iem-skip" href="#operator-content">Skip to setup</a>
      <header className="iem-topbar">
        <div>
          <p className="text-caption text-text-secondary">IEM Cast · Operator</p>
          <h1 className="text-title">Operator dashboard</h1>
        </div>
        <div className="iem-row">
          <span className="iem-environment">
            {bridge ? 'Desktop app · Native IPC available' : 'Browser · Native access unavailable'}
          </span>
          <ThemeChoice />
        </div>
      </header>
      <div className="iem-dashboard">
        <aside className="iem-nav">
          <p className="text-label">Setup</p>
          <nav aria-label="Setup steps">
            {steps.map((step, i) => (
              <a
                key={step}
                href={`#setup-${i + 1}`}
                aria-current={currentStep === i ? 'location' : undefined}
                onClick={() => setCurrentStep(i)}
              >
                <span className="iem-step-number">{i + 1}</span>
                <span className="iem-nav-step-label">{step}</span>
                {currentStep === i && <span className="iem-current-step">Current</span>}
              </a>
            ))}
          </nav>
          <p className="text-caption text-text-secondary">Work through setup without starting capture or audio.</p>
          <Button
            variant={bridge ? 'secondary' : 'primary'}
            onClick={() => setPreview(true)}
          >
            Open simulated preview
          </Button>
        </aside>
        <main id="operator-content" className="iem-stack">
          <section className="iem-panel iem-summary" aria-labelledby="host-summary">
            <h2 id="host-summary" className="text-section">Host summary</h2>
            <div className="iem-summary-grid">
              {[
                ['Capture', 'Not confirmed'],
                ['HTTPS join service', 'Unavailable'],
                ['Phone audio', 'Unavailable'],
              ].map(([label, value]) => (
                <div key={label}>
                  <p className="text-caption text-text-secondary">{label}</p>
                  <p className="text-label">{value}</p>
                </div>
              ))}
            </div>
            <p className="text-caption text-text-secondary">IPC availability is not a running capture, server, or audio connection. No operation is reported running.</p>
          </section>
          <section id="setup-1" className="iem-panel">
            <DeviceView bridge={bridge} onSelect={setDevice} />
          </section>
          <section id="setup-2" className="iem-panel iem-stack">
            <div className="iem-row">
              <h2 className="text-section">Network & certificates</h2>
              <Button disabled={!bridge} onClick={() => setScan(n => n + 1)}>
                Rescan interfaces
              </Button>
            </div>
            <p className="text-text-secondary">Setup values only — entering paths does not load a certificate or establish trust.</p>
            {interfaceError && (
              <p role="alert" className="text-danger">{interfaceError}</p>
            )}
            <label className="iem-field">
              Host network interface
              <select
                value={interfaceIp}
                disabled={!interfaces.length}
                onChange={e => setInterfaceIp(e.target.value)}
              >
                <option value="">
                  {interfaces.length ? 'Choose a reported interface' : 'Unavailable'}
                </option>
                {interfaces.map(i => (
                  <option key={`${i.name}-${i.ipAddress}`} value={i.ipAddress}>
                    {i.name} · {i.ipAddress}
                  </option>
                ))}
              </select>
            </label>
            <div className="iem-split">
              <label className="iem-field">
                Certificate path
                <input
                  value={certificate}
                  onChange={e => setCertificate(e.target.value)}
                  placeholder="/local/path/certificate.pem"
                  autoComplete="off"
                  spellCheck={false}
                />
              </label>
              <label className="iem-field">
                Key path
                <input
                  value={key}
                  onChange={e => setKey(e.target.value)}
                  placeholder="/local/path/key.pem"
                  autoComplete="off"
                  spellCheck={false}
                />
              </label>
            </div>
            <p className="iem-hint">Use a certificate matching the selected LAN address. Enroll and trust it manually on each phone; this app does not install trust or bypass certificate warnings.</p>
          </section>
          <section id="setup-3" className="iem-panel iem-stack">
            <h2 className="text-section">Host controls</h2>
            <dl className="iem-review">
              <dt>Selected device</dt>
              <dd>{device?.name ?? 'Not selected'}</dd>
              <dt>Network address</dt>
              <dd>{interfaceIp || 'Not selected'}</dd>
              <dt>Certificate paths</dt>
              <dd>{certificate && key ? 'Entered · Not validated' : 'Not entered'}</dd>
            </dl>
            <div className="iem-row">
              <Button
                variant="primary"
                disabled
                explanation="Host startup is incomplete in this build."
              >
                Start host
              </Button>
              <Button
                variant="destructive"
                disabled
                explanation="No operation is reported running."
              >
                Stop host
              </Button>
            </div>
          </section>
          <section id="setup-4" className="iem-panel">
            <SourcesView bridge={bridge} />
          </section>
          <section id="setup-5" className="iem-panel">
            <PairingView bridge={bridge} />
          </section>
          <p className="text-caption text-text-secondary">POC work in progress · Not stage qualified. Network statistics cannot establish hearing safety or full-path audio latency.</p>
        </main>
      </div>
    </div>
  );
}
