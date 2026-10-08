import { useEffect, useState } from 'react';
import type { DeviceInfo, SourceInfo } from '../protocol';
import type { HostBridge } from '../desktop-bridge/contracts';
import { Button } from '../ui/Button';

export interface SourcesViewProps {
  bridge?: HostBridge;
  device?: DeviceInfo | null;
}

// The catalog is read-only in this build: it mirrors the SELECTED device's real channels, derived by
// the host exactly as it does for phones. Nothing here is fabricated, and no mutation can persist.
export function SourcesView({ bridge, device }: SourcesViewProps) {
  const [sources, setSources] = useState<SourceInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const deviceId = device?.deviceId ?? null;

  useEffect(() => {
    if (!bridge || deviceId === null) {
      setSources(null);
      setError(null);
      return;
    }
    let current = true;
    setSources(null);
    setError(null);
    bridge
      .sourceCatalog(deviceId)
      .then(list => {
        if (current) setSources(list);
      })
      .catch(e => {
        if (current) {
          setSources(null);
          setError(e instanceof Error ? e.message : 'Could not read the source catalog.');
        }
      });
    return () => {
      current = false;
    };
  }, [bridge, deviceId]);

  return (
    <div className="iem-stack">
      <h2 className="text-section">Sources</h2>
      {!device && <p className="iem-hint">Select a capture device to see its source channels.</p>}
      {device && !bridge && (
        <p className="iem-hint">Source details are unavailable in a browser. Use the desktop app.</p>
      )}
      {device && bridge && error && (
        <p role="alert" className="text-danger">{error}</p>
      )}
      {device && bridge && !error && sources !== null && sources.length === 0 && (
        <p className="iem-hint">This device reports no source channels.</p>
      )}
      {device && bridge && !error && sources !== null && sources.length > 0 && (
        <>
          <p className="text-text-secondary">
            These are the {sources.length} channels reported by {device.name}. They are read-only:
            this build cannot persist source renames or permissions.
          </p>
          <ul className="iem-stack" aria-label="Reported source channels">
            {sources.map(source => (
              <li key={source.sourceId} className="iem-row">
                <span className="text-label">{source.label}</span>
                <span className="text-caption text-text-secondary">
                  Channel {source.physicalIndex + 1} · {source.role}
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
      <div className="iem-row">
        <Button disabled explanation="Source names cannot be saved in this build.">
          Save label
        </Button>
        <Button disabled explanation="Source permissions are not stored in this build.">
          Publish sources
        </Button>
      </div>
    </div>
  );
}
