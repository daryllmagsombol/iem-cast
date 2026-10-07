import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { PairingView } from '../../admin/PairingView';
import { fakeBridge } from './helpers';

test('pairing_view_issues_a_fresh_single_use_credential_and_never_logs_it', async () => {
  const bridge = fakeBridge({ joinUrl: 'https://host.local:8443/join#t=SECRET', expiresInSeconds: 120 });
  render(<PairingView bridge={bridge} />);
  fireEvent.click(screen.getByRole('button', { name: /generate pairing/i }));
  expect(await screen.findByText(/expires in 120 s/i)).toBeInTheDocument();
  expect(bridge.issueCalls).toBe(1); // fresh credential per phone
  expect(bridge.logged).not.toContain('SECRET'); // private token never written to logs
});

test('pairing_view_renders_a_qr_for_the_join_url', async () => {
  const bridge = fakeBridge({ joinUrl: 'https://host.local:8443/join#t=ONCE', expiresInSeconds: 90 });
  render(<PairingView bridge={bridge} />);
  fireEvent.click(screen.getByRole('button', { name: /generate pairing/i }));
  expect(await screen.findByLabelText(/pairing qr code/i)).toBeInTheDocument();
});
