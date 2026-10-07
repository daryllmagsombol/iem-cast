import type { ChannelStripState } from '../ChannelStrip';
import type { DiagnosticsSnapshot } from '../../receiver-ports';
import type {
  CatalogSnapshot,
  DeviceInfo,
  HostBridge,
  InterfaceInfo,
  SourceInfo,
  StartHostRequest,
  StartHostResult,
} from '../../protocol';
import type { OutputDeviceInfo, PairingCredential } from '../../desktop-bridge/contracts';

/**
 * Test fixtures for Lane D UI tests. These are private test helpers, never
 * production APIs. Shared protocol types are INT-owned and only imported here.
 */

export function mockStrip(overrides: Partial<ChannelStripState> = {}): ChannelStripState {
  return {
    sourceId: '33333333-3333-4333-8333-333333333333',
    label: 'Lead vocal',
    requestedDb: -12,
    appliedDb: -12,
    muted: false,
    ...overrides,
  };
}

export function diagSnapshot(
  input: {
    rtt?: DiagnosticsSnapshot['network_rtt_ms'];
    buffer?: DiagnosticsSnapshot['buffer_delay_ms'];
    lastUpdated?: number | null;
  } = {},
): DiagnosticsSnapshot {
  return {
    network_rtt_ms: input.rtt ?? 'Unavailable',
    buffer_delay_ms: input.buffer ?? 'Waiting for sample',
    end_to_end: 'Not measured',
    last_updated: input.lastUpdated ?? 1_700_000_000_000,
  };
}

export interface FakeBridge extends HostBridge {
  issueCalls: number;
  logged: string[];
}

export function fakeBridge(credential: PairingCredential): FakeBridge {
  const catalog = { catalogRevision: '34', sources: [] as SourceInfo[] } as unknown as CatalogSnapshot;
  const bridge: FakeBridge = {
    issueCalls: 0,
    logged: [],
    listDevices: async (): Promise<DeviceInfo[]> => [],
    listInterfaces: async (): Promise<InterfaceInfo[]> => [],
    startHost: async (_req: StartHostRequest): Promise<StartHostResult> => ({
      hostEpoch: '',
      audioEpoch: '',
      joinUrl: '',
      catalog,
    }),
    stopHost: async (): Promise<void> => {},
    sourceCatalog: async (): Promise<SourceInfo[]> => [],
    setAvailableSources: async (): Promise<CatalogSnapshot> => catalog,
    setSourceLabel: async (): Promise<CatalogSnapshot> => catalog,
    issuePairingCredential: async (): Promise<PairingCredential> => {
      bridge.issueCalls += 1;
      // Deliberately redacted: never log the one-use fragment token from joinUrl.
      bridge.logged.push('[pairing-credential-issued]');
      return credential;
    },
    listOutputDevices: async (): Promise<OutputDeviceInfo[]> => [],
    startMonitor: async (): Promise<void> => {},
    stopMonitor: async (): Promise<void> => {},
  };
  return bridge;
}
