import { useEffect, useState } from 'react';
import type { DeviceInfo } from '../protocol';
import type { HostBridge } from '../desktop-bridge/contracts';
import { Button } from '../ui/Button';
import { Notice } from '../ui/Notice';

export interface DeviceViewProps {
  bridge: HostBridge;
}

export function DeviceView({ bridge }: DeviceViewProps) {
  const [devices, setDevices] = useState<DeviceInfo[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    bridge
      .listDevices()
      .then((list) => {
        if (!active) return;
        setDevices(list);
        const preferred = list.find((device) => device.isDefault) ?? list[0];
        setSelectedId(preferred?.deviceId ?? null);
      })
      .catch(() => {
        if (active) setError('Could not list capture devices.');
      });
    return () => {
      active = false;
    };
  }, [bridge]);

  const selected = devices?.find((device) => device.deviceId === selectedId) ?? null;

  return (
    <div className="flex flex-col gap-5">
      <header className="flex flex-col gap-1">
        <h1 className="text-title text-text">Capture device</h1>
        <p className="text-body text-text-secondary">
          Choose the Soundcraft USB input device. Rate must be actually supported; nothing is resampled.
        </p>
      </header>

      {error ? (
        <Notice tone="error" title="Device scan failed" live="alert">
          <p>{error}</p>
        </Notice>
      ) : null}

      {devices === null && !error ? (
        <Notice tone="empty" title="Scanning for input devices">
          <p>No progress is shown while the host enumerates CoreAudio inputs.</p>
        </Notice>
      ) : null}

      {devices !== null && devices.length === 0 ? (
        <Notice tone="empty" title="No input devices found">
          <p>Connect the Soundcraft USB interface and rescan.</p>
        </Notice>
      ) : null}

      {devices && devices.length > 0 ? (
        <fieldset className="flex flex-col gap-2 rounded-panel border border-boundary p-4">
          <legend className="px-1 text-label text-text">Available inputs</legend>
          <ul className="flex list-none flex-col gap-2 p-0">
            {devices.map((device) => (
              <li key={device.deviceId} className="flex min-h-target items-center gap-3">
                <input
                  type="radio"
                  id={`device-${device.deviceId}`}
                  name="capture-device"
                  checked={device.deviceId === selectedId}
                  onChange={() => setSelectedId(device.deviceId)}
                  className="h-5 w-5 accent-accent"
                />
                <label htmlFor={`device-${device.deviceId}`} className="flex flex-col">
                  <span className="text-label text-text">
                    {device.name}
                    {device.isDefault ? <span className="ml-2 text-caption text-text-secondary">Default</span> : null}
                  </span>
                  <span className="text-caption text-text-secondary">
                    {device.inputChannels} input channels ·{' '}
                    {device.sampleRateHz === 48000 ? '48 kHz supported' : '48 kHz not reported'}
                    {device.sampleRateHz === null ? ' (unknown)' : ''}
                  </span>
                </label>
              </li>
            ))}
          </ul>
        </fieldset>
      ) : null}

      {selected ? (
        <Notice tone={selected.inputChannels > 24 ? 'error' : 'empty'} title="Channel mapping check">
          {selected.inputChannels > 24 ? (
            <p>This device reports more than 24 input channels and cannot be used for the POC.</p>
          ) : (
            <p>
              {selected.inputChannels} channels will be mapped. USB master L/R is excluded from the source
              summation by default.
            </p>
          )}
        </Notice>
      ) : null}

      <div className="flex flex-wrap gap-2">
        <Button
          variant="primary"
          disabled={!selected || selected.inputChannels > 24 || selected.sampleRateHz !== 48000}
          explanation={
            selected && selected.sampleRateHz !== 48000
              ? 'This device does not report 48 kHz; unsupported rates are rejected, not resampled.'
              : selected && selected.inputChannels > 24
                ? 'Channel count above the 24-channel maximum is rejected, not trimmed.'
                : undefined
          }
        >
          Use this device
        </Button>
      </div>
    </div>
  );
}
