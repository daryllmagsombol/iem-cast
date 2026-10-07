/**
 * Shared TypeScript fixtures.
 *
 * The JSON files under `crates/host-core/fixtures/` are the single source of truth for both the
 * Rust and TypeScript tests. Those files are **wire** payloads (camelCase), so this module types
 * them with the wire DTOs and parses the one that is an envelope. Domain (snake_case) view types
 * are produced by the receiver controller, not here.
 */

import mixPatchJson from '../../../crates/host-core/fixtures/mix.patch.json';
import catalogSnapshotJson from '../../../crates/host-core/fixtures/catalog.snapshot.json';
import mixAppliedJson from '../../../crates/host-core/fixtures/mix.applied.json';
import armJson from '../../../crates/host-core/fixtures/arm.json';
import { parseEnvelope } from './protocol';
import type {
  CatalogSnapshot,
  EnvelopeV1,
  ListenArm,
  MixApplied,
  MixPatch,
} from './protocol';

/** Raw `mix.patch.json` wire envelope, exactly as stored. */
export const mixPatchWire = mixPatchJson;

/** Raw `catalog.snapshot.json` wire payload. */
export const catalogSnapshotWire = catalogSnapshotJson;

/** Raw `mix.applied.json` wire payload. */
export const mixAppliedWire = mixAppliedJson;

/** Raw `arm.json` wire payload. */
export const armWire = armJson;

/** Parsed and validated `mix.patch` envelope (counters branded, strings preserved). */
export const mixPatch = parseEnvelope<MixPatch>(mixPatchJson);

/** `catalog.snapshot` wire payload fixture. */
export const catalogSnapshot = catalogSnapshotJson as unknown as CatalogSnapshot;

/** `mix.applied` wire payload fixture. */
export const mixApplied = mixAppliedJson as unknown as MixApplied;

/** `listen.arm` wire payload fixture. */
export const armFixture = armJson as unknown as ListenArm;

/** The parsed `mix.patch` fixture, re-exported for ergonomic typing. */
export type MixPatchEnvelope = EnvelopeV1<MixPatch>;
