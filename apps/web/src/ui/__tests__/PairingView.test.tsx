import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test } from 'vitest';
import { PairingView } from '../../admin/PairingView';
import { fakeBridge } from './helpers';
afterEach(cleanup);
test('unsupported pairing never issues a placeholder credential or renders a fake QR', () => {
  const bridge = fakeBridge({ joinUrl: 'https://host.local/#SECRET', expiresInSeconds: 120 });
  render(<PairingView bridge={bridge} />);
  const button = screen.getByRole('button', { name: 'Generate pairing code' });
  expect(button).toBeDisabled();
  fireEvent.click(button);
  expect(bridge.issueCalls).toBe(0);
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  expect(screen.getByText('Pairing requires the host’s secure join service.')).toBeInTheDocument();
});
