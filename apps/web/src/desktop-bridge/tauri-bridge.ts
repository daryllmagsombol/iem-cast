/**
 * Tauri-backed operator host bridge.
 *
 * Implements the frozen `HostBridge` contract over Tauri IPC. Only the bundled operator webview
 * may import this module; the LAN musician bundle must never reach it (musicians go through
 * `receiver-ports.ts`).
 *
 * The Rust commands take the caller's window label as their first argument, which the shell
 * verifies against the operator window. We read it from the current Tauri window rather than
 * hard-coding it, so a mislabelled window is rejected by the host instead of trusted here.
 */

import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

import type { HostBridge, PairingCredential } from './contracts';
import type {
  CatalogSnapshot,
  CounterString,
  DeviceInfo,
  InterfaceInfo,
  SampleFormat,
  SourceInfo,
  StartHostRequest,
  StartHostResult,
} from '../protocol';

/** Raw shapes returned by the Rust commands (camelCase). */
interface RawDeviceInfo {
  deviceId: string;
  name: string;
  isDefault: boolean;
  inputChannels: number;
  sampleFormat: string;
  sampleRateHz: number | null;
}

interface RawSourceInfo {
  sourceId: string;
  physicalIndex: number;
  label: string;
  available: boolean;
  availableToMusicians: boolean;
}

interface RawCatalogSnapshot {
  catalogRevision: string;
  sources: RawSourceInfo[];
}

interface RawStartHostResult {
  hostEpoch: string;
  audioEpoch: string;
  joinUrl: string;
}

function toSampleFormats(raw: string): SampleFormat[] {
  return raw === 'F32' || raw === 'I16' || raw === 'U16' ? [raw] : [];
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

function toSource(raw: RawSourceInfo): SourceInfo {
  return {
    sourceId: raw.sourceId,
    physicalIndex: raw.physicalIndex,
    label: raw.label,
    // The bridge reports availability only. Role/authorization are derived from the catalog:
    // every listed source is an operator-authorized input channel.
    role: 'inputChannel',
    authorized: true,
    available: raw.available,
    stereoPair: null,
  };
}

function toCatalog(raw: RawCatalogSnapshot): CatalogSnapshot {
  return {
    catalogRevision: raw.catalogRevision as CounterString,
    sources: raw.sources.map(toSource),
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
      const raw = await invoke<RawDeviceInfo[]>('list_devices', { windowLabel: label });
      return raw.map(toDevice);
    },

    async listInterfaces() {
      return invoke<InterfaceInfo[]>('list_interfaces', { windowLabel: label });
    },

    async startHost(req: StartHostRequest) {
      const raw = await invoke<RawStartHostResult>('start_host', {
        windowLabel: label,
        request: {
          deviceId: req.capture.deviceId,
          interfaceIp: req.interface.ipAddress,
          certificatePath: req.certificatePath,
          keyPath: req.keyPath,
        },
      });
      // `start_host` returns the epoch/URL; the operator catalog is fetched separately so the UI
      // never displays a catalog that was never actually built.
      const catalog = toCatalog(await invoke<RawCatalogSnapshot>('source_catalog', { windowLabel: label }));
      return {
        hostEpoch: raw.hostEpoch as StartHostResult['hostEpoch'],
        audioEpoch: raw.audioEpoch as StartHostResult['audioEpoch'],
        joinUrl: raw.joinUrl,
        catalog,
      } satisfies StartHostResult;
    },

    async stopHost() {
      await invoke<void>('stop_host', { windowLabel: label });
    },

    async sourceCatalog() {
      // The Rust `source_catalog` command returns a full `CatalogSnapshot`; the frozen bridge
      // contract exposes only the source list, so unwrap it here rather than reshaping the host.
      const raw = await invoke<RawCatalogSnapshot>('source_catalog', { windowLabel: label });
      return (raw.sources ?? []).map(toSource);
    },

    async setAvailableSources(ids: string[]) {
      const raw = await invoke<RawCatalogSnapshot>('set_available_sources', {
        windowLabel: label,
        ids,
      });
      return toCatalog(raw);
    },

    async setSourceLabel(id: string, labelText: string) {
      const raw = await invoke<RawCatalogSnapshot>('set_source_label', {
        windowLabel: label,
        id,
        label: labelText,
      });
      return toCatalog(raw);
    },

    async issuePairingCredential() {
      return invoke<PairingCredential>('issue_pairing_credential', { windowLabel: label });
    },
  };
}
