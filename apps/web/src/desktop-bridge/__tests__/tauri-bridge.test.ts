/**
 * Adapter regression tests for the Tauri operator bridge.
 *
 * These pin the *production* adapter (`createTauriHostBridge`) against the exact serialized
 * shapes emitted by `apps/desktop/src-tauri/src/bridge.rs` / `commands.rs`. Only the external
 * `@tauri-apps/api` boundary is mocked; no UI/copy behavior is asserted here.
 */
import { beforeEach, describe, expect, test, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  isTauri: vi.fn(() => true),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: vi.fn(() => ({ label: 'operator' })),
}));

import { invoke, isTauri } from '@tauri-apps/api/core';
import { createTauriHostBridge, IpcInvokeError, isOperatorShell } from '../tauri-bridge';

const invokeMock = vi.mocked(invoke);
const isTauriMock = vi.mocked(isTauri);

/** Serialized `bridge::DeviceInfo` (camelCase). `sampleFormat` is Rust `Debug` of `Option`. */
const rawDevices = [
  {
    deviceId: 'dev-f32',
    name: 'Soundcraft Signature 22 MTK',
    isDefault: true,
    inputChannels: 22,
    sampleFormat: 'Some(F32)',
    sampleRateHz: 48000,
  },
  {
    deviceId: 'dev-i16',
    name: 'USB I16 interface',
    isDefault: false,
    inputChannels: 2,
    sampleFormat: 'Some(I16)',
    sampleRateHz: null,
  },
  {
    deviceId: 'dev-none',
    name: 'No reported format',
    isDefault: false,
    inputChannels: 2,
    sampleFormat: 'None',
    sampleRateHz: null,
  },
  {
    deviceId: 'dev-unknown',
    name: 'Unrepresentable format',
    isDefault: false,
    inputChannels: 2,
    sampleFormat: 'Some(I24)',
    sampleRateHz: null,
  },
];

/** Serialized `bridge::CatalogSnapshot` from `source_catalog` / `set_available_sources`. */
const rawCatalog = {
  catalogRevision: '34',
  sources: [
    {
      sourceId: '33333333-3333-4333-8333-333333333333',
      physicalIndex: 6,
      label: 'Lead vocal',
      available: true,
      availableToMusicians: true,
    },
    {
      sourceId: '44444444-4444-4444-8444-444444444444',
      physicalIndex: 22,
      label: 'Master L/R',
      available: true,
      availableToMusicians: false,
    },
    {
      sourceId: '55555555-5555-4555-8555-555555555555',
      physicalIndex: 7,
      label: 'Unplugged but authorized',
      available: false,
      availableToMusicians: true,
    },
  ],
};

beforeEach(() => {
  invokeMock.mockReset();
  isTauriMock.mockReset();
  isTauriMock.mockReturnValue(true);
});

describe('listDevices', () => {
  test('decodes the Rust Debug sampleFormat without inventing formats or bounds', async () => {
    invokeMock.mockResolvedValueOnce(rawDevices);
    const devices = await createTauriHostBridge().listDevices();

    expect(invokeMock).toHaveBeenCalledWith('list_devices', { windowLabel: 'operator' });
    expect(devices.map((d) => d.sampleFormats)).toEqual([['F32'], ['I16'], [], []]);
    expect(devices.map((d) => d.sampleRateHz)).toEqual([48000, null, null, null]);
    // The Rust bridge reports no buffer bounds; never fabricate a range.
    expect(devices.every((d) => d.bufferMinFrames === null && d.bufferMaxFrames === null)).toBe(true);
  });
});

describe('sourceCatalog', () => {
  test('unwraps CatalogSnapshot.sources exactly and normalizes availability/authorization', async () => {
    invokeMock.mockResolvedValueOnce(rawCatalog);
    const sources = await createTauriHostBridge().sourceCatalog();

    expect(invokeMock).toHaveBeenCalledWith('source_catalog', { windowLabel: 'operator' });
    expect(sources).toEqual([
      {
        sourceId: '33333333-3333-4333-8333-333333333333',
        physicalIndex: 6,
        label: 'Lead vocal',
        role: 'inputChannel',
        authorized: true,
        available: true,
        stereoPair: null,
      },
      {
        sourceId: '44444444-4444-4444-8444-444444444444',
        physicalIndex: 22,
        label: 'Master L/R',
        role: 'inputChannel',
        authorized: false,
        available: false,
        stereoPair: null,
      },
      {
        sourceId: '55555555-5555-4555-8555-555555555555',
        physicalIndex: 7,
        label: 'Unplugged but authorized',
        role: 'inputChannel',
        authorized: true,
        available: false,
        stereoPair: null,
      },
    ]);
  });

  test('does not silently substitute an empty list for a malformed response', async () => {
    invokeMock.mockResolvedValueOnce({ catalogRevision: '34' });
    await expect(createTauriHostBridge().sourceCatalog()).rejects.toThrow();
  });
});

describe('setAvailableSources / setSourceLabel', () => {
  test('maps the returned CatalogSnapshot through the same source adapter', async () => {
    invokeMock
      .mockResolvedValueOnce(rawCatalog)
      .mockResolvedValueOnce({ ...rawCatalog, catalogRevision: '35' });
    const bridge = createTauriHostBridge();

    const available = await bridge.setAvailableSources(['33333333-3333-4333-8333-333333333333']);
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'set_available_sources', {
      windowLabel: 'operator',
      ids: ['33333333-3333-4333-8333-333333333333'],
    });
    expect(available.catalogRevision).toBe('34');
    expect(available.sources[1]).toMatchObject({ authorized: false, available: false });

    const labelled = await bridge.setSourceLabel('44444444-4444-4444-8444-444444444444', 'Drums');
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'set_source_label', {
      windowLabel: 'operator',
      id: '44444444-4444-4444-8444-444444444444',
      label: 'Drums',
    });
    expect(labelled.catalogRevision).toBe('35');
  });
});

describe('startHost', () => {
  test('flattens the request to the Rust DTO and sends actual capture/interface/cert values', async () => {
    invokeMock
      .mockResolvedValueOnce({
        hostEpoch: '11111111-1111-4111-8111-111111111111',
        audioEpoch: '22222222-2222-4222-8222-222222222222',
        joinUrl: 'https://192.168.1.10/join',
      })
      .mockResolvedValueOnce(rawCatalog);

    const result = await createTauriHostBridge().startHost({
      capture: { deviceId: 'dev-f32', sampleRateHz: 48000, bufferFrames: 128 },
      interface: { name: 'en0', ipAddress: '192.168.1.10', prefix: 24 },
      certificatePath: '/etc/iem/cert.pem',
      keyPath: '/etc/iem/key.pem',
    });

    expect(invokeMock).toHaveBeenNthCalledWith(1, 'start_host', {
      windowLabel: 'operator',
      request: {
        deviceId: 'dev-f32',
        interfaceIp: '192.168.1.10',
        certificatePath: '/etc/iem/cert.pem',
        keyPath: '/etc/iem/key.pem',
      },
    });
    // No fabricated capture params leak over the wire.
    const requestArg = invokeMock.mock.calls[0][1] as { request: Record<string, unknown> };
    expect(Object.keys(requestArg.request).sort()).toEqual([
      'certificatePath',
      'deviceId',
      'interfaceIp',
      'keyPath',
    ]);
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'source_catalog', { windowLabel: 'operator' });
    expect(result).toEqual({
      hostEpoch: '11111111-1111-4111-8111-111111111111',
      audioEpoch: '22222222-2222-4222-8222-222222222222',
      joinUrl: 'https://192.168.1.10/join',
      catalog: {
        catalogRevision: '34',
        sources: [
          expect.objectContaining({ authorized: true, available: true }),
          expect.objectContaining({ authorized: false, available: false }),
          expect.objectContaining({ authorized: true, available: false }),
        ],
      },
    });
  });
});

describe('IPC errors', () => {
  test('preserves the host code and message instead of fabricating a success/empty result', async () => {
    invokeMock.mockRejectedValueOnce({
      code: 'WINDOW_FORBIDDEN',
      message: 'caller is not the operator window',
    });

    const error = await createTauriHostBridge()
      .listDevices()
      .then(() => null)
      .catch((e: unknown) => e);

    expect(error).toBeInstanceOf(IpcInvokeError);
    expect((error as IpcInvokeError).code).toBe('WINDOW_FORBIDDEN');
    expect((error as IpcInvokeError).message).toBe('caller is not the operator window');
  });

  test('passes through a non-IpcError failure unchanged', async () => {
    const boom = new Error('transport down');
    invokeMock.mockRejectedValueOnce(boom);
    await expect(createTauriHostBridge().listDevices()).rejects.toBe(boom);
  });
});

describe('isOperatorShell', () => {
  test('reflects isTauri and stays false when the probe throws', () => {
    isTauriMock.mockReturnValueOnce(true);
    expect(isOperatorShell()).toBe(true);
    isTauriMock.mockReturnValueOnce(false);
    expect(isOperatorShell()).toBe(false);
    isTauriMock.mockImplementationOnce(() => {
      throw new Error('no window');
    });
    expect(isOperatorShell()).toBe(false);
  });
});

describe('listOutputDevices', () => {
  test('calls list_output_devices with the window label and maps the DTO verbatim', async () => {
    const raw = [
      { id: 'out-default', name: 'MacBook Speakers', isDefault: true },
      { id: 'out-usb', name: 'USB Monitor Out', isDefault: false },
    ];
    invokeMock.mockResolvedValueOnce(raw);

    const devices = await createTauriHostBridge().listOutputDevices();

    expect(invokeMock).toHaveBeenCalledWith('list_output_devices', { windowLabel: 'operator' });
    expect(devices).toEqual(raw);
  });
});

describe('startMonitor', () => {
  test('sends the 0-based slot and the device id, preserving an explicit null', async () => {
    const bridge = createTauriHostBridge();

    invokeMock.mockResolvedValueOnce(undefined);
    await bridge.startMonitor(0, 'out-usb');
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'start_monitor', {
      windowLabel: 'operator',
      slot: 0,
      deviceId: 'out-usb',
    });

    // A `null` device id is the "system default output" request; never drop the key.
    invokeMock.mockResolvedValueOnce(undefined);
    await bridge.startMonitor(3, null);
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'start_monitor', {
      windowLabel: 'operator',
      slot: 3,
      deviceId: null,
    });
  });

  test('preserves a typed monitor error code', async () => {
    invokeMock.mockRejectedValueOnce({
      code: 'MONITOR_SLOT_INVALID',
      message: 'slot is out of range',
    });

    const error = await createTauriHostBridge()
      .startMonitor(9, null)
      .then(() => null)
      .catch((e: unknown) => e);

    expect(error).toBeInstanceOf(IpcInvokeError);
    expect((error as IpcInvokeError).code).toBe('MONITOR_SLOT_INVALID');
  });
});

describe('stopMonitor', () => {
  test('calls stop_monitor with only the window label', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await createTauriHostBridge().stopMonitor();
    expect(invokeMock).toHaveBeenCalledWith('stop_monitor', { windowLabel: 'operator' });
  });
});
