import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { DigitalMeter } from '../DigitalMeter';

test('missing_telemetry_renders_unavailable_never_zero', () => {
  render(<DigitalMeter value={undefined} />);
  expect(screen.getByText(/unavailable/i)).toBeInTheDocument();
  expect(screen.queryByText('0')).not.toBeInTheDocument();
});

test('finite_peak_renders_dbfs_readout', () => {
  render(<DigitalMeter value={-6} label="Lead vocal input" />);
  expect(screen.getByText(/-6\.0 dbfs/i)).toBeInTheDocument();
});

test('clipping_is_stated_as_text_not_color_alone', () => {
  render(<DigitalMeter value={0} label="Personal output L" />);
  expect(screen.getByText(/clipping/i)).toBeInTheDocument();
});

test('simulated_badge_only_renders_when_requested', () => {
  const { rerender } = render(<DigitalMeter value={-3} />);
  expect(screen.queryByText(/simulated/i)).not.toBeInTheDocument();
  rerender(<DigitalMeter value={-3} simulated />);
  expect(screen.getByText(/simulated/i)).toBeInTheDocument();
});
