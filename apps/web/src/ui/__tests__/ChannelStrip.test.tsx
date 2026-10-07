import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { ChannelStrip } from '../ChannelStrip';
import { mockStrip } from './helpers';

test('channel_mute_shows_requested_pending_until_host_applied_revision', () => {
  render(
    <ChannelStrip
      state={mockStrip({ requestedDb: -12, appliedDb: -6 })}
      onGainChange={() => {}}
      onMuteChange={() => {}}
    />,
  );
  expect(screen.getByText(/requested -12 db .* pending/i)).toBeInTheDocument();
  expect(screen.getByText(/applied -6 db/i)).toBeInTheDocument();
});

test('applied_value_shown_when_no_pending_edit', () => {
  render(
    <ChannelStrip
      state={mockStrip({ requestedDb: -18, appliedDb: -18 })}
      onGainChange={() => {}}
      onMuteChange={() => {}}
    />,
  );
  expect(screen.getByText(/applied -18 db/i)).toBeInTheDocument();
  expect(screen.queryByText(/pending/i)).not.toBeInTheDocument();
});

test('channel_strip_meter_is_not_simulated_by_default', () => {
  render(
    <ChannelStrip state={mockStrip()} onGainChange={() => {}} onMuteChange={() => {}} />,
  );
  expect(screen.queryByText(/simulated/i)).not.toBeInTheDocument();
});
