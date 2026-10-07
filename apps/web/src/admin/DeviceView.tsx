import { useEffect, useRef, useState } from 'react';
import type { DeviceInfo } from '../protocol';
import type { HostBridge } from '../desktop-bridge/contracts';
import { Button } from '../ui/Button';

export interface DeviceViewProps {
  bridge?: HostBridge;
  onSelect?(device: DeviceInfo | null): void;
}

export function DeviceView({ bridge, onSelect }: DeviceViewProps) {
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [chosen, setChosen] = useState('');
  const [selected, setSelected] = useState<DeviceInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [scan, setScan] = useState(0);
  const callback = useRef(onSelect);
  callback.current = onSelect;

  useEffect(() => {
    if (!bridge) return;
    let current = true;
    setBusy(true);
    setError(null);
    setSelected(null);
    callback.current?.(null);
    bridge
      .listDevices()
      .then(list => {
        if (!current) return;
        setDevices(list);
        setChosen((list.find(d => d.isDefault) ?? list[0])?.deviceId ?? '');
      })
      .catch(e => {
        if (current) {
          setDevices([]);
          setError(e instanceof Error ? e.message : 'Could not list capture devices.');
        }
      })
      .finally(() => {
        if (current) setBusy(false);
      });
    return () => {
      current = false;
    };
  }, [bridge, scan]);

  const candidate = devices.find(d => d.deviceId === chosen);
  const valid =
    candidate &&
    candidate.sampleRateHz === 48000 &&
    candidate.inputChannels > 0 &&
    candidate.inputChannels <= 24;

  return (
    <div className="iem-stack">
      <div className="iem-row">
        <div>
          <h2 className="text-section">Capture device</h2>
          <p className="text-text-secondary">Select a reported USB input. Selection does not start capture.</p>
        </div>
        <Button disabled={!bridge || busy} onClick={() => setScan(n => n + 1)}>
          {busy ? 'Scanning devices' : 'Rescan devices'}
        </Button>
      </div>
      {!bridge && (
        <p className="iem-hint">Device enumeration is unavailable in a browser. Use the desktop app, or explore the simulated preview.</p>
      )}
      {error && <p role="alert" className="text-danger">{error}</p>}
      {bridge && !busy && !error && devices.length === 0 && <p>No input devices found</p>}
      {devices.length > 0 && (
        <fieldset className="iem-stack">
          <legend className="text-label">Reported input devices</legend>
          {devices.map(d => (
            <label key={d.deviceId} className="iem-device-option">
              <input
                type="radio"
                name="capture-device"
                checked={chosen === d.deviceId}
                onChange={() => setChosen(d.deviceId)}
              />
              <span>
                <strong>{d.name}</strong>
                <span className="block text-caption text-text-secondary">
                  {d.inputChannels} input channels · {d.sampleRateHz === 48000 ? '48 kHz reported' : '48 kHz not reported'}{d.sampleRateHz === null ? ' (unknown)' : ''}
                </span>
              </span>
            </label>
          ))}
        </fieldset>
      )}
      {candidate && (
        <p className="text-caption text-text-secondary">
          Buffer bounds: {candidate.bufferMinFrames === null || candidate.bufferMaxFrames === null ? 'Unavailable' : `${candidate.bufferMinFrames}–${candidate.bufferMaxFrames} frames`}. USB mapping still requires verification.
        </p>
      )}
      {candidate && candidate.inputChannels > 24 && (
        <p className="text-danger">This device reports more than 24 input channels and cannot be used for the POC.</p>
      )}
      <Button
        variant="primary"
        disabled={!valid || busy}
        explanation={
          !bridge
            ? 'Native device access is required.'
            : candidate && !valid
              ? 'Select a device reporting 48 kHz and no more than 24 input channels.'
              : undefined
        }
        onClick={() => {
          if (candidate && valid) {
            setSelected(candidate);
            onSelect?.(candidate);
          }
        }}
      >
        Select device
      </Button>
      {selected && (
        <p role="status" className="text-accent">
          Selected: {selected.name} — setup only, capture has not started.
        </p>
      )}
    </div>
  );
}
