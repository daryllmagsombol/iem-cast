import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent } from '@testing-library/react';
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

test('synthetic_catalog_is_not_presented_as_verified_sources_and_mutations_are_disabled', () => {
  const bridge = bridgeWith(sources);
  bridge.sourceCatalog = vi.fn(); bridge.setAvailableSources = vi.fn(); bridge.setSourceLabel = vi.fn();
  render(<SourcesView bridge={bridge} />);
  expect(screen.queryByDisplayValue('Lead vocal')).not.toBeInTheDocument();
  const save = screen.getByRole('button', { name: 'Save label' });
  const publish = screen.getByRole('button', { name: 'Publish sources' });
  expect(save).toBeDisabled(); expect(publish).toBeDisabled();
  fireEvent.click(save); fireEvent.click(publish);
  expect(bridge.sourceCatalog).not.toHaveBeenCalled();
  expect(bridge.setAvailableSources).not.toHaveBeenCalled(); expect(bridge.setSourceLabel).not.toHaveBeenCalled();
});

test('sources_view_shows_empty_guidance_without_sources', async () => {
  render(<SourcesView bridge={bridgeWith([])} />);
  expect(await screen.findByText(/no sources available/i)).toBeInTheDocument();
});
