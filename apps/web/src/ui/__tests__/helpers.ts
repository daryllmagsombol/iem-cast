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
import type { HostDefaults, OutputDeviceInfo, PairingCredential } from '../../desktop-bridge/contracts';

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
    target?: DiagnosticsSnapshot['buffer_target_delay_ms'];
    minimum?: DiagnosticsSnapshot['buffer_minimum_delay_ms'];
    lost?: DiagnosticsSnapshot['packets_lost_delta'];
    concealment?: DiagnosticsSnapshot['concealment_percent'];
    preference?: DiagnosticsSnapshot['jitter_buffer_preference'];
    lastUpdated?: number | null;
  } = {},
): DiagnosticsSnapshot {
  return {
    network_rtt_ms: input.rtt ?? 'Unavailable',
    buffer_delay_ms: input.buffer ?? 'Waiting for sample',
    // Additive compatibility fields; unchanged behavior for existing callers.
    buffer_target_delay_ms: input.target ?? 'Unavailable',
    buffer_minimum_delay_ms: input.minimum ?? 'Unavailable',
    packets_lost_delta: input.lost ?? 'Unavailable',
    concealment_percent: input.concealment ?? 'Unavailable',
    jitter_buffer_preference: input.preference ?? { state: 'Not requested', requested_ms: null },
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
    sourceCatalog: async (_deviceId: string | null): Promise<SourceInfo[]> => [],
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
    // No discoverable defaults by default; null fields leave the inputs blank.
    hostDefaults: async (): Promise<HostDefaults> => ({
      interfaceName: null,
      interfaceIp: null,
      certificatePath: null,
      keyPath: null,
    }),
  };
  return bridge;
}
