/**
 * Shared pure default-mix builder and wire normalization (Task 5).
 *
 * This module is the single source of truth for:
 *
 * - `initialMix(catalog)`: the neutral template both phone paths seed from, so a new connection
 *   (and the arm bootstrap when no mix is accepted yet) cannot drift.
 * - `normalizeSourceGain` / `normalizeMixSnapshot` / `normalizeCatalog`: legacy wire payloads that
 *   omit `pan` / `defaultPan` / `stereoSide` are normalized **once** at the boundary, so the rest of
 *   the app can treat `SourceGain.pan` as always present and numeric.
 *
 * The module is pure: no ports, no DOM, no side effects. Pan is canonical in `[-1, 1]`; a finite
 * out-of-range value is clamped consistently with the backend, while `null`/non-finite/malformed
 * pan is rejected rather than silently clamped to success.
 */

import { ProtocolParseError } from '../protocol';
import type {
  CatalogSnapshot,
  CounterString,
  MixSnapshot,
  SourceGain,
  SourceInfo,
} from '../protocol';

/** Incoming source-gain shape: legacy payloads may omit `pan` or send an explicit `null`. */
export interface RawSourceGain {
  sourceId: string;
  gainDb: number;
  muted: boolean;
  pan?: number | null;
}

/** Clamp a finite pan to the canonical `[-1, 1]` range. */
export function canonicalPan(value: number): number {
  return Math.min(1, Math.max(-1, value));
}

/**
 * Normalize one incoming source gain.
 *
 * - **omitted** pan resolves to `0` (center) — legacy clients stay deterministic;
 * - `null`, a non-number, or a non-finite value is **rejected** (`ProtocolParseError`);
 * - a finite out-of-range value is clamped to `[-1, 1]`.
 */
export function normalizeSourceGain(g: RawSourceGain): SourceGain {
  let pan: number;
  if (g.pan === undefined) {
    pan = 0;
  } else if (g.pan === null || typeof g.pan !== 'number' || !Number.isFinite(g.pan)) {
    throw new ProtocolParseError(`source ${g.sourceId} pan must be a finite number`);
  } else {
    pan = canonicalPan(g.pan);
  }
  return { sourceId: g.sourceId, gainDb: g.gainDb, muted: g.muted, pan };
}

/** Accepted mix/master fields for a legacy snapshot before `pan` normalization. */
export interface RawMixSnapshot {
  catalogRevision: string;
  mixRevision: string;
  sources: RawSourceGain[];
  masterDb: number;
  masterMuted: boolean;
}

/** Normalize every source in an incoming mix snapshot, preserving revisions verbatim. */
export function normalizeMixSnapshot(raw: RawMixSnapshot): MixSnapshot {
  return {
    catalogRevision: raw.catalogRevision as CounterString,
    mixRevision: raw.mixRevision as CounterString,
    sources: raw.sources.map(normalizeSourceGain),
    masterDb: raw.masterDb,
    masterMuted: raw.masterMuted,
  };
}

/** Tolerant catalog default pan: an absent/invalid value is `0` (center), never `undefined`. */
function normalizeDefaultPan(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? canonicalPan(value) : 0;
}

/** Normalize one catalog source: legacy payloads omit `defaultPan`/`stereoSide`. */
export function normalizeSourceInfo(raw: SourceInfo): SourceInfo {
  return {
    sourceId: raw.sourceId,
    physicalIndex: raw.physicalIndex,
    label: raw.label,
    role: raw.role === 'masterLr' ? 'masterLr' : 'inputChannel',
    authorized: raw.authorized === true,
    available: raw.available === true,
    stereoPair: typeof raw.stereoPair === 'string' ? raw.stereoPair : null,
    stereoSide: raw.stereoSide === 'left' || raw.stereoSide === 'right' ? raw.stereoSide : null,
    defaultPan: normalizeDefaultPan(raw.defaultPan),
  };
}

/** Normalize a catalog snapshot's sources at the incoming boundary. */
export function normalizeCatalog(raw: {
  catalogRevision: string;
  sources: SourceInfo[];
}): CatalogSnapshot {
  return {
    catalogRevision: raw.catalogRevision as CounterString,
    sources: raw.sources.map(normalizeSourceInfo),
  };
}

/**
 * The neutral template every phone path seeds from when no personal mix exists yet.
 *
 * Sources are `gain -60`, `muted: true`, and `pan` from the catalog default (a mono `InputChannel`)
 * or the shared pair default; an **unpaired `MasterLr`** keeps its fixed parity split and exposes no
 * pan control, so it is given a safe `0` setting. A stereo pair is included as a **unit**: if either
 * member is missing, unavailable, or unauthorized the pair is excluded entirely rather than silently
 * degraded into a mono source. Master is `-60`, muted; the template revision is `0`.
 */
export function initialMix(catalog: CatalogSnapshot): MixSnapshot {
  const byId = new Map(catalog.sources.map(source => [source.sourceId, source]));
  const usable = (source: SourceInfo | undefined): source is SourceInfo =>
    !!source && source.available && source.authorized;

  // A link is valid only when it is **reciprocal**: each member names the other. Collect every id
  // touched by a one-sided declaration (a source that names a peer that does not name it back) so
  // such a claim can never seed a lone mono source or an independent half of a pair.
  const nonReciprocal = new Set<string>();
  for (const source of catalog.sources) {
    const peerId = source.stereoPair;
    if (!peerId) continue;
    const peer = byId.get(peerId);
    if (!peer || peer.stereoPair !== source.sourceId) {
      nonReciprocal.add(source.sourceId);
      if (peer) nonReciprocal.add(peer.sourceId);
    }
  }

  const sources: SourceGain[] = catalog.sources
    .filter(source => {
      if (!usable(source)) return false;
      // Any source involved in a non-reciprocal declaration is excluded as a unit.
      if (nonReciprocal.has(source.sourceId)) return false;
      // A linked member is valid only when its reciprocal peer is also present and usable.
      if (source.stereoPair) return usable(byId.get(source.stereoPair));
      return true;
    })
    .map(source => ({
      sourceId: source.sourceId,
      gainDb: -60,
      muted: true,
      // An unpaired MasterLr is fixed-side with no pan control: keep a safe center setting.
      // A legacy catalog that omits `defaultPan` normalizes to center rather than `undefined`.
      pan: !source.stereoPair && source.role === 'masterLr'
        ? 0
        : normalizeDefaultPan(source.defaultPan),
    }));

  return {
    catalogRevision: catalog.catalogRevision,
    mixRevision: '0' as CounterString,
    sources,
    masterDb: -60,
    masterMuted: true,
  };
}
