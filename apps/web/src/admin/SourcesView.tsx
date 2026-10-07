import type { HostBridge } from '../desktop-bridge/contracts';
import { Button } from '../ui/Button';

// The current RPC returns a synthetic catalog. Never present it as verified USB sources.
export function SourcesView(_props: { bridge?: HostBridge }) {
  return (
    <div className="iem-stack">
      <h2 className="text-section">Sources</h2>
      <p className="iem-hint">No sources available</p>
      <p className="text-text-secondary">A verified source catalog is unavailable in this build. Device channel count is not a confirmed source map. USB master L/R must remain excluded from personal source summation by default.</p>
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
