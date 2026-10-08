import { useEffect, useRef, useState } from 'react';
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
  const [host, setHost] = useState<{ joinUrl: string; hostEpoch: string; audioEpoch: string } | null>(
    null,
  );
  const [hostBusy, setHostBusy] = useState(false);
  const [hostError, setHostError] = useState<string | null>(null);
  // Backend-discovered defaults are applied once per bridge; a rescan must not clobber edits.
  const defaultsAppliedFor = useRef<HostBridge | null>(null);
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
      })
      .finally(() => {
        if (!current || defaultsAppliedFor.current === bridge) return;
        defaultsAppliedFor.current = bridge;
        // Best-effort discovery: a failure leaves the inputs blank without breaking the screen.
        bridge
          .hostDefaults()
          .then(defaults => {
            if (!current) return;
            // Pre-fill only fields the operator has not already typed into (still empty). Every
            // field remains editable, and a `null` default leaves the input empty.
            setInterfaceIp(prev => (prev === '' ? defaults.interfaceIp ?? '' : prev));
            setCertificate(prev => (prev === '' ? defaults.certificatePath ?? '' : prev));
            setKey(prev => (prev === '' ? defaults.keyPath ?? '' : prev));
          })
          .catch(() => {
            // Discovery is optional; blanks are a valid outcome.
          });
      });
    return () => {
      current = false;
    };
  }, [bridge, scan]);

  if (preview) return <SimulatedPreview onExit={() => setPreview(false)} />;

  const selectedInterface = interfaces.find(i => i.ipAddress === interfaceIp) ?? null;
  const pathsEntered = certificate.trim() !== '' && key.trim() !== '';
  const hostRunning = host !== null;
  const canStartHost =
    Boolean(bridge) && device !== null && selectedInterface !== null && pathsEntered && !hostRunning && !hostBusy;

  async function startHost() {
    if (!bridge || !device || !selectedInterface || !pathsEntered) return;
    setHostBusy(true);
    setHostError(null);
    try {
      const result = await bridge.startHost({
        capture: { deviceId: device.deviceId, sampleRateHz: 48000, bufferFrames: 128 },
        interface: selectedInterface,
        certificatePath: certificate.trim(),
        keyPath: key.trim(),
      });
      setHost({
        joinUrl: result.joinUrl,
        hostEpoch: result.hostEpoch,
        audioEpoch: result.audioEpoch,
      });
    } catch (e) {
      setHost(null);
      setHostError(e instanceof Error ? e.message : 'Could not start the host.');
    } finally {
      setHostBusy(false);
    }
  }

  async function stopHost() {
    if (!bridge) return;
    setHostBusy(true);
    setHostError(null);
    try {
      await bridge.stopHost();
      setHost(null);
    } catch (e) {
      setHostError(e instanceof Error ? e.message : 'Could not stop the host.');
    } finally {
      setHostBusy(false);
    }
  }

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
                ['Capture', device ? `${device.name} · setup only` : 'Not confirmed'],
                ['HTTPS join service', hostRunning ? 'Running' : 'Not running'],
                ['Phone audio', hostRunning ? 'Ready for a listener' : 'Unavailable'],
              ].map(([label, value]) => (
                <div key={label}>
                  <p className="text-caption text-text-secondary">{label}</p>
                  <p className="text-label">{value}</p>
                </div>
              ))}
            </div>
            {hostRunning ? (
              <p className="text-caption text-text-secondary">Host is running. The join service is listening; no phone audio is connected until a listener pairs.</p>
            ) : (
              <p className="text-caption text-text-secondary">IPC availability is not a running capture, server, or audio connection. No operation is reported running.</p>
            )}
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
              <dd>{pathsEntered ? 'Entered · Not validated' : 'Not entered'}</dd>
            </dl>
            {!bridge && (
              <p className="text-caption text-text-secondary">
                Unavailable — start the host from the desktop app. A browser has no native host.
              </p>
            )}
            <div className="iem-row">
              <Button
                variant="primary"
                disabled={!canStartHost}
                pending={hostBusy && !hostRunning}
                explanation={
                  !bridge
                    ? 'Native host access is required.'
                    : hostRunning
                      ? 'The host is already running.'
                      : !device
                        ? 'Select a capture device first.'
                        : !selectedInterface
                          ? 'Select a network interface first.'
                          : !pathsEntered
                            ? 'Enter both a certificate path and a key path.'
                            : undefined
                }
                onClick={startHost}
              >
                Start host
              </Button>
              <Button
                variant="destructive"
                disabled={!hostRunning || hostBusy}
                pending={hostBusy && hostRunning}
                explanation={!hostRunning ? 'No host is reported running.' : undefined}
                onClick={stopHost}
              >
                Stop host
              </Button>
            </div>
            {hostError && <p role="alert" className="text-danger">{hostError}</p>}
            {hostRunning && (
              <div role="status" className="iem-stack">
                <p className="text-success text-label">Host running</p>
                <label className="iem-field">
                  Join URL
                  <input
                    readOnly
                    value={host.joinUrl}
                    onFocus={e => e.currentTarget.select()}
                    spellCheck={false}
                  />
                </label>
                <p className="iem-hint">
                  This URL carries a single-use pairing token. It is shown here for the running host
                  and is not saved.
                </p>
                <p className="text-caption text-text-secondary">
                  host epoch {host.hostEpoch} · audio epoch {host.audioEpoch}
                </p>
              </div>
            )}
          </section>
          <section id="setup-4" className="iem-panel">
            <SourcesView bridge={bridge} device={device} />
          </section>
          <section id="setup-5" className="iem-panel">
            <PairingView bridge={bridge} hostRunning={hostRunning} />
          </section>
          <p className="text-caption text-text-secondary">POC work in progress · Not stage qualified. Network statistics cannot establish hearing safety or full-path audio latency.</p>
        </main>
      </div>
    </div>
  );
}
