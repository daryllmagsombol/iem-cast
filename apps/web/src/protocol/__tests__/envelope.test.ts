import { parseEnvelope, parseFrameCount } from '../index';
import mixPatchFixture from '../../../../../crates/host-core/fixtures/mix.patch.json';

test('parseEnvelope preserves wire type and decimal revision strings and stays serializable', () => {
  const parsed = parseEnvelope(mixPatchFixture);
  expect(parsed.type).toBe('mix.patch');
  expect(parsed.payload.baseRevision).toBe('120');           // wire string preserved
  expect(BigInt(parsed.payload.baseRevision)).toBe(120n);    // BigInt only for comparison
  expect(parsed.payload.sources[0].sourceId).toBe('33333333-3333-4333-8333-333333333333');
  expect(() => JSON.stringify(parsed)).not.toThrow();
  expect(JSON.stringify(parsed)).toContain('"120"');
});

test('parseFrameCount parses a u32 number and rejects larger values', () => {
  expect(parseFrameCount('240')).toBe(240);
  expect(() => parseFrameCount('4294967296')).toThrow(/u32/);
});
