import { useId } from 'react';
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

function formatPackets(value: NetworkValue): string {
  return typeof value === 'number' ? `${value} ${value === 1 ? 'packet' : 'packets'}` : value;
}

function formatPercent(value: NetworkValue): string {
  if (typeof value !== 'number') return value;
  if (value > 0 && value < 0.01) return '<0.01%';
  return `${Number(value.toFixed(2))}%`;
}

function Reading({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
      <dt className="min-w-0 text-body text-text-secondary">{label}</dt>
      <dd className="min-w-0 font-mono text-readout tabular-nums text-text [overflow-wrap:anywhere]">{value}</dd>
    </div>
  );
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
  const headingId = useId();
  const { network_rtt_ms, buffer_delay_ms, last_updated, jitter_buffer_preference } = snapshot;

  const rows: Array<{ label: string; value: string }> = [
    { label: 'Network RTT', value: formatMs(network_rtt_ms) },
    { label: 'Browser buffer delay', value: formatMs(buffer_delay_ms) },
    { label: 'End-to-end audio latency', value: 'Not measured' },
  ];
  const intervalRows = [
    { label: 'Browser target delay', value: formatMs(snapshot.buffer_target_delay_ms) },
    { label: 'Browser minimum delay', value: formatMs(snapshot.buffer_minimum_delay_ms) },
    { label: 'Packets lost', value: formatPackets(snapshot.packets_lost_delta) },
    { label: 'Audio concealment', value: formatPercent(snapshot.concealment_percent) },
  ];

  return (
    <section aria-labelledby={headingId} className="min-w-0 rounded-panel border border-boundary bg-surface p-4">
      <h2 id={headingId} className="text-label text-text">
        Latency diagnostics
        {simulated ? <span className="ml-2 text-caption text-warning">SIMULATED</span> : null}
      </h2>
      <p className="mt-1 text-caption text-text-secondary">
        Network and buffer readings are not full-path audio latency. End-to-end requires a physical measurement.
      </p>
      <dl className="mt-4 flex flex-col gap-3">
        {rows.map((row) => (
          <Reading key={row.label} {...row} />
        ))}
      </dl>
      <details className="mt-4 border-t border-boundary">
        <summary className="min-h-target rounded-control py-3 text-label text-accent [overflow-wrap:anywhere] [scroll-margin-block:var(--iem-space-5)] hover:bg-surface-raised">
          Browser buffer details
        </summary>
        <p className="text-caption text-text-secondary">
          Readings below cover the latest sample interval. Target and minimum are browser-reported interval averages, not measurements of the sound reaching your ears or its minimum possible delay.
        </p>
        <dl className="mt-3 flex flex-col gap-3">
          {intervalRows.map(row => <Reading key={row.label} {...row} />)}
        </dl>
        <p className="mt-3 text-caption text-text-secondary">
          Audio concealment is the share of audio samples the browser replaced to cover missing audio.
        </p>
        <div className="mt-4 rounded-control border border-boundary bg-surface-raised p-3">
          <h3 className="text-label text-text">Jitter buffer preference</h3>
          <dl className="mt-3 flex flex-col gap-3">
            <Reading label="Requested preference" value={jitter_buffer_preference.requested_ms === null ? (jitter_buffer_preference.state === 'Not requested' ? 'Not requested' : 'Unavailable') : formatMs(jitter_buffer_preference.requested_ms)} />
            <Reading label="Request outcome" value={jitter_buffer_preference.state} />
          </dl>
          <p className="mt-3 text-caption text-text-secondary">
            Accepted means the request was accepted, not that the actual buffer matches it. The browser may ignore the preference or choose a higher delay.
          </p>
        </div>
      </details>
      <p className="mt-3 text-caption text-text-secondary">{formatLastUpdated(last_updated)}</p>
    </section>
  );
}
