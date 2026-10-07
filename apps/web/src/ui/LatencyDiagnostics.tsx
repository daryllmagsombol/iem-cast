import type { DiagnosticsSnapshot } from '../receiver-ports';

export interface LatencyDiagnosticsProps {
  snapshot: DiagnosticsSnapshot;
  /** Adds a visible SIMULATED badge. Only the Pages sample-data demo sets this. */
  simulated?: boolean;
}

type NetworkValue = number | 'Unavailable' | 'Waiting for sample';

function formatMs(value: NetworkValue): string {
  if (typeof value === 'number') return `${Math.round(value)} ms`;
  return value;
}

function formatLastUpdated(lastUpdated: number | null): string {
  if (lastUpdated === null) return 'No update yet';
  const date = new Date(lastUpdated);
  return `Last updated ${date.toLocaleTimeString()}`;
}

/**
 * Compact stacked label/value rows. Refresh is driven by the snapshot's 1 Hz owner;
 * this component only renders. Values never fabricate zero.
 */
export function LatencyDiagnostics({ snapshot, simulated = false }: LatencyDiagnosticsProps) {
  const { network_rtt_ms, buffer_delay_ms, last_updated } = snapshot;

  const rows: Array<{ label: string; value: string }> = [
    { label: 'Network RTT', value: formatMs(network_rtt_ms) },
    { label: 'Browser buffer delay', value: formatMs(buffer_delay_ms) },
    { label: 'End-to-end audio latency', value: 'Not measured' },
  ];

  return (
    <section aria-labelledby="latency-diagnostics-heading" className="rounded-panel border border-boundary bg-surface p-4">
      <h2 id="latency-diagnostics-heading" className="text-label text-text">
        Latency diagnostics
        {simulated ? <span className="ml-2 text-caption text-warning">SIMULATED</span> : null}
      </h2>
      <p className="mt-1 text-caption text-text-secondary">
        Network and buffer readings are not full-path audio latency. End-to-end requires a physical measurement.
      </p>
      <dl className="mt-3 flex flex-col gap-2">
        {rows.map((row) => (
          <div key={row.label} className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
            <dt className="text-body text-text-secondary">{row.label}</dt>
            <dd className="font-mono text-readout tabular-nums text-text">{row.value}</dd>
          </div>
        ))}
      </dl>
      <p className="mt-2 text-caption text-text-secondary">{formatLastUpdated(last_updated)}</p>
    </section>
  );
}
