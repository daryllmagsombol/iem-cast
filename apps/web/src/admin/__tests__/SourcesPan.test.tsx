import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { SourcesView } from '../SourcesView';
import { AdminRoot } from '../AdminRoot';
import type {
  CounterString,
  DeviceInfo,
  InterfaceInfo,
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

  // The stale response carries an older committed revision; it must not overwrite the newer draft
  // and it is recovery-by-reload, not a retry against the stale base.
  second.resolve(makeSettings('dev-a', '4', [sourceA, sourceB]));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Reload' })).toBeInTheDocument());
  expect(screen.getByText(/changed underneath this request/i)).toBeInTheDocument();
  expect(screen.queryByText('Saved')).not.toBeInTheDocument();
  expect(slider).toHaveValue('50');
});

test('a durability-uncertain save shows the uncertain state with Reload and re-reads on reload', async () => {
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': makeSettings('dev-a', '7', [sourceA, sourceB]) });
  bridge.readSourceSettings = vi.fn(bridge.readSourceSettings);
  (bridge.saveSourceDefaults as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
    Object.assign(new Error('settings were installed but restart durability is unconfirmed; reload before retrying'), {
      code: 'SOURCE_SETTINGS_DURABILITY_UNCERTAIN',
    }),
  );

  render(<SourcesView bridge={bridge} device={deviceA} />);
  const slider = await screen.findByRole('slider', { name: /Channel 1 default pan/ });
  fireEvent.change(slider, { target: { value: '-50' } });

  // An uncertain commit must not read as a plain failure with a retry that can never succeed.
  expect(await screen.findByText(/restart durability was not confirmed/i)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Reload' })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();

  // Reload must genuinely re-read settings, not resubmit with the stale base revision.
  const readsBefore = (bridge.readSourceSettings as ReturnType<typeof vi.fn>).mock.calls.length;
  fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
  await waitFor(() =>
    expect((bridge.readSourceSettings as ReturnType<typeof vi.fn>).mock.calls.length).toBeGreaterThan(readsBefore),
  );
  expect(bridge.saveSourceDefaults).toHaveBeenCalledTimes(1);
});

test('a revision-conflict save surfaces the host message and offers Reload, not Retry', async () => {
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': makeSettings('dev-a', '7', [sourceA, sourceB]) });
  (bridge.saveSourceDefaults as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
    Object.assign(new Error('base revision 7 does not match the committed revision 8'), {
      code: 'SOURCE_SETTINGS_REVISION_CONFLICT',
    }),
  );

  render(<SourcesView bridge={bridge} device={deviceA} />);
  const slider = await screen.findByRole('slider', { name: /Channel 1 default pan/ });
  fireEvent.change(slider, { target: { value: '-50' } });

  expect(await screen.findByText(/does not match the committed revision/i)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Reload' })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
});

test('a failed link save Retry replays the intended pair set and never reports Saved', async () => {
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': makeSettings('dev-a', '1', [sourceA, sourceB]) });
  (bridge.saveSourceLinks as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('link write failed'));

  render(<SourcesView bridge={bridge} device={deviceA} />);
  fireEvent.change(await screen.findByLabelText('Left source'), { target: { value: 'src-a' } });
  fireEvent.change(screen.getByLabelText('Right source'), { target: { value: 'src-b' } });
  fireEvent.click(screen.getByRole('button', { name: 'Link' }));

  expect(await screen.findByText('failed')).toBeInTheDocument();
  expect(screen.queryByText('Saved')).not.toBeInTheDocument();

  // Retry must replay the SAME intended mutation, not the currently committed (empty) pair set.
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(bridge.saveSourceLinks).toHaveBeenCalledTimes(2));
  const calls = (bridge.saveSourceLinks as ReturnType<typeof vi.fn>).mock.calls;
  expect(calls[1]).toEqual(calls[0]);
  expect(calls[0]).toEqual(['dev-a', '1', [{ leftSourceId: 'src-a', rightSourceId: 'src-b' }], false]);
  // Only after a genuine round trip does the grouped pair appear.
  expect(await screen.findByRole('slider', { name: /Channel 1 and Channel 2 default balance/ })).toBeInTheDocument();
});

test('a replacement binding routes Link through confirmation and saves with replaceBinding true', async () => {
  const replacement = makeSettings('dev-a', '1', [sourceA, sourceB], { bindingStatus: 'replacement' });
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': replacement });
  render(<SourcesView bridge={bridge} device={deviceA} />);

  fireEvent.change(await screen.findByLabelText('Left source'), { target: { value: 'src-a' } });
  fireEvent.change(screen.getByLabelText('Right source'), { target: { value: 'src-b' } });
  fireEvent.click(screen.getByRole('button', { name: 'Link' }));

  expect(await screen.findByText('Replace the saved profile?')).toBeInTheDocument();
  expect(bridge.saveSourceLinks).not.toHaveBeenCalled();

  fireEvent.click(screen.getByRole('button', { name: 'Confirm save' }));
  await waitFor(() =>
    expect(bridge.saveSourceLinks).toHaveBeenCalledWith('dev-a', '1', [{ leftSourceId: 'src-a', rightSourceId: 'src-b' }], true),
  );
});

test('a failed replacement confirm resets consent so the next edit re-confirms', async () => {
  const replacement = makeSettings('dev-a', '1', [sourceA, sourceB], { bindingStatus: 'replacement' });
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': replacement });
  (bridge.saveSourceDefaults as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('write failed'));

  render(<SourcesView bridge={bridge} device={deviceA} />);
  const slider = await screen.findByRole('slider', { name: /Channel 1 default pan/ });
  fireEvent.change(slider, { target: { value: '-50' } });

  fireEvent.click(await screen.findByRole('button', { name: 'Confirm save' }));
  expect(await screen.findByText('failed')).toBeInTheDocument();

  // A later edit must ask again rather than silently reusing the previous consent.
  fireEvent.change(slider, { target: { value: '50' } });
  expect(await screen.findByText('Replace the saved profile?')).toBeInTheDocument();
  expect(bridge.saveSourceDefaults).toHaveBeenCalledTimes(1);
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

test('a link confirm that aborts because the host started clears consent and re-confirms later', async () => {
  const replacement = makeSettings('dev-a', '1', [sourceA, sourceB], { bindingStatus: 'replacement' });
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': replacement });
  const { rerender } = render(<SourcesView bridge={bridge} device={deviceA} />);

  fireEvent.change(await screen.findByLabelText('Left source'), { target: { value: 'src-a' } });
  fireEvent.change(screen.getByLabelText('Right source'), { target: { value: 'src-b' } });
  fireEvent.click(screen.getByRole('button', { name: 'Link' }));
  expect(await screen.findByText('Replace the saved profile?')).toBeInTheDocument();

  // The host starts between the banner and the confirm; the click must abort truthfully.
  rerender(<SourcesView bridge={bridge} device={deviceA} hostRunning />);
  fireEvent.click(screen.getByRole('button', { name: 'Confirm save' }));
  expect(await screen.findByText('Stop the host before replacing source settings.')).toBeInTheDocument();
  expect(bridge.saveSourceLinks).not.toHaveBeenCalled();

  // A later link edit must re-confirm rather than silently reuse the abandoned consent.
  rerender(<SourcesView bridge={bridge} device={deviceA} />);
  fireEvent.click(screen.getByRole('button', { name: 'Link' }));
  expect(await screen.findByText('Replace the saved profile?')).toBeInTheDocument();
  expect(bridge.saveSourceLinks).not.toHaveBeenCalled();
});

test('a channel with unresolved pair metadata is not offered an editable mono default pan', async () => {
  const dangling: SourceInfo = { ...mono('src-x', 0, 'Dangling channel'), stereoPair: 'missing-peer', stereoSide: 'left' };
  const bridge = bridgeWith({ 'dev-a': [dangling] }, { 'dev-a': makeSettings('dev-a', '1', [dangling]) });
  render(<SourcesView bridge={bridge} device={deviceA} />);

  expect(await screen.findByText(/incomplete stereo-link metadata/i)).toBeInTheDocument();
  expect(screen.queryByRole('slider', { name: /Dangling channel default pan/ })).not.toBeInTheDocument();
});

test('AdminRoot disables Start host while an unsaved source default draft exists', async () => {
  const iface: InterfaceInfo = { name: 'Ethernet', ipAddress: '192.168.1.10', prefix: 24 };
  const bridge = bridgeWith({ 'dev-a': [sourceA, sourceB] }, { 'dev-a': makeSettings('dev-a', '7', [sourceA, sourceB]) });
  bridge.listDevices = vi.fn(async () => [deviceA]);
  bridge.listInterfaces = vi.fn(async () => [iface]);
  bridge.hostDefaults = vi.fn(async () => ({
    interfaceName: null,
    interfaceIp: null,
    certificatePath: null,
    keyPath: null,
  }));
  const pending = deferred<SourceSettings>();
  (bridge.saveSourceDefaults as ReturnType<typeof vi.fn>).mockReturnValueOnce(pending.promise);

  render(<AdminRoot bridge={bridge} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Select device' }));
  fireEvent.change(screen.getByLabelText('Host network interface'), { target: { value: '192.168.1.10' } });
  fireEvent.change(screen.getByLabelText('Certificate path'), { target: { value: '/cert.pem' } });
  fireEvent.change(screen.getByLabelText('Key path'), { target: { value: '/key.pem' } });

  const start = () => screen.getByRole('button', { name: 'Start host' });
  await waitFor(() => expect(start()).toBeEnabled());

  // An unsaved draft blocks Start, with a truthful explanation.
  const slider = await screen.findByRole('slider', { name: /Channel 1 default pan/ });
  fireEvent.change(slider, { target: { value: '-50' } });
  await waitFor(() => expect(start()).toBeDisabled());
  expect(screen.getByText(/Save or reload your source default edits before starting/i)).toBeInTheDocument();

  // Once the edit round-trips to the committed store, Start is available again.
  const committed = applyDefaults(
    makeSettings('dev-a', '7', [sourceA, sourceB]),
    [
      { sourceId: 'src-a', pan: -0.5 },
      { sourceId: 'src-b', pan: 0 },
    ],
    '8',
  );
  pending.resolve(committed);
  await waitFor(() => expect(start()).toBeEnabled());
});
