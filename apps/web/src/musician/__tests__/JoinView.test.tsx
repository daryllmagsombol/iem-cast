import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { JoinView } from '../JoinView';

test('join_view_starts_muted_until_an_explicit_gesture', () => {
  render(
    <JoinView
      phase="ready-muted"
      hasSources
      connecting={false}
      error={null}
      onStartListening={() => {}}
      onCancel={() => {}}
    />,
  );
  expect(screen.getByText(/connected · not listening/i)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /start listening/i })).toBeInTheDocument();
});

test('start_listening_is_unavailable_without_sources', () => {
  render(
    <JoinView
      phase="ready-muted"
      hasSources={false}
      connecting={false}
      error={null}
      onStartListening={() => {}}
      onCancel={() => {}}
    />,
  );
  expect(screen.getByText(/no sources available/i)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /start listening/i })).toBeDisabled();
});

test('joined_state_shows_listening_label', () => {
  render(
    <JoinView
      phase="armed"
      hasSources
      connecting={false}
      error={null}
      onStartListening={() => {}}
      onCancel={() => {}}
    />,
  );
  expect(screen.getAllByText(/^listening$/i).length).toBeGreaterThan(0);
});
