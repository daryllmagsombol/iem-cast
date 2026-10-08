import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { SourcesView } from '../SourcesView';
import type { DeviceInfo, SourceInfo } from '../../protocol';
import type { CatalogSnapshot } from '../../protocol';
import type { HostBridge } from '../../desktop-bridge/contracts';

const twoChannelDevice: DeviceInfo = {
  deviceId: 'blackhole-2ch',
  name: 'BlackHole 2ch',
  isDefault: true,
  inputChannels: 2,
  sampleFormats: ['F32'],
  sampleRateHz: 48000,
  bufferMinFrames: null,
  bufferMaxFrames: null,
};

const twoChannelSources: SourceInfo[] = [
  {
    sourceId: '00000000-0000-4000-8000-000000000000',
    physicalIndex: 0,
    label: 'Channel 1',
    role: 'inputChannel',
    authorized: true,
    available: true,
    stereoPair: null,
  },
  {
    sourceId: '01010101-0101-4101-8101-010101010101',
    physicalIndex: 1,
    label: 'Channel 2',
    role: 'inputChannel',
    authorized: true,
    available: true,
    stereoPair: null,
  },
];

function bridgeWith(byDevice: Record<string, SourceInfo[]>): HostBridge {
  return {
    listDevices: async () => [],
    listInterfaces: async () => [],
    startHost: async () => ({}) as never,
    stopHost: async () => {},
    sourceCatalog: async (deviceId: string | null) => byDevice[deviceId ?? ''] ?? [],
    setAvailableSources: async () => ({ catalogRevision: '0', sources: [] } as unknown as CatalogSnapshot),
    setSourceLabel: async () => ({ catalogRevision: '0', sources: [] } as unknown as CatalogSnapshot),
    issuePairingCredential: async () => ({ joinUrl: '', expiresInSeconds: 0 }),
  } as unknown as HostBridge;
}

test('renders exactly the selected device channel rows read-only', async () => {
  const bridge = bridgeWith({ 'blackhole-2ch': twoChannelSources });
  bridge.sourceCatalog = vi.fn(bridge.sourceCatalog);
  render(<SourcesView bridge={bridge} device={twoChannelDevice} />);

  expect(await screen.findByText('Channel 1')).toBeInTheDocument();
  expect(screen.getByText('Channel 2')).toBeInTheDocument();
  expect(bridge.sourceCatalog).toHaveBeenCalledWith('blackhole-2ch');
  // Two rows, no fabricated 22-channel draft, and no editable name inputs.
  const rows = screen.getAllByRole('listitem');
  expect(rows).toHaveLength(2);
  expect(screen.queryByText('Channel 22')).not.toBeInTheDocument();
  expect(screen.queryByDisplayValue('Channel 1')).not.toBeInTheDocument();
});

test('no device selected shows the empty guidance and never calls the catalog', () => {
  const bridge = bridgeWith({});
  bridge.sourceCatalog = vi.fn(bridge.sourceCatalog);
  render(<SourcesView bridge={bridge} device={null} />);

  expect(screen.getByText(/select a capture device/i)).toBeInTheDocument();
  expect(bridge.sourceCatalog).not.toHaveBeenCalled();
});

test('a device reporting no channels shows an honest empty state', async () => {
  render(<SourcesView bridge={bridgeWith({ 'blackhole-2ch': [] })} device={twoChannelDevice} />);
  expect(await screen.findByText(/reports no source channels/i)).toBeInTheDocument();
});

test('label and publish stay disabled and never call the mutation RPCs', async () => {
  const bridge = bridgeWith({ 'blackhole-2ch': twoChannelSources });
  bridge.setAvailableSources = vi.fn();
  bridge.setSourceLabel = vi.fn();
  render(<SourcesView bridge={bridge} device={twoChannelDevice} />);
  await screen.findByText('Channel 1');

  const save = screen.getByRole('button', { name: 'Save label' });
  const publish = screen.getByRole('button', { name: 'Publish sources' });
  expect(save).toBeDisabled();
  expect(publish).toBeDisabled();
  fireEvent.click(save);
  fireEvent.click(publish);
  await waitFor(() => expect(bridge.setAvailableSources).not.toHaveBeenCalled());
  expect(bridge.setSourceLabel).not.toHaveBeenCalled();
});
