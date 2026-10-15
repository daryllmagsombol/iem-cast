/**
 * Tauri-backed operator host bridge.
 *
 * Implements the frozen `HostBridge` contract over Tauri IPC. Only the bundled operator webview
 * may import this module; the LAN musician bundle must never reach it (musicians go through
 * `receiver-ports.ts`).
 *
 * The legacy Rust commands take the caller's window label as their first argument, which the shell
 * verifies against the operator window. We read it from the current Tauri window rather than
 * hard-coding it, so a mislabelled window is rejected by the host instead of trusted here.
 *
 * The new source-settings commands (`read_source_settings`/`save_source_defaults`/
 * `save_source_links`) instead authorize the **actual injected caller window** natively and take no
 * frontend `windowLabel`, so no identity is passed from here for them.
 */

import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

import type { HostBridge, HostDefaults, OutputDeviceInfo, PairingCredential } from './contracts';
import type {
  BindingStatus,
  CatalogSnapshot,
  CounterString,
  DeviceInfo,
  InterfaceInfo,
  SampleFormat,
  SourceInfo,
  SourceLinkPair,
  SourcePanDefault,
  SourceSettings,
  StartHostRequest,
  StartHostResult,
  StereoSide,
} from '../protocol';

/** Raw shapes returned by the Rust commands (camelCase). */
interface RawDeviceInfo {
  deviceId: string;
  name: string;
  isDefault: boolean;
  inputChannels: number;
  /**
   * Rust `Debug` rendering of `Option<SampleFormat>` (e.g. `"Some(F32)"`, `"None"`), because the
   * command formats `format!("{:?}", sample_formats.first())`.
   */
  sampleFormat: string;
  sampleRateHz: number | null;
}

interface RawSourceInfo {
  sourceId: string;
  physicalIndex: number;
  label: string;
  /** Configured role (`inputChannel`/`masterLr`); legacy payloads may omit it. */
  role?: string;
  /** Physical presence reported by the capture backend. */
  available: boolean;
  /** Operator cast flag: whether the source is published to musicians. */
  availableToMusicians: boolean;
  /** Configured reciprocal stereo peer, when linked (legacy payloads omit it). */
  stereoPair?: string | null;
  /** Configured side (`left`/`right`), when linked (legacy payloads omit it). */
  stereoSide?: string | null;
  /** Configured default pan (legacy payloads omit it → center). */
  defaultPan?: number;
}

interface RawCatalogSnapshot {
  catalogRevision: string;
  sources: RawSourceInfo[];
}

interface RawSourceSettings {
  deviceId: string;
  inputChannels: number;
  settingsRevision: string;
  bindingStatus: string;
  durabilityUncertain: boolean;
  catalog: RawCatalogSnapshot;
}

interface RawStartHostResult {
  hostEpoch: string;
  audioEpoch: string;
  joinUrl: string;
}

/**
 * An IPC failure carrying the host's stable code and human-readable message. Wrapping preserves
 * both instead of collapsing a typed host error into a bare `Error` or a missing response.
 */
export class IpcInvokeError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = 'IpcInvokeError';
    this.code = code;
  }
}

function isIpcErrorShape(value: unknown): value is { code: string; message: string } {
  if (typeof value !== 'object' || value === null) return false;
  const candidate = value as { code?: unknown; message?: unknown };
  return typeof candidate.code === 'string' && typeof candidate.message === 'string';
}

/** Invoke a command, preserving typed host errors and rethrowing everything else unchanged. */
async function callInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    if (isIpcErrorShape(error)) {
      throw new IpcInvokeError(error.code, error.message);
    }
    throw error;
  }
}

const KNOWN_SAMPLE_FORMATS: ReadonlySet<string> = new Set<SampleFormat>(['F32', 'I16', 'U16']);

/**
 * Decode the Rust `Debug` `sampleFormat` string into at most one contract sample format.
 *
 * The host sends `Some(F32)`/`None` (Rust `Debug` of `Option<SampleFormat>`), not the wire enum
 * name. Only a known enum variant — bare or wrapped in `Some(...)` — is accepted; anything else
 * (`None`, `Some(I24)`, an unknown variant) yields no formats rather than an invented one.
 */
function toSampleFormats(raw: string): SampleFormat[] {
  const inner = raw.startsWith('Some(') && raw.endsWith(')') ? raw.slice('Some('.length, -1) : raw;
  return KNOWN_SAMPLE_FORMATS.has(inner) ? [inner as SampleFormat] : [];
}

function toDevice(raw: RawDeviceInfo): DeviceInfo {
  return {
    deviceId: raw.deviceId,
    name: raw.name,
    isDefault: raw.isDefault,
    inputChannels: raw.inputChannels,
    sampleFormats: toSampleFormats(raw.sampleFormat),
    sampleRateHz: raw.sampleRateHz,
    // The bridge does not report buffer bounds; unknown is `null`, never a fabricated range.
    bufferMinFrames: null,
    bufferMaxFrames: null,
  };
}

/**
 * Map the bridge source onto the full contract source.
 *
 * The Rust bridge reports physical presence (`available`) and the operator cast flag
 * (`availableToMusicians`). Authorization is the explicit cast flag only (conservative — never
 * assumed from physical presence); `available` reflects the physical flag AND the cast flag.
 *
 * Configured metadata (`role`, `stereoPair`, `stereoSide`, `defaultPan`) is carried **verbatim**
 * from the committed/prepared projection, so a real map is never reported as hardcoded mono. Only
 * when a legacy payload omits a field do we fall back to the additive contract default
 * (`inputChannel` / `null` / `null` / `0`) — never a fabricated pairing or side.
 */
function toSource(raw: RawSourceInfo): SourceInfo {
  const castEnabled = raw.availableToMusicians === true;
  const role = raw.role === 'masterLr' ? 'masterLr' : 'inputChannel';
  const stereoSide: StereoSide | null =
    raw.stereoSide === 'left' || raw.stereoSide === 'right' ? raw.stereoSide : null;
  return {
    sourceId: raw.sourceId,
    physicalIndex: raw.physicalIndex,
    label: raw.label,
    role,
    authorized: castEnabled,
    available: raw.available === true && castEnabled,
    stereoPair: typeof raw.stereoPair === 'string' ? raw.stereoPair : null,
    stereoSide,
    defaultPan: typeof raw.defaultPan === 'number' && Number.isFinite(raw.defaultPan)
      ? Math.min(1, Math.max(-1, raw.defaultPan))
      : 0,
  };
}

function toCatalog(raw: RawCatalogSnapshot): CatalogSnapshot {
  // Unwrap the Rust `CatalogSnapshot.sources` exactly. A response without a real array is a host
  // contract violation, not an empty catalog: never substitute `[]`.
  if (typeof raw !== 'object' || raw === null || !Array.isArray(raw.sources)) {
    throw new Error('source catalog response is missing a sources array');
  }
  return {
    catalogRevision: raw.catalogRevision as CounterString,
    sources: raw.sources.map(toSource),
  };
}

/** Map the host binding status onto the frozen union; an unknown value is a contract violation. */
function toBindingStatus(raw: string): BindingStatus {
  if (raw === 'saved' || raw === 'new' || raw === 'replacement' || raw === 'legacy') return raw;
  throw new Error(`unknown bindingStatus from host: ${raw}`);
}

/**
 * Decode the host `SourceSettings` verbatim into the frozen contract, preserving the truthful
 * `bindingStatus` and `durabilityUncertain` flags rather than inferring an ordinary save.
 */
function toSourceSettings(raw: RawSourceSettings): SourceSettings {
  return {
    deviceId: raw.deviceId,
    inputChannels: raw.inputChannels,
    settingsRevision: raw.settingsRevision as CounterString,
    bindingStatus: toBindingStatus(raw.bindingStatus),
    durabilityUncertain: raw.durabilityUncertain === true,
    catalog: toCatalog(raw.catalog),
  };
}

/** Whether this page is running inside the Tauri operator shell. */
export function isOperatorShell(): boolean {
  try {
    return isTauri();
  } catch {
    return false;
  }
}

/** Create a `HostBridge` backed by Tauri IPC. */
export function createTauriHostBridge(): HostBridge {
  const label = getCurrentWindow().label;

  return {
    async listDevices() {
      const raw = await callInvoke<RawDeviceInfo[]>('list_devices', { windowLabel: label });
      return raw.map(toDevice);
    },

    async listInterfaces() {
      return callInvoke<InterfaceInfo[]>('list_interfaces', { windowLabel: label });
    },

    async hostDefaults() {
      // The backend discovers these; the frontend never hard-codes them. `null` fields are passed
      // through unchanged so the UI leaves those inputs blank.
      return callInvoke<HostDefaults>('host_defaults', { windowLabel: label });
    },

    async startHost(req: StartHostRequest) {
      const raw = await callInvoke<RawStartHostResult>('start_host', {
        windowLabel: label,
        // Flatten the contract request onto the Rust `StartHostRequest` DTO. The bridge derives its
        // own rate/buffer, so no capture parameters are fabricated or sent.
        request: {
          deviceId: req.capture.deviceId,
          interfaceIp: req.interface.ipAddress,
          certificatePath: req.certificatePath,
          keyPath: req.keyPath,
        },
      });
      // `start_host` returns the epoch/URL; the operator catalog is fetched separately so the UI
      // never displays a catalog that was never actually built. It is fetched for the device the
      // host was started against.
      const catalog = toCatalog(
        await callInvoke<RawCatalogSnapshot>('source_catalog', {
          windowLabel: label,
          deviceId: req.capture.deviceId,
        }),
      );
      return {
        hostEpoch: raw.hostEpoch as StartHostResult['hostEpoch'],
        audioEpoch: raw.audioEpoch as StartHostResult['audioEpoch'],
        joinUrl: raw.joinUrl,
        catalog,
      } satisfies StartHostResult;
    },

    async stopHost() {
      await callInvoke<void>('stop_host', { windowLabel: label });
    },

    async sourceCatalog(deviceId: string | null) {
      // The Rust `source_catalog` command returns a full `CatalogSnapshot`; the frozen bridge
      // contract exposes only the source list, so unwrap it here rather than reshaping the host.
      // `deviceId: null` is sent explicitly so the host can distinguish "no selection" from a
      // malformed call, matching how `start_monitor` treats its optional device.
      const raw = await callInvoke<RawCatalogSnapshot>('source_catalog', {
        windowLabel: label,
        deviceId,
      });
      return toCatalog(raw).sources;
    },

    async setAvailableSources(ids: string[]) {
      const raw = await callInvoke<RawCatalogSnapshot>('set_available_sources', {
        windowLabel: label,
        ids,
      });
      return toCatalog(raw);
    },

    async setSourceLabel(id: string, labelText: string) {
      const raw = await callInvoke<RawCatalogSnapshot>('set_source_label', {
        windowLabel: label,
        id,
        label: labelText,
      });
      return toCatalog(raw);
    },

    async readSourceSettings(deviceId: string) {
      // The new commands authorize the ACTUAL injected caller window natively, so they take no
      // frontend `windowLabel` argument: sending one would be identity the host must not trust.
      const raw = await callInvoke<RawSourceSettings>('read_source_settings', { deviceId });
      return toSourceSettings(raw);
    },

    async saveSourceDefaults(
      deviceId: string,
      baseSettingsRevision: string,
      sourceDefaults: SourcePanDefault[],
      replaceBinding = false,
    ) {
      const raw = await callInvoke<RawSourceSettings>('save_source_defaults', {
        deviceId,
        baseSettingsRevision,
        sourceDefaults,
        // Explicit false by default; only an explicit `true` asks the host to replace a binding.
        replaceBinding,
      });
      return toSourceSettings(raw);
    },

    async saveSourceLinks(
      deviceId: string,
      baseSettingsRevision: string,
      pairs: SourceLinkPair[],
      replaceBinding = false,
    ) {
      const raw = await callInvoke<RawSourceSettings>('save_source_links', {
        deviceId,
        baseSettingsRevision,
        pairs,
        replaceBinding,
      });
      return toSourceSettings(raw);
    },

    async issuePairingCredential() {
      return callInvoke<PairingCredential>('issue_pairing_credential', { windowLabel: label });
    },

    async listOutputDevices() {
      const raw = await callInvoke<OutputDeviceInfo[]>('list_output_devices', {
        windowLabel: label,
      });
      return raw;
    },

    async startMonitor(slot: number, deviceId: string | null) {
      // The Rust command takes `slot` (u32) and `device_id: Option<String>`. `null` is the system
      // default output request; the key is always sent so omitting it can never be misread.
      await callInvoke<void>('start_monitor', { windowLabel: label, slot, deviceId });
    },

    async stopMonitor() {
      await callInvoke<void>('stop_monitor', { windowLabel: label });
    },
  };
}
