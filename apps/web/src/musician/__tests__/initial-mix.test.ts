import { describe, expect, test } from 'vitest';

import type { CatalogSnapshot, CounterString, SourceInfo } from '../../protocol';
import { ProtocolParseError } from '../../protocol';
import {
  canonicalPan,
  initialMix,
  normalizeCatalog,
  normalizeMixSnapshot,
  normalizeSourceGain,
} from '../initial-mix';

/** Build the one-source catalog the plan's tests use. */
function catalogWith(defaultPan: number, overrides: Partial<SourceInfo> = {}): CatalogSnapshot {
  return {
    catalogRevision: '7' as CounterString,
    sources: [
      {
        sourceId: 'a',
        physicalIndex: 0,
        label: 'A',
        role: 'inputChannel',
        authorized: true,
        available: true,
        stereoPair: null,
        stereoSide: null,
        defaultPan,
        ...overrides,
      },
    ],
  };
}

describe('initialMix', () => {
  test('initial_mix_matches_for_both_phone_paths', () => {
    const mix = initialMix(catalogWith(0));
    expect(mix.sources).toEqual([{ sourceId: 'a', gainDb: -60, muted: true, pan: 0 }]);
    expect(mix.masterDb).toBe(-60);
    expect(mix.masterMuted).toBe(true);
    expect(mix.mixRevision).toBe('0');
    expect(mix.catalogRevision).toBe('7');
  });

  test('nonzero_default_pan_projects_into_initial_mix', () => {
    expect(initialMix(catalogWith(0.35)).sources[0].pan).toBe(0.35);
  });

  test('absent_catalog_default_pan_seeds_center', () => {
    // A legacy catalog source with no defaultPan must seed center, never undefined.
    const legacy = { ...catalogWith(0).sources[0] } as Partial<SourceInfo>;
    delete legacy.defaultPan;
    const mixed = initialMix({ catalogRevision: '7' as CounterString, sources: [legacy as SourceInfo] });
    expect(mixed.sources[0].pan).toBe(0);
  });

  test('excludes unavailable or unauthorized sources as a unit, never as mono fallback', () => {
    const catalog: CatalogSnapshot = {
      catalogRevision: '7' as CounterString,
      sources: [
        { sourceId: 'ok', physicalIndex: 0, label: 'Ok', role: 'inputChannel', authorized: true, available: true, stereoPair: null, stereoSide: null, defaultPan: 0 },
        { sourceId: 'unauth', physicalIndex: 1, label: 'Unauth', role: 'inputChannel', authorized: false, available: true, stereoPair: null, stereoSide: null, defaultPan: 0 },
        { sourceId: 'gone', physicalIndex: 2, label: 'Gone', role: 'inputChannel', authorized: true, available: false, stereoPair: null, stereoSide: null, defaultPan: 0 },
      ],
    };
    expect(initialMix(catalog).sources.map(s => s.sourceId)).toEqual(['ok']);
  });

  test('a pair is fail-closed: a missing/half/unavailable member excludes the whole pair', () => {
    const base = (overrides: Partial<SourceInfo>, sourceId: string): SourceInfo => ({
      sourceId,
      physicalIndex: 0,
      label: sourceId,
      role: 'inputChannel',
      authorized: true,
      available: true,
      stereoPair: null,
      stereoSide: null,
      defaultPan: 0,
      ...overrides,
    });
    // Left present, peer missing entirely.
    const half: CatalogSnapshot = {
      catalogRevision: '7' as CounterString,
      sources: [base({ stereoPair: 'right', stereoSide: 'left' }, 'left')],
    };
    expect(initialMix(half).sources).toEqual([]);

    // Reciprocal pair where the peer is unavailable.
    const closed: CatalogSnapshot = {
      catalogRevision: '7' as CounterString,
      sources: [
        base({ stereoPair: 'right', stereoSide: 'left' }, 'left'),
        base({ stereoPair: 'left', stereoSide: 'right', available: false }, 'right'),
      ],
    };
    expect(initialMix(closed).sources).toEqual([]);

    // Complete reciprocal pair seeds both members consistently.
    const paired: CatalogSnapshot = {
      catalogRevision: '7' as CounterString,
      sources: [
        base({ stereoPair: 'right', stereoSide: 'left', defaultPan: 0.2 }, 'left'),
        base({ stereoPair: 'left', stereoSide: 'right', defaultPan: 0.2 }, 'right'),
      ],
    };
    expect(initialMix(paired).sources).toEqual([
      { sourceId: 'left', gainDb: -60, muted: true, pan: 0.2 },
      { sourceId: 'right', gainDb: -60, muted: true, pan: 0.2 },
    ]);
  });

  test('a non-reciprocal one-sided marker excludes the claiming source and its peer', () => {
    const base = (overrides: Partial<SourceInfo>, sourceId: string): SourceInfo => ({
      sourceId,
      physicalIndex: 0,
      label: sourceId,
      role: 'inputChannel',
      authorized: true,
      available: true,
      stereoPair: null,
      stereoSide: null,
      defaultPan: 0,
      ...overrides,
    });
    // A claims B, but B claims nobody. Neither may seed as an independent mono source.
    const oneSided: CatalogSnapshot = {
      catalogRevision: '7' as CounterString,
      sources: [
        base({ stereoPair: 'b', stereoSide: 'left' }, 'a'),
        base({ stereoPair: null, stereoSide: null }, 'b'),
      ],
    };
    expect(initialMix(oneSided).sources).toEqual([]);
  });

  test('an unpaired masterLr keeps a fixed safe center setting', () => {
    const catalog: CatalogSnapshot = {
      catalogRevision: '7' as CounterString,
      sources: [
        { sourceId: 'm', physicalIndex: 0, label: 'Master', role: 'masterLr', authorized: true, available: true, stereoPair: null, stereoSide: null, defaultPan: 0.6 },
      ],
    };
    expect(initialMix(catalog).sources[0].pan).toBe(0);
  });
});

describe('normalizeSourceGain', () => {
  test('legacy_snapshot_without_pan_normalizes_to_center', () => {
    expect(normalizeSourceGain({ sourceId: 'a', gainDb: -3, muted: false }).pan).toBe(0);
  });

  test('explicit null or non-finite pan is rejected, never clamped to success', () => {
    expect(() => normalizeSourceGain({ sourceId: 'a', gainDb: -3, muted: false, pan: null })).toThrow(ProtocolParseError);
    expect(() => normalizeSourceGain({ sourceId: 'a', gainDb: -3, muted: false, pan: Number.NaN })).toThrow(ProtocolParseError);
    expect(() => normalizeSourceGain({ sourceId: 'a', gainDb: -3, muted: false, pan: Number.POSITIVE_INFINITY })).toThrow(ProtocolParseError);
  });

  test('finite out-of-range pan is clamped to the canonical range', () => {
    expect(normalizeSourceGain({ sourceId: 'a', gainDb: -3, muted: false, pan: -2 }).pan).toBe(-1);
    expect(normalizeSourceGain({ sourceId: 'a', gainDb: -3, muted: false, pan: 5 }).pan).toBe(1);
    expect(canonicalPan(0.4)).toBe(0.4);
  });
});

describe('normalizeMixSnapshot / normalizeCatalog', () => {
  test('normalizes every legacy source pan while preserving revisions verbatim', () => {
    const raw = normalizeMixSnapshot({
      catalogRevision: '7',
      mixRevision: '3',
      sources: [
        { sourceId: 'a', gainDb: -6, muted: false },
        { sourceId: 'b', gainDb: -6, muted: true, pan: 1.5 },
      ],
      masterDb: -6,
      masterMuted: false,
    });
    expect(raw.catalogRevision).toBe('7');
    expect(raw.mixRevision).toBe('3');
    expect(raw.sources.map(s => s.pan)).toEqual([0, 1]);
  });

  test('legacy catalog source without defaultPan/stereoSide normalizes to center/null', () => {
    const catalog = normalizeCatalog({
      catalogRevision: '7',
      sources: [
        { sourceId: 'a', physicalIndex: 0, label: 'A', role: 'inputChannel', authorized: true, available: true, stereoPair: null } as SourceInfo,
      ],
    });
    expect(catalog.sources[0].defaultPan).toBe(0);
    expect(catalog.sources[0].stereoSide).toBeNull();
  });
});
