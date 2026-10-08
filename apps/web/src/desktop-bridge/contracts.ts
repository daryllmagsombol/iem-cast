/**
 * Frozen Task 1 desktop-bridge contract (Tauri **operator** IPC only).
 *
 * This file is the TypeScript mirror of the Tauri command surface. The LAN musician bundle must
 * never import it (nor `@tauri-apps/api`); musicians go through `receiver-ports.ts` instead.
 *
 * Wire field names are camelCase, matching the Rust contract.
 */

import type {
  CatalogSnapshot,
  DeviceInfo,
  InterfaceInfo,
  SourceInfo,
  StartHostRequest,
  StartHostResult,
} from '../protocol';

/** A fresh single-use pairing credential minted by the host `PairStore`. */
export interface PairingCredential {
  /** Join URL; the one-use fragment token is embedded here and must never be logged. */
  joinUrl: string;
  expiresInSeconds: number;
}

/** A local monitor output destination on the host machine. */
export interface OutputDeviceInfo {
  /** Stable output device id passed back to `startMonitor`. */
  id: string;
  name: string;
  isDefault: boolean;
}

/**
 * Operator-only host bridge. Implemented over Tauri `invoke`; command identifiers are snake_case
 * on the Rust side (`list_devices`, ..., `stop_monitor`).
 */
export interface HostBridge {
  listDevices(): Promise<DeviceInfo[]>;
  listInterfaces(): Promise<InterfaceInfo[]>;
  startHost(req: StartHostRequest): Promise<StartHostResult>;
  stopHost(): Promise<void>;
  /**
   * Read the real source catalog of the selected capture device. `deviceId` of `null` means no
   * device is selected and yields an explicit empty list. The catalog is read-only.
   */
  sourceCatalog(deviceId: string | null): Promise<SourceInfo[]>;
  setAvailableSources(ids: string[]): Promise<CatalogSnapshot>;
  setSourceLabel(id: string, label: string): Promise<CatalogSnapshot>;
  issuePairingCredential(): Promise<PairingCredential>;
  /** Enumerate the host machine's output devices for the local monitor. */
  listOutputDevices(): Promise<OutputDeviceInfo[]>;
  /**
   * Tap one active listener slot's mix onto a local output. `deviceId` of `null` requests the
   * system default output. Requires a running host.
   */
  startMonitor(slot: number, deviceId: string | null): Promise<void>;
  /** Stop the local monitor. Idempotent. */
  stopMonitor(): Promise<void>;
}
