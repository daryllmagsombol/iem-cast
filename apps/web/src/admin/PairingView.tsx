import type { HostBridge } from '../desktop-bridge/contracts';
import { Button } from '../ui/Button';
export interface PairingViewProps {
  bridge?: HostBridge;
}

export function PairingView(_props: PairingViewProps) {
  return (
    <div className="iem-stack">
      <h2 className="text-section">Pairing & local monitor</h2>
      <div className="iem-split">
        <div>
          <h3 className="text-label">Secure phone pairing</h3>
          <p className="text-text-secondary">Unavailable — no HTTPS join service is running.</p>
          <Button disabled explanation="Pairing requires the host’s secure join service.">
            Generate pairing code
          </Button>
        </div>
        <div>
          <h3 className="text-label">Local monitor</h3>
          <p className="text-text-secondary">Unavailable — monitoring controls are not exposed to the desktop app.</p>
          <p className="text-caption text-text-secondary">No output device or audio route is selected by this screen.</p>
        </div>
      </div>
    </div>
  );
}
