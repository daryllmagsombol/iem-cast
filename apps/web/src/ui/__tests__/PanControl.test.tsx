import '@testing-library/jest-dom/vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { PanControl } from '../PanControl';

function mount(value = 0, options: { disabled?: boolean; fullLabel?: string; label?: string } = {}) {
  const onChange = vi.fn();
  const onReset = vi.fn();
  const result = render(<PanControl label="Pan" value={value} onChange={onChange} onReset={onReset} {...options} />);
  return { ...result, onChange, onReset, slider: screen.getByRole('slider') };
}

test.each([
  [0, 'Center', 'Center'],
  [-0.25, '25% left', '25 percent left'],
  [0.5, '50% right', '50 percent right'],
  [-1, 'Full left', 'Full left'],
  [1, 'Full right', 'Full right'],
] as const)('normalized_%s_has_honest_readout_and_accessible_value', (value, readout, phrase) => {
  const { slider, onChange, onReset } = mount(value);
  expect(slider).toHaveAttribute('min', '-100');
  expect(slider).toHaveAttribute('max', '100');
  expect(slider).toHaveAttribute('step', '1');
  expect(slider).toHaveValue(String(value * 100));
  expect(slider).toHaveAttribute('aria-valuetext', phrase);
  expect(screen.getByTestId('pan-readout')).toHaveTextContent(readout);
  expect(onChange).not.toHaveBeenCalled();
  expect(onReset).not.toHaveBeenCalled();
});

test.each(['Pan', 'Balance', 'Default pan', 'Default balance'])('label_%s_is_supported_without_other_semantics', label => {
  const { slider } = mount(0, { label });
  expect(slider).toHaveAccessibleName(label);
  expect(screen.queryByText(/gain|mute|saved|listening/i)).not.toBeInTheDocument();
});

test('fullLabel_names_the_source_or_pair_without_truncating_the_accessible_name', () => {
  const { slider } = mount(0.25, { label: 'Balance', fullLabel: 'Keys stereo pair balance' });
  expect(slider).toHaveAccessibleName('Keys stereo pair balance');
  expect(screen.getByRole('button', { name: 'Move Keys stereo pair balance left' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Move Keys stereo pair balance right' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Center Keys stereo pair balance' })).toBeEnabled();
  expect(slider).toHaveAccessibleDescription(/left.*center.*right/i);
});

test('range_changes_convert_display_percent_to_normalized_callback_only_at_the_boundary', () => {
  const { slider, onChange } = mount(0.5);
  fireEvent.change(slider, { target: { value: '-50' } });
  expect(onChange).toHaveBeenLastCalledWith(-0.5);
  // A request is not an applied/optimistic readout: only the parent prop updates it.
  expect(screen.getByTestId('pan-readout')).toHaveTextContent('50% right');
});

test('L_and_R_buttons_move_one_percent_without_dragging', () => {
  const { onChange } = mount(0.5);
  fireEvent.click(screen.getByRole('button', { name: 'Move Pan left' }));
  expect(onChange).toHaveBeenLastCalledWith(0.49);
  fireEvent.click(screen.getByRole('button', { name: 'Move Pan right' }));
  expect(onChange).toHaveBeenLastCalledWith(0.51);
});

test.each([
  ['ArrowLeft', 0.49], ['ArrowDown', 0.49],
  ['ArrowRight', 0.51], ['ArrowUp', 0.51],
  ['PageDown', 0.4], ['PageUp', 0.6],
  ['Home', -1], ['End', 1],
] as const)('keyboard_%s_emits_normalized_%s', (key, expected) => {
  const { slider, onChange } = mount(0.5);
  fireEvent.keyDown(slider, { key });
  expect(onChange).toHaveBeenCalledExactlyOnceWith(expected);
});

test('endpoint_buttons_are_disabled_and_keyboard_steps_never_exceed_bounds', () => {
  const { slider, onChange, rerender, onReset } = mount(-1);
  expect(screen.getByRole('button', { name: 'Move Pan left' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Move Pan right' })).toBeEnabled();
  fireEvent.keyDown(slider, { key: 'PageDown' });
  expect(onChange).not.toHaveBeenCalled();
  rerender(<PanControl label="Pan" value={0.99} onChange={onChange} onReset={onReset} />);
  fireEvent.keyDown(slider, { key: 'PageUp' });
  expect(onChange).toHaveBeenLastCalledWith(1);
  rerender(<PanControl label="Pan" value={1} onChange={onChange} onReset={onReset} />);
  expect(screen.getByRole('button', { name: 'Move Pan right' })).toBeDisabled();
});

test('Center_calls_only_onReset_and_waits_for_controlled_prop_update', () => {
  const { onChange, onReset, rerender } = mount(0.5);
  fireEvent.click(screen.getByRole('button', { name: 'Center Pan' }));
  expect(onReset).toHaveBeenCalledExactlyOnceWith();
  expect(onChange).not.toHaveBeenCalled();
  expect(screen.getByTestId('pan-readout')).toHaveTextContent('50% right');
  rerender(<PanControl label="Pan" value={0} onChange={onChange} onReset={onReset} />);
  expect(screen.getByTestId('pan-readout')).toHaveTextContent('Center');
  expect(screen.getByRole('button', { name: 'Center Pan' })).toBeDisabled();
});

test('disabled_control_cannot_emit_range_button_keyboard_or_reset_requests', () => {
  const { slider, onChange, onReset } = mount(0.5, { disabled: true });
  expect(slider).toBeDisabled();
  for (const button of screen.getAllByRole('button')) {
    expect(button).toBeDisabled();
    fireEvent.click(button);
  }
  fireEvent.change(slider, { target: { value: '-50' } });
  fireEvent.keyDown(slider, { key: 'Home' });
  expect(onChange).not.toHaveBeenCalled();
  expect(onReset).not.toHaveBeenCalled();
});

test.each([NaN, Infinity, -Infinity])('nonfinite_%s_is_unavailable_and_disabled_not_a_fabricated_center', value => {
  const { slider, onChange, onReset } = mount(value);
  expect(screen.getByTestId('pan-readout')).toHaveTextContent('Unavailable');
  expect(slider).toHaveAttribute('aria-valuetext', 'Unavailable');
  expect(slider).toBeDisabled();
  for (const button of screen.getAllByRole('button')) expect(button).toBeDisabled();
  expect(onChange).not.toHaveBeenCalled();
  expect(onReset).not.toHaveBeenCalled();
});

test('finite_out_of_range_values_are_bounded_without_automatic_submission', () => {
  const { slider, onChange, onReset } = mount(2);
  expect(slider).toHaveValue('100');
  expect(screen.getByTestId('pan-readout')).toHaveTextContent('Full right');
  expect(onChange).not.toHaveBeenCalled();
  expect(onReset).not.toHaveBeenCalled();
});

test('tab_order_is_left_range_right_center_and_keyboard_reset_has_no_change_side_effect', async () => {
  const user = userEvent.setup();
  const { slider, onChange, onReset } = mount(0.5);
  await user.tab();
  expect(screen.getByRole('button', { name: 'Move Pan left' })).toHaveFocus();
  await user.tab();
  expect(slider).toHaveFocus();
  await user.tab();
  expect(screen.getByRole('button', { name: 'Move Pan right' })).toHaveFocus();
  await user.tab();
  expect(screen.getByRole('button', { name: 'Center Pan' })).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(onReset).toHaveBeenCalledOnce();
  expect(onChange).not.toHaveBeenCalled();
});

test('modified_navigation_keys_do_not_override_platform_shortcuts', () => {
  const { slider, onChange } = mount(0.5);
  fireEvent.keyDown(slider, { key: 'ArrowLeft', ctrlKey: true });
  fireEvent.keyDown(slider, { key: 'ArrowRight', metaKey: true });
  fireEvent.keyDown(slider, { key: 'Home', altKey: true });
  expect(onChange).not.toHaveBeenCalled();
});

test('rounding_a_sub_percent_value_does_not_disable_an_explicit_reset_to_exact_zero', () => {
  const { onChange, onReset } = mount(0.004);
  expect(screen.getByRole('slider')).toHaveValue('0');
  const center = screen.getByRole('button', { name: 'Center Pan' });
  expect(center).toBeEnabled();
  fireEvent.click(center);
  expect(onReset).toHaveBeenCalledOnce();
  expect(onChange).not.toHaveBeenCalled();
});
