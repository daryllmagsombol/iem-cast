import '@testing-library/jest-dom/vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { ChannelStrip } from '../ChannelStrip';
import { mockStrip } from './helpers';

test('mono_strip_renders_pan_after_gain_and_reports_normalized_value', () => {
  const onChange = vi.fn();
  const onReset = vi.fn();
  render(
    <ChannelStrip
      state={mockStrip()}
      onGainChange={() => {}}
      onMuteChange={() => {}}
      pan={{ label: 'Pan', value: 0.25, fullLabel: 'Lead vocal pan', onChange, onReset }}
    />,
  );
  // Gain is still the primary control; pan is additive and independently named.
  expect(screen.getByRole('slider', { name: 'Lead vocal' })).toBeInTheDocument();
  const pan = screen.getByRole('slider', { name: 'Lead vocal pan' });
  expect(pan).toHaveValue('25');
  expect(screen.getByTestId('pan-readout')).toHaveTextContent('25% right');

  fireEvent.change(pan, { target: { value: '-50' } });
  expect(onChange).toHaveBeenLastCalledWith(-0.5);

  fireEvent.click(screen.getByRole('button', { name: 'Center Lead vocal pan' }));
  expect(onReset).toHaveBeenCalledOnce();
});

test('strip_without_pan_config_renders_no_pan_control', () => {
  render(<ChannelStrip state={mockStrip()} onGainChange={() => {}} onMuteChange={() => {}} />);
  expect(screen.queryByTestId('pan-readout')).not.toBeInTheDocument();
  expect(screen.queryByRole('slider', { name: /pan/i })).not.toBeInTheDocument();
});

test('disabled_strip_disables_an_otherwise_enabled_pan_and_never_unmutes', () => {
  const onMuteChange = vi.fn();
  render(
    <ChannelStrip
      state={mockStrip({ muted: true })}
      disabled
      onGainChange={() => {}}
      onMuteChange={onMuteChange}
      pan={{ label: 'Pan', value: 0, fullLabel: 'Lead vocal pan', onChange: () => {}, onReset: () => {} }}
    />,
  );
  expect(screen.getByRole('slider', { name: 'Lead vocal pan' })).toBeDisabled();
  expect(screen.getByRole('button', { name: /Unmute Lead vocal/i })).toBeDisabled();
  expect(onMuteChange).not.toHaveBeenCalled();
});

test('mute_state_does_not_change_the_reported_pan_position', () => {
  const { rerender } = render(
    <ChannelStrip
      state={mockStrip({ muted: true })}
      onGainChange={() => {}}
      onMuteChange={() => {}}
      pan={{ label: 'Pan', value: -0.5, fullLabel: 'Lead vocal pan', onChange: () => {}, onReset: () => {} }}
    />,
  );
  expect(screen.getByTestId('pan-readout')).toHaveTextContent('50% left');
  rerender(
    <ChannelStrip
      state={mockStrip({ muted: false })}
      onGainChange={() => {}}
      onMuteChange={() => {}}
      pan={{ label: 'Pan', value: -0.5, fullLabel: 'Lead vocal pan', onChange: () => {}, onReset: () => {} }}
    />,
  );
  expect(screen.getByTestId('pan-readout')).toHaveTextContent('50% left');
});
