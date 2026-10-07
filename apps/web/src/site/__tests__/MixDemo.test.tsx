import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { MixDemo } from '../MixDemo';

test('demo_shows_persistent_simulated_notice_and_never_says_listening', () => {
  render(<MixDemo />);
  expect(screen.getByText(/simulated · silent demo · no host connection/i)).toBeInTheDocument();
  expect(screen.queryByText(/start listening/i)).not.toBeInTheDocument();
});

test('primary_action_is_start_demo_not_start_listening', () => {
  render(<MixDemo />);
  expect(screen.getByRole('button', { name: /^start demo$/i })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /start listening/i })).not.toBeInTheDocument();
});

test('every_meter_carries_a_simulated_badge', () => {
  render(<MixDemo />);
  const badges = screen.getAllByText(/^simulated$/i);
  // One badge per GainControl readout plus per meter caption plus master.
  expect(badges.length).toBeGreaterThan(0);
});

test('end_to_end_latency_reads_not_measured', () => {
  render(<MixDemo />);
  expect(screen.getByText(/^not measured$/i)).toBeInTheDocument();
});

test('sample_state_selector_offers_all_states_including_unavailable', () => {
  render(<MixDemo />);
  const select = screen.getByLabelText(/sample state/i);
  expect(select).toBeInTheDocument();
  expect(screen.getByRole('option', { name: /unavailable/i })).toBeInTheDocument();
  expect(screen.getByRole('option', { name: /waiting for sample/i })).toBeInTheDocument();
});
