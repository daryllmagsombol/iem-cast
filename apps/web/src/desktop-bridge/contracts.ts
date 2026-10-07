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

/**
 * Operator-only host bridge. Implemented over Tauri `invoke`; command identifiers are snake_case
 * on the Rust side (`list_devices`, ..., `issue_pairing_credential`).
 */
export interface HostBridge {
  listDevices(): Promise<DeviceInfo[]>;
  listInterfaces(): Promise<InterfaceInfo[]>;
  startHost(req: StartHostRequest): Promise<StartHostResult>;
  stopHost(): Promise<void>;
  sourceCatalog(): Promise<SourceInfo[]>;
  setAvailableSources(ids: string[]): Promise<CatalogSnapshot>;
  setSourceLabel(id: string, label: string): Promise<CatalogSnapshot>;
  issuePairingCredential(): Promise<PairingCredential>;
}
