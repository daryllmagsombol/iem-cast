import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { AdminRoot } from '../AdminRoot';
import { fakeBridge } from '../../ui/__tests__/helpers';

afterEach(cleanup);

test('browser preview is the primary action and navigation marks only the current section', () => {
  render(<AdminRoot />);
  expect(screen.getByRole('button', { name: 'Open simulated preview' })).toHaveClass('bg-accent');
  const capture = screen.getByRole('link', { name: /Capture device/ });
  const network = screen.getByRole('link', { name: /Network & certificates/ });
  expect(capture).toHaveAttribute('aria-current', 'location');
  fireEvent.click(network);
  expect(network).toHaveAttribute('aria-current', 'location');
  expect(capture).not.toHaveAttribute('aria-current');
  expect(screen.queryByText('Complete')).not.toBeInTheDocument();
});

test('Capture summary reports unknown health without a backend status probe', () => {
  render(<AdminRoot />);
  const summary = screen.getByRole('heading', { name: 'Host summary' }).closest('section');
  expect(summary).not.toBeNull();
  expect(within(summary!).getByText('Not confirmed')).toBeInTheDocument();
  expect(within(summary!).queryByText('Not running')).not.toBeInTheDocument();
  expect(within(summary!).getByText(/No operation is reported running/)).toBeInTheDocument();
});

test('browser dashboard is useful but never implies a native host', () => {
  render(<AdminRoot />);
  expect(screen.getByRole('heading', { name: 'Network & certificates' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Start host' })).toBeDisabled();
  expect(screen.getByText('Host startup is incomplete in this build.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Open simulated preview' })).toBeEnabled();
});

test('real enumeration, selection and rescan do not call unsupported mutation RPCs', async () => {
  const bridge = fakeBridge({ joinUrl: 'https://fake.invalid', expiresInSeconds: 120 });
  bridge.listDevices = vi.fn().mockResolvedValue([{ deviceId: 'usb', name: 'Reported USB input', isDefault: false, inputChannels: 4, sampleFormats: ['F32'], sampleRateHz: 48000, bufferMinFrames: null, bufferMaxFrames: null }]);
  bridge.listInterfaces = vi.fn().mockResolvedValue([{ name: 'Ethernet', ipAddress: '192.168.1.10', prefix: 24 }]);
  bridge.sourceCatalog = vi.fn();
  bridge.startHost = vi.fn();
  bridge.stopHost = vi.fn();
  bridge.setSourceLabel = vi.fn();
  bridge.setAvailableSources = vi.fn();
  bridge.issuePairingCredential = vi.fn();
  render(<AdminRoot bridge={bridge} />);
  await screen.findByText('Reported USB input');
  fireEvent.click(screen.getByRole('button', { name: 'Select device' }));
  expect(screen.getByText(/Selected: Reported USB input/)).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Certificate path'), { target: { value: '/local/cert.pem' } });
  expect(screen.getByLabelText('Certificate path')).toHaveValue('/local/cert.pem');
  expect(screen.getByRole('button', { name: 'Generate pairing code' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Save label' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Rescan devices' }));
  expect(bridge.listDevices).toHaveBeenCalledTimes(2);
  for (const method of [bridge.sourceCatalog, bridge.startHost, bridge.stopHost, bridge.setSourceLabel, bridge.setAvailableSources, bridge.issuePairingCredential]) expect(method).not.toHaveBeenCalled();
});

test('enumeration error has retry and never supplies fictional devices', async () => {
  const bridge = fakeBridge({ joinUrl: '', expiresInSeconds: 0 });
  bridge.listDevices = vi.fn().mockRejectedValue(new Error('USB enumeration unavailable'));
  render(<AdminRoot bridge={bridge} />);
  expect(await screen.findByText('USB enumeration unavailable')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Rescan devices' })).toBeEnabled();
  expect(screen.queryByText('Soundcraft 22 MTK')).not.toBeInTheDocument();
});

test('preview has isolated fictional data, no RPC calls, and a usable exit', async () => {
  const bridge = fakeBridge({ joinUrl: '', expiresInSeconds: 0 });
  bridge.listDevices = vi.fn().mockResolvedValue([]);
  render(<AdminRoot bridge={bridge} />);
  await screen.findByText('No input devices found');
  const count = vi.mocked(bridge.listDevices).mock.calls.length;
  fireEvent.click(screen.getByRole('button', { name: 'Open simulated preview' }));
  expect(screen.getByText('SIMULATED · No host connection · No audio')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Select simulated device' }));
  expect(screen.getByText('Simulated device selected — no capture')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Unmute simulated Lead vocal' }));
  expect(screen.getByRole('button', { name: 'Mute simulated Lead vocal' })).toBeInTheDocument();
  expect(bridge.listDevices).toHaveBeenCalledTimes(count);
  expect(bridge.issueCalls).toBe(0);
  fireEvent.click(screen.getByRole('button', { name: 'Exit preview' }));
  expect(screen.getByRole('heading', { name: 'Operator dashboard' })).toBeInTheDocument();
});
