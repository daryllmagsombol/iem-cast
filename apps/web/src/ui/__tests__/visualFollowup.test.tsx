import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { Button } from '../Button';
import { ChannelStrip } from '../ChannelStrip';
import { mockStrip } from './helpers';

afterEach(cleanup);

test('source action reserves intrinsic label width rather than shrinking below its text', () => {
  const onMuteChange = vi.fn();
  render(
    <ChannelStrip
      state={mockStrip({ muted: true, label: 'A deliberately long vocal source name' })}
      onGainChange={() => {}}
      onMuteChange={onMuteChange}
    />,
  );
  const action = screen.getByRole('button', { name: 'Unmute A deliberately long vocal source name' });
  expect(action).toHaveClass('shrink-0', 'whitespace-nowrap');
  fireEvent.click(action);
  expect(onMuteChange).toHaveBeenCalledWith(false);
  expect(screen.getByText('Source details')).toBeInTheDocument();
});

test('disabled buttons have a readable unavailable affordance, but pending buttons do not', () => {
  const { rerender } = render(<Button variant="primary" disabled>Start host</Button>);
  expect(screen.getByRole('button', { name: 'Start host' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Start host' })).toHaveClass('iem-button-unavailable');
  expect(screen.getByText('Unavailable')).toBeInTheDocument();
  rerender(<Button pending disabled>Start host</Button>);
  expect(screen.getByRole('button', { name: /Start host/ })).toHaveAttribute('aria-busy', 'true');
  expect(screen.queryByText('Unavailable')).not.toBeInTheDocument();
  expect(screen.getByText('Pending')).toBeInTheDocument();
});
