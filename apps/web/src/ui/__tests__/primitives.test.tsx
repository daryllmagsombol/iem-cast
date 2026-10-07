import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { StatusBadge } from '../StatusBadge';
import { Notice } from '../Notice';
import { Toggle } from '../Toggle';
import { Button } from '../Button';

test('status_badge_renders_text_not_color_alone', () => {
  render(<StatusBadge tone="warning">Connected · Not listening</StatusBadge>);
  expect(screen.getByText(/connected · not listening/i)).toBeInTheDocument();
});

test('notice_error_uses_alert_role', () => {
  render(
    <Notice tone="error" title="Connection lost" live="alert">
      <p>Output silenced</p>
    </Notice>,
  );
  expect(screen.getByRole('alert')).toHaveTextContent(/connection lost/i);
});

test('toggle_exposes_native_switch_state_and_visible_wording', () => {
  render(<Toggle label="Personal master mute" checked onChange={() => {}} onLabel="Muted" offLabel="Not muted" />);
  const toggle = screen.getByRole('switch', { name: /personal master mute/i });
  expect(toggle).toBeChecked();
  expect(screen.getByText(/^muted$/i)).toBeInTheDocument();
});

test('button_pending_state_is_busy_and_shows_explanation', () => {
  render(
    <Button pending explanation="Waiting for the host to apply this change.">
      Apply
    </Button>,
  );
  const button = screen.getByRole('button', { name: /apply/i });
  expect(button).toBeDisabled();
  expect(button).toHaveAttribute('aria-busy', 'true');
  expect(screen.getByText(/waiting for the host/i)).toBeInTheDocument();
});
