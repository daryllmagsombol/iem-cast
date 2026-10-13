import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
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

afterEach(cleanup);

const deviceA: DeviceInfo = {
  deviceId: 'dev-a',
  name: 'Device A interface',
  isDefault: true,
  inputChannels: 2,
  sampleFormats: ['F32'],
  sampleRateHz: 48000,
  bufferMinFrames: null,
  bufferMaxFrames: null,
};

const deviceB: DeviceInfo = {
  deviceId: 'dev-b',
  name: 'Device B interface',
  isDefault: false,
  inputChannels: 1,
  sampleFormats: ['F32'],
  sampleRateHz: 48000,
  bufferMinFrames: null,
  bufferMaxFrames: null,
};

function mono(sourceId: string, physicalIndex: number, label: string, defaultPan = 0): SourceInfo {
  return {
    sourceId,
    physicalIndex,
    label,
    role: 'inputChannel',
    authorized: true,
    available: true,
    stereoPair: null,
    stereoSide: null,
    defaultPan,
  };
}

const sourceA = mono('src-a', 0, 'Channel 1');
const sourceB = mono('src-b', 1, 'Channel 2');

function makeSettings(
  deviceId: string,
  revision: string,
  sources: SourceInfo[],
  overrides: Partial<SourceSettings> = {},
): SourceSettings {
  return {
    deviceId,
    inputChannels: sources.length,
    settingsRevision: revision as unknown as CounterString,
    bindingStatus: 'saved',
    durabilityUncertain: false,
    catalog: { catalogRevision: revision as unknown as CounterString, sources },
    ...overrides,
  };
}

interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason: unknown): void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Apply a saved default vector onto a settings view, as a real host would commit it. */
function applyDefaults(settings: SourceSettings, defaults: SourcePanDefault[], revision: string): SourceSettings {
  const pans = new Map(defaults.map(d => [d.sourceId, d.pan]));
  return {
    ...settings,
    settingsRevision: revision as unknown as CounterString,
    catalog: {
      ...settings.catalog,
      catalogRevision: revision as unknown as CounterString,
      sources: settings.catalog.sources.map(source =>
        pans.has(source.sourceId) ? { ...source, defaultPan: pans.get(source.sourceId)! } : source,
      ),
    },
  };
}

/**
 * Extends SourcesView.test.tsx `bridgeWith` with the Task 5 settings commands. `saveSourceDefaults`
 * is stateful: it commits the complete vector and returns a newer revision, so `Saved` reflects a
 * genuine round trip rather than a value the test fabricated.
 */
function bridgeWith(
  catalogs: Record<string, SourceInfo[]>,
  initialSettings: Record<string, SourceSettings>,
): HostBridge {
  const store: Record<string, SourceSettings> = { ...initialSettings };
  return {
    listDevices: async () => [],
    listInterfaces: async () => [],
    startHost: async () => ({}) as never,
    stopHost: async () => {},
    sourceCatalog: async (deviceId: string | null) => catalogs[deviceId ?? ''] ?? [],
    setAvailableSources: async () => ({ catalogRevision: '0', sources: [] }) as never,
    setSourceLabel: async () => ({ catalogRevision: '0', sources: [] }) as never,
    readSourceSettings: async (deviceId: string) => store[deviceId]!,
    saveSourceDefaults: vi.fn(async (deviceId: string, _base: string, defaults: SourcePanDefault[]) => {
      const committed = applyDefaults(store[deviceId]!, defaults, String(Number(store[deviceId]!.settingsRevision) + 1));
      store[deviceId] = committed;
      return committed;
    }),
    saveSourceLinks: vi.fn(async (deviceId: string, _base: string, pairs: SourceLinkPair[]) => {
      const byId = new Map(store[deviceId]!.catalog.sources.map(s => [s.sourceId, s]));
      const links = new Map<string, { peer: string; side: 'left' | 'right' }>();
      for (const pair of pairs) {
        links.set(pair.leftSourceId, { peer: pair.rightSourceId, side: 'left' });
        links.set(pair.rightSourceId, { peer: pair.leftSourceId, side: 'right' });
      }
      const linked: SourceSettings = {
        ...store[deviceId]!,
        settingsRevision: String(Number(store[deviceId]!.settingsRevision) + 1) as unknown as CounterString,
        bindingStatus: 'saved',
        catalog: {
          ...store[deviceId]!.catalog,
          sources: store[deviceId]!.catalog.sources.map(source => {
            const link = links.get(source.sourceId);
            return link && byId.has(link.peer)
              ? { ...source, stereoPair: link.peer, stereoSide: link.side, defaultPan: 0 }
              : { ...source, stereoPair: null, stereoSide: null };
          }),
        },
      };
      store[deviceId] = linked;
      return linked;
    }),
    issuePairingCredential: async () => ({ joinUrl: '', expiresInSeconds: 0 }),
  } as unknown as HostBridge;
}

test('a valid reciprocal pair renders one grouped "Default balance" entry', async () => {
  const left: SourceInfo = { ...mono('src-l', 0, 'Keys L'), stereoPair: 'src-r', stereoSide: 'left', defaultPan: -0.25 };
  const right: SourceInfo = { ...mono('src-r', 1, 'Keys R'), stereoPair: 'src-l', stereoSide: 'right', defaultPan: -0.25 };
  const bridge = bridgeWith({ 'dev-a': [left, right] }, { 'dev-a': makeSettings('dev-a', '1', [left, right]) });
  render(<SourcesView bridge={bridge} device={deviceA} />);

  const balance = await screen.findByRole('slider', { name: /Keys L and Keys R default balance/ });
  expect(balance).toHaveValue('-25');
  expect(screen.queryByRole('slider', { name: /Keys L default pan/ })).not.toBeInTheDocument();
});

test('Link saves a stopped-host pair and groups the two members', async () => {
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': makeSettings('dev-a', '1', [sourceA, sourceB]) });
  render(<SourcesView bridge={bridge} device={deviceA} />);

  fireEvent.change(await screen.findByLabelText('Left source'), { target: { value: 'src-a' } });
  fireEvent.change(screen.getByLabelText('Right source'), { target: { value: 'src-b' } });
  fireEvent.click(screen.getByRole('button', { name: 'Link' }));

  await waitFor(() =>
    expect(bridge.saveSourceLinks).toHaveBeenCalledWith('dev-a', '1', [{ leftSourceId: 'src-a', rightSourceId: 'src-b' }], false),
  );
  expect(await screen.findByRole('slider', { name: /Channel 1 and Channel 2 default balance/ })).toBeInTheDocument();
});

test('stereo link actions are disabled while the host is running', async () => {
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': makeSettings('dev-a', '1', [sourceA, sourceB]) });
  render(<SourcesView bridge={bridge} device={deviceA} hostRunning />);

  await screen.findByRole('heading', { name: 'Defaults for new personal mixes' });
  expect(screen.getByRole('button', { name: 'Link' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Unlink' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Reverse' })).toBeDisabled();
  // Defaults editing is allowed while running; only link actions are gated on the host being stopped.
  expect(screen.getByRole('slider', { name: /Channel 1 default pan/ })).toBeEnabled();
});

test('link actions stay enabled while the host is stopped (the default)', async () => {
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': makeSettings('dev-a', '1', [sourceA, sourceB]) });
  render(<SourcesView bridge={bridge} device={deviceA} />);

  await screen.findByRole('button', { name: 'Link' });
  expect(screen.getByRole('button', { name: 'Link' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Unlink' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Reverse' })).toBeEnabled();
});

test('changing a mono default pan to -50% saves the complete vector against the current revision', async () => {
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': makeSettings('dev-a', '7', [sourceA, sourceB]) });
  render(<SourcesView bridge={bridge} device={deviceA} />);

  const slider = await screen.findByRole('slider', { name: /Channel 1 default pan/ });
  fireEvent.change(slider, { target: { value: '-50' } });

  await waitFor(() => expect(bridge.saveSourceDefaults).toHaveBeenCalledTimes(1));
  expect(bridge.saveSourceDefaults).toHaveBeenCalledWith(
    'dev-a',
    '7',
    [
      { sourceId: 'src-a', pan: -0.5 },
      { sourceId: 'src-b', pan: 0 },
    ],
    false,
  );
  // Only a genuinely committed round trip is labelled Saved.
  expect(await screen.findByText('Saved')).toBeInTheDocument();
});

test('a rejected save shows failed with Retry; a stale older-revision response keeps the newer draft', async () => {
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': makeSettings('dev-a', '5', [sourceA, sourceB]) });
  const first = deferred<SourceSettings>();
  const second = deferred<SourceSettings>();
  (bridge.saveSourceDefaults as ReturnType<typeof vi.fn>)
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);

  render(<SourcesView bridge={bridge} device={deviceA} />);
  const slider = await screen.findByRole('slider', { name: /Channel 1 default pan/ });

  fireEvent.change(slider, { target: { value: '-50' } });
  await waitFor(() => expect(bridge.saveSourceDefaults).toHaveBeenCalledTimes(1));

  first.reject(new Error('disk full'));
  expect(await screen.findByText(/failed/)).toBeInTheDocument();
  expect(await screen.findByRole('button', { name: 'Retry' })).toBeInTheDocument();

  // A newer draft while the stale save is still outstanding.
  fireEvent.change(slider, { target: { value: '50' } });
  await waitFor(() => expect(bridge.saveSourceDefaults).toHaveBeenCalledTimes(2));
  expect(slider).toHaveValue('50');

  // The stale response carries an older committed revision; it must not overwrite the newer draft.
  second.resolve(makeSettings('dev-a', '4', [sourceA, sourceB]));
  await waitFor(() => expect(screen.getByText(/failed/)).toBeInTheDocument());
  expect(screen.queryByText('Saved')).not.toBeInTheDocument();
  expect(slider).toHaveValue('50');
});

test('switching device mid-flight discards the previous device save response', async () => {
  const bridge = bridgeWith(
    { 'dev-a': [sourceA, sourceB], 'dev-b': [mono('src-b1', 0, 'Device B input', 0.25)] },
    {
      'dev-a': makeSettings('dev-a', '1', [sourceA, sourceB]),
      'dev-b': makeSettings('dev-b', '9', [mono('src-b1', 0, 'Device B input', 0.25)]),
    },
  );
  const pending = deferred<SourceSettings>();
  (bridge.saveSourceDefaults as ReturnType<typeof vi.fn>).mockReturnValueOnce(pending.promise);

  const { rerender } = render(<SourcesView bridge={bridge} device={deviceA} />);
  const sliderA = await screen.findByRole('slider', { name: /Channel 1 default pan/ });
  fireEvent.change(sliderA, { target: { value: '-50' } });
  await waitFor(() => expect(bridge.saveSourceDefaults).toHaveBeenCalledTimes(1));

  rerender(<SourcesView bridge={bridge} device={deviceB} />);
  await waitFor(() =>
    expect(screen.getByRole('slider', { name: /Device B input default pan/ })).toHaveValue('25'),
  );

  // The old device's save resolves late; it must not touch the newly selected device's state.
  pending.resolve(makeSettings('dev-a', '2', [mono('src-a', 0, 'Channel 1', -0.5), sourceB]));
  await new Promise(r => setTimeout(r, 0));

  expect(screen.getByRole('slider', { name: /Device B input default pan/ })).toHaveValue('25');
  expect(screen.queryByText('Saved')).not.toBeInTheDocument();
});
