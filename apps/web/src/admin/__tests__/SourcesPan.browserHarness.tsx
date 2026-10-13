// Browser-only fixture for the Sources defaults panel. Never imported by a product entrypoint.
// All bridge responses are fixed test values: no host, signaling, media, or persistence.
import { createRoot } from 'react-dom/client';
import { SourcesView } from '../SourcesView';
import type {
  CounterString,
  DeviceInfo,
  SourceInfo,
  SourceLinkPair,
  SourcePanDefault,
  SourceSettings,
} from '../../protocol';
import type { HostBridge } from '../../desktop-bridge/contracts';

const device: DeviceInfo = {
  deviceId: 'fixture-device',
  name: 'Browser-test USB fixture',
  isDefault: false,
  inputChannels: 3,
  sampleFormats: ['F32'],
  sampleRateHz: 48000,
  bufferMinFrames: null,
  bufferMaxFrames: null,
};

const sources: SourceInfo[] = [
  {
    sourceId: 'fixture-mono-a',
    physicalIndex: 0,
    label: 'A deliberately long browser-test vocal source name',
    role: 'inputChannel',
    authorized: true,
    available: true,
    stereoPair: null,
    stereoSide: null,
    defaultPan: 0.5,
  },
  {
    sourceId: 'fixture-left',
    physicalIndex: 1,
    label: 'Browser-test keys left',
    role: 'inputChannel',
    authorized: true,
    available: true,
    stereoPair: 'fixture-right',
    stereoSide: 'left',
    defaultPan: -0.25,
  },
  {
    sourceId: 'fixture-right',
    physicalIndex: 2,
    label: 'Browser-test keys right',
    role: 'inputChannel',
    authorized: true,
    available: true,
    stereoPair: 'fixture-left',
    stereoSide: 'right',
    defaultPan: -0.25,
  },
];

function makeSettings(sourcesList: SourceInfo[]): SourceSettings {
  return {
    deviceId: device.deviceId,
    inputChannels: sourcesList.length,
    settingsRevision: '5' as unknown as CounterString,
    bindingStatus: 'saved',
    durabilityUncertain: false,
    catalog: { catalogRevision: '5' as unknown as CounterString, sources: sourcesList },
  };
}

export function mountSourcesFixture({ hostRunning = false }: { hostRunning?: boolean } = {}) {
  // Self-contained: never depends on the product entrypoint's own `#root`, and each mount creates
  // its own container so a fixture can be disposed and remounted during a run.
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);

  const calls: string[] = [];
  let store = makeSettings(sources);
  const bridge = {
    listDevices: async () => [device],
    listInterfaces: async () => [],
    startHost: async () => ({}) as never,
    stopHost: async () => {},
    sourceCatalog: async () => store.catalog.sources,
    setAvailableSources: async () => store.catalog,
    setSourceLabel: async () => store.catalog,
    readSourceSettings: async () => { calls.push('read'); return store; },
    saveSourceDefaults: async (_deviceId: string, _base: string, defaults: SourcePanDefault[]) => {
      calls.push('save-defaults');
      const pans = new Map(defaults.map(d => [d.sourceId, d.pan]));
      store = {
        ...store,
        settingsRevision: String(Number(store.settingsRevision) + 1) as unknown as CounterString,
        catalog: {
          ...store.catalog,
          sources: store.catalog.sources.map(source =>
            pans.has(source.sourceId) ? { ...source, defaultPan: pans.get(source.sourceId)! } : source,
          ),
        },
      };
      return store;
    },
    saveSourceLinks: async (_deviceId: string, _base: string, _pairs: SourceLinkPair[]) => {
      calls.push('save-links');
      return store;
    },
    issuePairingCredential: async () => ({ joinUrl: '', expiresInSeconds: 0 }),
  } as unknown as HostBridge;

  root.render(
    <main className="iem-panel">
      <SourcesView bridge={bridge} device={device} hostRunning={hostRunning} />
    </main>,
  );
  return { calls, dispose: () => root.unmount() };
}
