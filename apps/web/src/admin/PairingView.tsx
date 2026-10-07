import { useEffect, useRef, useState } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import type { HostBridge, OutputDeviceInfo, PairingCredential } from '../desktop-bridge/contracts';
import { Button } from '../ui/Button';

/**
 * Listener slots offered for the local monitor. Each slot is a per-listener mix tap in the host
 * DSP (`host-core` `audio::engine::MAX_SESSIONS = 4`); Rust rejects a slot outside this range with
 * `MONITOR_SLOT_INVALID`.
 */
export const MONITOR_SLOTS = [0, 1, 2, 3] as const;

const DEFAULT_OUTPUT = '__default__';

export interface PairingViewProps {
  bridge?: HostBridge;
  /** Whether a host session is actually reported running by the host controls. */
  hostRunning: boolean;
}

/** Best-effort read of an IPC error `code`, without importing the Tauri adapter into the view. */
function errorCode(error: unknown): string | null {
  if (typeof error === 'object' && error !== null && 'code' in error) {
    const code = (error as { code?: unknown }).code;
    if (typeof code === 'string') return code;
  }
  return null;
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

/**
 * Beginner-safe pairing and the operator's private local monitor.
 *
 * The QR encodes the live single-use join URL returned by the host; the raw token is never shown as
 * separate text or persisted. The local monitor taps one active listener slot onto a chosen output
 * device and never implies a listener exists when the app cannot observe one.
 */
export function PairingView({ bridge, hostRunning }: PairingViewProps) {
  const [credential, setCredential] = useState<PairingCredential | null>(null);
  const [pairingBusy, setPairingBusy] = useState(false);
  const [pairingError, setPairingError] = useState<string | null>(null);
  const [pairingNeedsHost, setPairingNeedsHost] = useState(false);

  const [outputs, setOutputs] = useState<OutputDeviceInfo[]>([]);
  const [outputsError, setOutputsError] = useState<string | null>(null);
  const [outputsBusy, setOutputsBusy] = useState(false);
  const [outputChoice, setOutputChoice] = useState<string>(DEFAULT_OUTPUT);
  const [slot, setSlot] = useState<number>(MONITOR_SLOTS[0]);
  const [listenerConfirmed, setListenerConfirmed] = useState(false);
  const [monitorRunning, setMonitorRunning] = useState(false);
  const [monitorBusy, setMonitorBusy] = useState(false);
  const [monitorError, setMonitorError] = useState<string | null>(null);
  const [outputScan, setOutputScan] = useState(0);

  // A credential is bound to one host generation; drop it when the host stops.
  const wasRunning = useRef(hostRunning);
  useEffect(() => {
    if (wasRunning.current && !hostRunning) {
      setCredential(null);
      setPairingError(null);
      setPairingNeedsHost(false);
      setMonitorRunning(false);
      setListenerConfirmed(false);
      setOutputs([]);
      setOutputChoice(DEFAULT_OUTPUT);
    }
    wasRunning.current = hostRunning;
  }, [hostRunning]);

  useEffect(() => {
    if (!bridge || !hostRunning) {
      setOutputs([]);
      return;
    }
    let current = true;
    setOutputsBusy(true);
    setOutputsError(null);
    bridge
      .listOutputDevices()
      .then(list => {
        if (!current) return;
        setOutputs(list);
      })
      .catch(e => {
        if (!current) return;
        setOutputs([]);
        setOutputsError(errorMessage(e, 'Could not list output devices.'));
      })
      .finally(() => {
        if (current) setOutputsBusy(false);
      });
    return () => {
      current = false;
    };
  }, [bridge, hostRunning, outputScan]);

  async function generateCode() {
    if (!bridge) return;
    setPairingBusy(true);
    setPairingError(null);
    setPairingNeedsHost(false);
    try {
      const next = await bridge.issuePairingCredential();
      if (!next.joinUrl) {
        // A credential with no join URL is not a usable code; never render an empty QR.
        setCredential(null);
        setPairingError('The host returned no join URL for this pairing code.');
        return;
      }
      setCredential(next);
    } catch (e) {
      setCredential(null);
      const code = errorCode(e);
      const needsHost = code === 'HOST_NOT_RUNNING' || code === 'PAIRING_UNAVAILABLE';
      setPairingNeedsHost(needsHost);
      setPairingError(errorMessage(e, 'Could not generate a pairing code.'));
    } finally {
      setPairingBusy(false);
    }
  }

  async function startMonitor() {
    if (!bridge) return;
    setMonitorBusy(true);
    setMonitorError(null);
    try {
      await bridge.startMonitor(slot, outputChoice === DEFAULT_OUTPUT ? null : outputChoice);
      setMonitorRunning(true);
    } catch (e) {
      setMonitorRunning(false);
      setMonitorError(errorMessage(e, 'Could not start the local monitor.'));
    } finally {
      setMonitorBusy(false);
    }
  }

  async function stopMonitor() {
    if (!bridge) return;
    setMonitorBusy(true);
    setMonitorError(null);
    try {
      await bridge.stopMonitor();
      setMonitorRunning(false);
    } catch (e) {
      setMonitorError(errorMessage(e, 'Could not stop the local monitor.'));
    } finally {
      setMonitorBusy(false);
    }
  }

  const canGenerate = Boolean(bridge) && hostRunning && !pairingBusy;
  const canStartMonitor =
    Boolean(bridge) && hostRunning && listenerConfirmed && !monitorRunning && !monitorBusy;

  return (
    <div className="iem-stack">
      <h2 className="text-section">Pairing & local monitor</h2>
      <div className="iem-split">
        <div className="iem-stack">
          <h3 className="text-label">Secure phone pairing</h3>
          {!bridge ? (
            <p className="text-text-secondary">
              Unavailable — the desktop app is required to reach the host.
            </p>
          ) : !hostRunning ? (
            <p className="text-text-secondary">
              Unavailable — start the host first, then generate a single-use code.
            </p>
          ) : (
            <p className="text-text-secondary">
              Generate a single-use code to pair one phone. It expires shortly and is never saved.
            </p>
          )}
          <Button
            disabled={!canGenerate}
            pending={pairingBusy}
            explanation={
              !bridge
                ? 'Native access is required.'
                : !hostRunning
                  ? 'Pairing requires the host’s secure join service.'
                  : undefined
            }
            onClick={generateCode}
          >
            {credential ? 'Generate a new code' : 'Generate pairing code'}
          </Button>
          {pairingError && (
            <div role="alert" className="iem-stack">
              <p className="text-danger">{pairingError}</p>
              {pairingNeedsHost && (
                <p className="text-text-secondary">
                  The host is not running. Start the host in the Host controls step, then generate a
                  new code.
                </p>
              )}
            </div>
          )}
          {credential && (
            <div className="iem-stack">
              <p role="status" className="text-accent">
                Pairing code ready — scan with the phone camera to join.
              </p>
              <div className="iem-qr" data-testid="pairing-qr">
                <QRCodeSVG
                  value={credential.joinUrl}
                  size={192}
                  level="M"
                  marginSize={2}
                  bgColor="#ffffff"
                  fgColor="#000000"
                  title="Single-use pairing QR code"
                />
              </div>
              <p className="iem-hint">
                Expires in {credential.expiresInSeconds} seconds. Single-use: it stops working after
                one phone joins or when it expires. Not saved by this app.
              </p>
            </div>
          )}
        </div>

        <div className="iem-stack">
          <h3 className="text-label">Local monitor</h3>
          <p className="text-caption text-text-secondary">
            Plays one active listener slot’s mix on this machine’s output. Use headphones when live
            microphones are open — monitor output through speakers can feed back into the mics.
          </p>
          {!bridge ? (
            <p className="text-text-secondary">
              Unavailable — the desktop app is required to reach the host.
            </p>
          ) : !hostRunning ? (
            <p className="text-text-secondary">
              Unavailable — start the host first to monitor a listener slot.
            </p>
          ) : (
            <div className="iem-stack">
              <div className="iem-row">
                <label className="iem-field">
                  Monitor output device
                  <select
                    value={outputChoice}
                    disabled={outputsBusy || monitorRunning}
                    onChange={e => setOutputChoice(e.target.value)}
                  >
                    <option value={DEFAULT_OUTPUT}>System default output</option>
                    {outputs.map(o => (
                      <option key={o.id} value={o.id}>
                        {o.name}
                        {o.isDefault ? ' · default' : ''}
                      </option>
                    ))}
                  </select>
                </label>
                <Button
                  disabled={outputsBusy}
                  pending={outputsBusy}
                  onClick={() => setOutputScan(n => n + 1)}
                >
                  Rescan outputs
                </Button>
              </div>
              {outputsError && (
                <p role="alert" className="text-danger">{outputsError}</p>
              )}
              {!outputsBusy && !outputsError && outputs.length === 0 && (
                <p className="text-caption text-text-secondary">
                  No dedicated output devices reported — the system default output is used.
                </p>
              )}
              <label className="iem-field">
                Listener slot
                <select
                  value={slot}
                  disabled={monitorRunning}
                  onChange={e => setSlot(Number(e.target.value))}
                >
                  {MONITOR_SLOTS.map(s => (
                    <option key={s} value={s}>
                      Slot {s + 1}
                    </option>
                  ))}
                </select>
              </label>
              <label className="iem-device-option">
                <input
                  type="checkbox"
                  checked={listenerConfirmed}
                  disabled={monitorRunning}
                  onChange={e => setListenerConfirmed(e.target.checked)}
                />
                <span>
                  <strong>A phone has joined this slot</strong>
                  <span className="block text-caption text-text-secondary">
                    This build cannot detect which slots have a listener, so the monitor is only
                    started once you confirm a phone is listening. Otherwise the tap carries no
                    audio.
                  </span>
                </span>
              </label>
              <div className="iem-row">
                <Button
                  variant="primary"
                  disabled={!canStartMonitor}
                  pending={monitorBusy && !monitorRunning}
                  explanation={
                    !listenerConfirmed
                      ? 'Confirm a phone has joined the selected slot first.'
                      : undefined
                  }
                  onClick={startMonitor}
                >
                  Start monitor
                </Button>
                <Button
                  variant="destructive"
                  disabled={!monitorRunning || monitorBusy}
                  explanation={!monitorRunning ? 'No local monitor is running.' : undefined}
                  onClick={stopMonitor}
                >
                  Stop monitor
                </Button>
              </div>
              {monitorError && (
                <p role="alert" className="text-danger">{monitorError}</p>
              )}
              {monitorRunning && (
                <p role="status" className="text-success">
                  Monitor running — slot {slot + 1} on{' '}
                  {outputChoice === DEFAULT_OUTPUT
                    ? 'the system default output'
                    : outputs.find(o => o.id === outputChoice)?.name ?? 'the selected output'}
                  .
                </p>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
