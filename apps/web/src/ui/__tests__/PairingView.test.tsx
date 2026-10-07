import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { PairingView } from '../../admin/PairingView';
import { fakeBridge } from './helpers';

afterEach(cleanup);

test('pairing stays disabled until the host is running, and never issues a placeholder credential', () => {
  const bridge = fakeBridge({ joinUrl: 'https://host.local/#SECRET', expiresInSeconds: 120 });
  render(<PairingView bridge={bridge} hostRunning={false} />);
  const button = screen.getByRole('button', { name: 'Generate pairing code' });
  expect(button).toBeDisabled();
  fireEvent.click(button);
  expect(bridge.issueCalls).toBe(0);
  expect(screen.queryByTestId('pairing-qr')).not.toBeInTheDocument();
  expect(screen.getAllByText(/start the host first/i).length).toBeGreaterThanOrEqual(1);
});

test('a running host renders the scannable join URL as a QR code with its expiry', async () => {
  const bridge = fakeBridge({ joinUrl: 'https://host.local:8443/join#t=secret', expiresInSeconds: 120 });
  render(<PairingView bridge={bridge} hostRunning />);
  fireEvent.click(screen.getByRole('button', { name: 'Generate pairing code' }));

  const qr = await screen.findByTestId('pairing-qr');
  expect(qr.querySelector('svg')).not.toBeNull();
  expect(qr.querySelector('title')?.textContent).toMatch(/pairing qr/i);
  expect(bridge.issueCalls).toBe(1);
  expect(screen.getByText(/Expires in 120 seconds/)).toBeInTheDocument();
  // The raw token is only inside the QR value, never rendered as separate text.
  expect(screen.queryByText(/t=secret/)).not.toBeInTheDocument();
  // Issue is logged only in redacted form.
  expect(bridge.logged).toEqual(['[pairing-credential-issued]']);
});

test('host-not-running pairing failure shows guidance instead of a code', async () => {
  const bridge = fakeBridge({ joinUrl: '', expiresInSeconds: 0 });
  bridge.issuePairingCredential = async () => {
    const error = new Error('no host is running');
    (error as Error & { code: string }).code = 'HOST_NOT_RUNNING';
    throw error;
  };
  render(<PairingView bridge={bridge} hostRunning />);
  fireEvent.click(screen.getByRole('button', { name: 'Generate pairing code' }));

  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent('no host is running');
  expect(alert).toHaveTextContent(/Start the host/i);
  expect(screen.queryByTestId('pairing-qr')).not.toBeInTheDocument();
});

test('a credential with no join URL never renders an empty QR', async () => {
  const bridge = fakeBridge({ joinUrl: '', expiresInSeconds: 120 });
  render(<PairingView bridge={bridge} hostRunning />);
  fireEvent.click(screen.getByRole('button', { name: 'Generate pairing code' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(/no join URL/i);
  expect(screen.queryByTestId('pairing-qr')).not.toBeInTheDocument();
});

test('local monitor requires an explicit listener confirmation before starting', async () => {
  const bridge = fakeBridge({ joinUrl: '', expiresInSeconds: 120 });
  bridge.listOutputDevices = vi.fn().mockResolvedValue([
    { id: 'out-usb', name: 'USB Monitor Out', isDefault: false },
  ]);
  bridge.startMonitor = vi.fn().mockResolvedValue(undefined);
  bridge.stopMonitor = vi.fn().mockResolvedValue(undefined);
  render(<PairingView bridge={bridge} hostRunning />);

  await screen.findByRole('option', { name: /USB Monitor Out/ });
  const start = screen.getByRole('button', { name: 'Start monitor' });
  expect(start).toBeDisabled();
  expect(screen.getByText(/Confirm a phone has joined/i)).toBeInTheDocument();

  fireEvent.change(screen.getByLabelText('Monitor output device'), {
    target: { value: 'out-usb' },
  });
  fireEvent.click(screen.getByRole('checkbox', { name: /A phone has joined this slot/ }));
  expect(start).toBeEnabled();
  fireEvent.click(start);

  await waitFor(() =>
    expect(bridge.startMonitor).toHaveBeenCalledWith(0, 'out-usb'),
  );
});

test('local monitor surfaces a start failure and never reports running', async () => {
  const bridge = fakeBridge({ joinUrl: '', expiresInSeconds: 120 });
  bridge.listOutputDevices = vi.fn().mockResolvedValue([]);
  bridge.startMonitor = vi.fn().mockRejectedValue(new Error('no host is running'));
  render(<PairingView bridge={bridge} hostRunning />);

  fireEvent.click(await screen.findByRole('checkbox', { name: /A phone has joined this slot/ }));
  fireEvent.click(screen.getByRole('button', { name: 'Start monitor' }));

  expect(await screen.findByRole('alert')).toHaveTextContent('no host is running');
  expect(screen.queryByText(/Monitor running/)).not.toBeInTheDocument();
});
