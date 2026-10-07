import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { DeviceView } from '../DeviceView';
import type { DeviceInfo } from '../../protocol';
import type { HostBridge } from '../../desktop-bridge/contracts';
import type { CatalogSnapshot } from '../../protocol';

function bridgeWith(devices: DeviceInfo[]): HostBridge {
  return {
    listDevices: async () => devices,
    listInterfaces: async () => [],
    startHost: async () => ({}) as never,
    stopHost: async () => {},
    sourceCatalog: async () => [],
    setAvailableSources: async () => ({ catalogRevision: '0', sources: [] } as unknown as CatalogSnapshot),
    setSourceLabel: async () => ({ catalogRevision: '0', sources: [] } as unknown as CatalogSnapshot),
    issuePairingCredential: async () => ({ joinUrl: '', expiresInSeconds: 0 }),
  } as unknown as HostBridge;
}

test('device_without_reported_48k_is_rejected_in_the_ui', async () => {
  const device: DeviceInfo = {
    deviceId: 'dev-1',
    name: 'Soundcraft 22 MTK',
    isDefault: true,
    inputChannels: 22,
    sampleFormats: ['F32'],
    sampleRateHz: 44100,
    bufferMinFrames: 64,
    bufferMaxFrames: 1024,
  };
  render(<DeviceView bridge={bridgeWith([device])} />);
  expect(await screen.findByText(/48 khz not reported/i)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /use this device/i })).toBeDisabled();
});

test('device_over_24_channels_is_rejected_not_trimmed', async () => {
  const device: DeviceInfo = {
    deviceId: 'dev-2',
    name: 'Oversized interface',
    isDefault: true,
    inputChannels: 32,
    sampleFormats: ['F32'],
    sampleRateHz: 48000,
    bufferMinFrames: 64,
    bufferMaxFrames: 1024,
  };
  render(<DeviceView bridge={bridgeWith([device])} />);
  expect(await screen.findByText(/more than 24 input channels/i)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /use this device/i })).toBeDisabled();
});

test('empty_device_list_shows_guidance', async () => {
  render(<DeviceView bridge={bridgeWith([])} />);
  expect(await screen.findByText(/no input devices found/i)).toBeInTheDocument();
});
