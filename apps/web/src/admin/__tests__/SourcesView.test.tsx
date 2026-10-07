import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { SourcesView } from '../SourcesView';
import type { SourceInfo } from '../../protocol';
import type { HostBridge } from '../../desktop-bridge/contracts';
import type { CatalogSnapshot } from '../../protocol';

const sources: SourceInfo[] = [
  {
    sourceId: 'aaaaaaaa-0000-4000-8000-000000000001',
    physicalIndex: 0,
    label: 'Lead vocal',
    role: 'inputChannel',
    authorized: true,
    available: true,
    stereoPair: null,
  },
];

function bridgeWith(list: SourceInfo[]): HostBridge {
  return {
    listDevices: async () => [],
    listInterfaces: async () => [],
    startHost: async () => ({}) as never,
    stopHost: async () => {},
    sourceCatalog: async () => list,
    setAvailableSources: async () => ({ catalogRevision: '34', sources: list } as unknown as CatalogSnapshot),
    setSourceLabel: async () => ({ catalogRevision: '35', sources: list } as unknown as CatalogSnapshot),
    issuePairingCredential: async () => ({ joinUrl: '', expiresInSeconds: 0 }),
  } as unknown as HostBridge;
}

test('sources_view_lists_published_channels_with_stable_identity', async () => {
  render(<SourcesView bridge={bridgeWith(sources)} />);
  expect(await screen.findByDisplayValue('Lead vocal')).toBeInTheDocument();
  expect(screen.getByLabelText(/^label$/i)).toBeInTheDocument();
});

test('sources_view_shows_empty_guidance_without_sources', async () => {
  render(<SourcesView bridge={bridgeWith([])} />);
  expect(await screen.findByText(/no sources available/i)).toBeInTheDocument();
});
