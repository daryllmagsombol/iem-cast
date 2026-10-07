import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { GainControl } from '../GainControl';

test('gain_control_uses_named_native_range_with_aria_valuetext', () => {
  render(<GainControl label="Lead vocal" valueDb={-12} onChange={() => {}} />);
  const range = screen.getByRole('slider', { name: /lead vocal/i });
  expect(range).toHaveAttribute('aria-valuetext', expect.stringMatching(/minus 12 decibels/i));
});

test('mute_is_labeled_logical_neg_infinity_separately_from_lowest_finite_gain', () => {
  render(<GainControl label="Keys" valueDb={-60} muted onChange={() => {}} />);
  expect(screen.getByText(/muted \(\u2212\u221e db\)/i)).toBeInTheDocument();
  // The mute readout is a single explicit label, not a numeric infinity value.
  expect(screen.queryByText(/-60 db/i)).not.toBeInTheDocument();
});

test('gain_control_exposes_decrease_and_increase_buttons', () => {
  render(<GainControl label="Drums" valueDb={-20} onChange={() => {}} />);
  expect(screen.getByRole('button', { name: /decrease drums gain/i })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /increase drums gain/i })).toBeInTheDocument();
});

test('increase_button_emits_bounded_next_step', () => {
  const onChange = vi.fn();
  render(<GainControl label="Bass" valueDb={-12} onChange={onChange} />);
  fireEvent.click(screen.getByRole('button', { name: /increase bass gain/i }));
  expect(onChange).toHaveBeenCalledWith(-11);
});

test('numeric_entry_validates_whole_db_within_bounds', () => {
  const onChange = vi.fn();
  render(<GainControl label="Guitar" valueDb={-12} onChange={onChange} />);
  const entry = screen.getByRole('spinbutton', { name: /guitar level in decibels/i });
  fireEvent.change(entry, { target: { value: '-25' } });
  expect(onChange).toHaveBeenCalledWith(-25);
});
