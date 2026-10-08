import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { AdminRoot } from '../AdminRoot';
import { fakeBridge } from '../../ui/__tests__/helpers';
import type { DeviceInfo, InterfaceInfo } from '../../protocol';

afterEach(cleanup);

const device: DeviceInfo = {
  deviceId: 'usb',
  name: 'Reported USB input',
  isDefault: false,
  inputChannels: 4,
  sampleFormats: ['F32'],
  sampleRateHz: 48000,
  bufferMinFrames: null,
  bufferMaxFrames: null,
};

const iface: InterfaceInfo = { name: 'Ethernet', ipAddress: '192.168.1.10', prefix: 24 };

function bridgeReady() {
  const bridge = fakeBridge({ joinUrl: '', expiresInSeconds: 120 });
  bridge.listDevices = vi.fn().mockResolvedValue([device]);
  bridge.listInterfaces = vi.fn().mockResolvedValue([iface]);
  return bridge;
}

async function configureReadyToStart() {
  fireEvent.click(await screen.findByRole('button', { name: 'Select device' }));
  fireEvent.change(screen.getByLabelText('Host network interface'), {
    target: { value: '192.168.1.10' },
  });
  fireEvent.change(screen.getByLabelText('Certificate path'), { target: { value: '/cert.pem' } });
  fireEvent.change(screen.getByLabelText('Key path'), { target: { value: '/key.pem' } });
}

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

test('host summary reports no running host until one actually starts', () => {
  render(<AdminRoot />);
  const summary = screen.getByRole('heading', { name: 'Host summary' }).closest('section');
  expect(summary).not.toBeNull();
  expect(within(summary!).getByText('Not running')).toBeInTheDocument();
  expect(within(summary!).getAllByText('Unavailable').length).toBeGreaterThanOrEqual(1);
  expect(within(summary!).getByText(/No operation is reported running/)).toBeInTheDocument();
});

test('browser dashboard is useful but never implies a native host', () => {
  render(<AdminRoot />);
  expect(screen.getByRole('heading', { name: 'Network & certificates' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Start host' })).toBeDisabled();
  expect(screen.queryByText('Host startup is incomplete in this build.')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Open simulated preview' })).toBeEnabled();
});

test('Start host enables only with a device, interface and both paths; Stop only while running', async () => {
  const bridge = bridgeReady();
  bridge.startHost = vi.fn().mockResolvedValue({
    hostEpoch: 'host-epoch-1',
    audioEpoch: 'audio-epoch-1',
    joinUrl: 'https://192.168.1.10:8443/join#t=secret',
    catalog: { catalogRevision: '0', sources: [] },
  });
  bridge.stopHost = vi.fn().mockResolvedValue(undefined);
  render(<AdminRoot bridge={bridge} />);

  await screen.findByText('Reported USB input');
  const start = () => screen.getByRole('button', { name: 'Start host' });
  const stop = () => screen.getByRole('button', { name: 'Stop host' });

  // Nothing selected yet.
  expect(start()).toBeDisabled();
  expect(stop()).toBeDisabled();

  fireEvent.click(screen.getByRole('button', { name: 'Select device' }));
  expect(start()).toBeDisabled(); // device only

  fireEvent.change(screen.getByLabelText('Host network interface'), {
    target: { value: '192.168.1.10' },
  });
  expect(start()).toBeDisabled(); // device + interface, no paths

  fireEvent.change(screen.getByLabelText('Certificate path'), { target: { value: '/cert.pem' } });
  expect(start()).toBeDisabled(); // certificate only

  fireEvent.change(screen.getByLabelText('Key path'), { target: { value: '/key.pem' } });
  expect(start()).toBeEnabled(); // device + interface + both paths

  fireEvent.click(start());
  await screen.findByText('Host running');

  expect(bridge.startHost).toHaveBeenCalledWith({
    capture: { deviceId: 'usb', sampleRateHz: 48000, bufferFrames: 128 },
    interface: iface,
    certificatePath: '/cert.pem',
    keyPath: '/key.pem',
  });
  // Real running state shows the returned join URL and disables a second start.
  expect(screen.getByDisplayValue('https://192.168.1.10:8443/join#t=secret')).toBeInTheDocument();
  expect(start()).toBeDisabled();
  expect(stop()).toBeEnabled();

  fireEvent.click(stop());
  await waitFor(() => expect(bridge.stopHost).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(screen.queryByText('Host running')).not.toBeInTheDocument());
  expect(start()).toBeEnabled();
});

test('host start failure is surfaced inline and never fakes a running state', async () => {
  const bridge = bridgeReady();
  bridge.startHost = vi
    .fn()
    .mockRejectedValue(new Error('certificate or key is missing or unreadable'));
  render(<AdminRoot bridge={bridge} />);
  await screen.findByText('Reported USB input');
  await configureReadyToStart();

  fireEvent.click(screen.getByRole('button', { name: 'Start host' }));

  expect(await screen.findByRole('alert')).toHaveTextContent(
    'certificate or key is missing or unreadable',
  );
  expect(screen.queryByText('Host running')).not.toBeInTheDocument();
});

test('real enumeration, selection and rescan do not call unsupported mutation RPCs', async () => {
  const bridge = fakeBridge({ joinUrl: 'https://fake.invalid', expiresInSeconds: 120 });
  bridge.listDevices = vi.fn().mockResolvedValue([device]);
  bridge.listInterfaces = vi.fn().mockResolvedValue([iface]);
  bridge.sourceCatalog = vi.fn().mockResolvedValue([]);
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
  // The host has not been started, so pairing stays unavailable and no mutation fires.
  expect(screen.getByRole('button', { name: 'Generate pairing code' })).toBeDisabled();
  // The sources step exposes every reported channel as already available; there is no publish
  // control that could imply a step the host does not require.
  fireEvent.click(screen.getByRole('button', { name: 'Rescan devices' }));
  expect(bridge.listDevices).toHaveBeenCalledTimes(2);
  // Selecting a device triggers the read-only source catalog for that device; mutation RPCs never fire.
  expect(bridge.sourceCatalog).toHaveBeenCalledWith('usb');
  for (const method of [bridge.startHost, bridge.stopHost, bridge.setSourceLabel, bridge.setAvailableSources, bridge.issuePairingCredential]) expect(method).not.toHaveBeenCalled();
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
