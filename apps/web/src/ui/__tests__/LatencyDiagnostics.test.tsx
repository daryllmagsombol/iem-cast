import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { LatencyDiagnostics } from '../LatencyDiagnostics';
import { diagSnapshot } from './helpers';

test('latency_diagnostics_show_not_measured_and_stacked_rows', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 42, buffer: 2 })} />);
  expect(screen.getByText(/network rtt/i)).toBeInTheDocument();
  expect(screen.getByText(/browser buffer delay/i)).toBeInTheDocument();
  expect(screen.getByText(/^not measured$/i)).toBeInTheDocument();
});

test('unavailable_is_never_rendered_as_zero', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 'Unavailable', buffer: 'Unavailable' })} />);
  expect(screen.getAllByText(/^unavailable$/i).length).toBe(2);
  expect(screen.queryByText(/^0 ms$/)).not.toBeInTheDocument();
});

test('first_sample_shows_waiting_for_sample', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 'Unavailable', buffer: 'Waiting for sample' })} />);
  expect(screen.getByText(/waiting for sample/i)).toBeInTheDocument();
  expect(screen.getByText(/^unavailable$/i)).toBeInTheDocument();
});

test('rtt_and_buffer_are_reported_separately_never_summed', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 40, buffer: 2, lastUpdated: null })} />);
  expect(screen.getByText(/40 ms/)).toBeInTheDocument();
  expect(screen.getByText(/2 ms/)).toBeInTheDocument();
  expect(screen.queryByText(/42/)).not.toBeInTheDocument();
});
