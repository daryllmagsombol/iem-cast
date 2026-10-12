import '@testing-library/jest-dom/vitest';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { LatencyDiagnostics } from '../LatencyDiagnostics';
import { diagSnapshot } from './helpers';

function reading(label: string) {
  return within(screen.getByText(label, { exact: true }).parentElement!).getByRole('definition', { hidden: true });
}

test('latency_diagnostics_show_not_measured_and_stacked_rows', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 42, buffer: 2 })} />);
  expect(screen.getByText(/network rtt/i)).toBeInTheDocument();
  expect(screen.getByText(/browser buffer delay/i)).toBeInTheDocument();
  expect(screen.getByText(/^not measured$/i)).toBeInTheDocument();
});

test('unavailable_is_never_rendered_as_zero', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 'Unavailable', buffer: 'Unavailable' })} />);
  expect(reading('Network RTT')).toHaveTextContent(/^Unavailable$/);
  expect(reading('Browser buffer delay')).toHaveTextContent(/^Unavailable$/);
  expect(screen.queryByText(/^0 ms$/)).not.toBeInTheDocument();
});

test('first_sample_shows_waiting_for_sample', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 'Unavailable', buffer: 'Waiting for sample' })} />);
  expect(screen.getByText(/waiting for sample/i)).toBeInTheDocument();
  expect(reading('Network RTT')).toHaveTextContent(/^Unavailable$/);
});

test('rtt_and_buffer_are_reported_separately_never_summed', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 40, buffer: 2, lastUpdated: null })} />);
  expect(screen.getByText(/40 ms/)).toBeInTheDocument();
  expect(screen.getByText(/2 ms/)).toBeInTheDocument();
  expect(screen.queryByText(/42/)).not.toBeInTheDocument();
});

test('additional_metrics_have_interval_context_and_distinct_units', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ target: 151.8, minimum: 38.2, lost: 3, concealment: 1.25 })} />);
  expect(reading('Browser target delay')).toHaveTextContent(/^152 ms$/);
  expect(reading('Browser minimum delay')).toHaveTextContent(/^38 ms$/);
  expect(reading('Packets lost')).toHaveTextContent(/^3 packets$/);
  expect(reading('Audio concealment')).toHaveTextContent(/^1.25%$/);
  expect(screen.getByText(/readings below cover the latest sample interval/i)).toBeInTheDocument();
  expect(screen.getByText(/target and minimum are browser-reported interval averages/i)).toBeInTheDocument();
});

test('all_interval_metrics_preserve_waiting_and_unavailable_without_units', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ target: 'Waiting for sample', minimum: 'Unavailable', lost: 'Waiting for sample', concealment: 'Unavailable' })} />);
  expect(reading('Browser target delay')).toHaveTextContent(/^Waiting for sample$/);
  expect(reading('Browser minimum delay')).toHaveTextContent(/^Unavailable$/);
  expect(reading('Packets lost')).toHaveTextContent(/^Waiting for sample$/);
  expect(reading('Audio concealment')).toHaveTextContent(/^Unavailable$/);
  expect(screen.queryByText(/^0 ms$/)).not.toBeInTheDocument();
  expect(screen.queryByText(/^0 packets$/)).not.toBeInTheDocument();
  expect(screen.queryByText(/^0%$/)).not.toBeInTheDocument();
});

test('genuine_zero_metrics_and_zero_requested_preference_are_not_unknown', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 0, buffer: 152, target: 0, minimum: 0, lost: 0, concealment: 0, preference: { state: 'Accepted', requested_ms: 0 } })} />);
  expect(reading('Network RTT')).toHaveTextContent(/^0 ms$/);
  expect(reading('Browser buffer delay')).toHaveTextContent(/^152 ms$/);
  expect(reading('Browser target delay')).toHaveTextContent(/^0 ms$/);
  expect(reading('Browser minimum delay')).toHaveTextContent(/^0 ms$/);
  expect(reading('Packets lost')).toHaveTextContent(/^0 packets$/);
  expect(reading('Audio concealment')).toHaveTextContent(/^0%$/);
  expect(reading('Requested preference')).toHaveTextContent(/^0 ms$/);
  expect(reading('Request outcome')).toHaveTextContent(/^Accepted$/);
  expect(screen.getByText(/accepted means the request was accepted, not that the actual buffer matches it/i)).toBeInTheDocument();
  expect(screen.getByText(/browser may ignore the preference or choose a higher delay/i)).toBeInTheDocument();
  expect(reading('End-to-end audio latency')).toHaveTextContent(/^Not measured$/);
  expect(screen.queryByText(/actual buffer: 0|0 ms cap|latency reduced/i)).not.toBeInTheDocument();
});

test.each(['Not requested', 'Unsupported', 'Rejected', 'Unavailable'] as const)('preference_state_%s_is_reported_without_inventing_a_request', state => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ preference: { state, requested_ms: null } })} />);
  expect(reading('Requested preference')).toHaveTextContent(state === 'Not requested' ? /^Not requested$/ : /^Unavailable$/);
  expect(reading('Request outcome')).toHaveTextContent(new RegExp(`^${state}$`));
  expect(screen.queryByText(/^0 ms$/)).not.toBeInTheDocument();
});

test('small_nonzero_concealment_is_not_rounded_to_zero_and_single_packet_is_singular', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ lost: 1, concealment: 0.001 })} />);
  expect(reading('Packets lost')).toHaveTextContent(/^1 packet$/);
  expect(reading('Audio concealment')).toHaveTextContent(/^<0.01%$/);
});

test('native_details_are_collapsed_by_default_and_keep_open_across_sample_updates_without_live_announcements', async () => {
  const user = userEvent.setup();
  const { container, rerender } = render(<LatencyDiagnostics snapshot={diagSnapshot()} simulated />);
  const summary = screen.getByText('Browser buffer details', { exact: true });
  const details = summary.closest('details')!;
  expect(details).not.toHaveAttribute('open');
  expect(screen.getByText('SIMULATED')).toBeInTheDocument();
  await user.tab();
  expect(summary).toHaveFocus();
  await user.click(summary);
  expect(details).toHaveAttribute('open');
  rerender(<LatencyDiagnostics snapshot={diagSnapshot({ target: 20 })} simulated />);
  expect(details).toHaveAttribute('open');
  expect(reading('Browser target delay')).toHaveTextContent(/^20 ms$/);
  expect(container.querySelector('[aria-live], [role="status"], [role="alert"]')).toBeNull();
});
