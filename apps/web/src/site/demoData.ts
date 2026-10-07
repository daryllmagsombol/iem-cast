/**
 * In-memory sample-data adapter for the GitHub Pages Mix demo.
 *
 * This file is intentionally self-contained and offline:
 * - no @tauri-apps/api, no ReceiverController, no transport imports
 * - no USB, LAN discovery, WSS, ICE, WebRTC media, microphone, or playback
 * - reloading the page resets all state; nothing is persisted
 *
 * Values use the POC's declared attenuation range (-60..0 dB) as fictional
 * example data. Nothing here represents a live host or actual audio.
 */

import type { ChannelStripState } from '../ui/ChannelStrip';
import type { DiagnosticsSnapshot } from '../receiver-ports';

export interface DemoSource {
  sourceId: string;
  label: string;
  /** stereo-linked when set to its pair's id; mono is centered. */
  stereoPairId?: string;
}

export interface DemoState {
  running: boolean;
  channels: ChannelStripState[];
  masterDb: number;
  masterMuted: boolean;
}

export type DemoStatusCode = 'normal' | 'waiting' | 'unavailable' | 'stale';

/** Small fictional catalog: mono, stereo-linked, and one deliberately long name. */
export const DEMO_SOURCES: DemoSource[] = [
  { sourceId: 'aaaaaaaa-0000-4000-8000-000000000001', label: 'Lead vocal' },
  { sourceId: 'aaaaaaaa-0000-4000-8000-000000000002', label: 'Backing vocal' },
  { sourceId: 'aaaaaaaa-0000-4000-8000-000000000003', label: 'Keys L' },
  {
    sourceId: 'aaaaaaaa-0000-4000-8000-000000000004',
    label: 'Keys R',
    stereoPairId: 'aaaaaaaa-0000-4000-8000-000000000003',
  },
  { sourceId: 'aaaaaaaa-0000-4000-8000-000000000005', label: 'Kick' },
  { sourceId: 'aaaaaaaa-0000-4000-8000-000000000006', label: 'Bass DI' },
  { sourceId: 'aaaaaaaa-0000-4000-8000-000000000007', label: 'Guitar amp close mic' },
  {
    sourceId: 'aaaaaaaa-0000-4000-8000-000000000008',
    label: 'Overhead left — audience-facing stage-right position',
  },
  { sourceId: 'aaaaaaaa-0000-4000-8000-000000000009', label: 'Snare top' },
];

export function createInitialState(): DemoState {
  return {
    running: false,
    // Start muted; changing gain never starts the demo or unmutes.
    channels: DEMO_SOURCES.map((source) => ({
      sourceId: source.sourceId,
      label: source.label,
      requestedDb: -12,
      appliedDb: -12,
      muted: true,
    })),
    masterDb: -6,
    masterMuted: true,
  };
}

const STATUS_SEQUENCE: DemoStatusCode[] = ['normal', 'waiting', 'unavailable', 'stale'];

export function nextStatusCode(current: DemoStatusCode): DemoStatusCode {
  const index = STATUS_SEQUENCE.indexOf(current);
  return STATUS_SEQUENCE[(index + 1) % STATUS_SEQUENCE.length] as DemoStatusCode;
}

export const DEMO_STATUS_LABELS: Record<DemoStatusCode, string> = {
  normal: 'Normal sample',
  waiting: 'Waiting for sample',
  unavailable: 'Unavailable',
  stale: 'Stale (simulated)',
};

/** Fictional fixture used to demonstrate the diagnostics display states. */
export function sampleDiagnostics(status: DemoStatusCode, now: number): DiagnosticsSnapshot {
  switch (status) {
    case 'normal':
      return { network_rtt_ms: 38, buffer_delay_ms: 2, end_to_end: 'Not measured', last_updated: now };
    case 'waiting':
      return {
        network_rtt_ms: 'Unavailable',
        buffer_delay_ms: 'Waiting for sample',
        end_to_end: 'Not measured',
        last_updated: null,
      };
    case 'unavailable':
      return {
        network_rtt_ms: 'Unavailable',
        buffer_delay_ms: 'Unavailable',
        end_to_end: 'Not measured',
        last_updated: null,
      };
    case 'stale':
      // Stale values are shown as Unavailable, keeping the last-known update label.
      return {
        network_rtt_ms: 'Unavailable',
        buffer_delay_ms: 'Unavailable',
        end_to_end: 'Not measured',
        last_updated: now - 30_000,
      };
    default:
      return {
        network_rtt_ms: 'Unavailable',
        buffer_delay_ms: 'Unavailable',
        end_to_end: 'Not measured',
        last_updated: null,
      };
  }
}

/** Deterministic, bounded fictional meter fixtures (dBFS). Never claims real audio. */
export function sampleMeters(status: DemoStatusCode, tick: number): Record<string, number | undefined> {
  if (status === 'unavailable' || status === 'stale') return {};

  if (status === 'waiting') {
    // First samples are undecided: only the first source is present and lit.
    const first = DEMO_SOURCES[0];
    return first ? { [first.sourceId]: -30 } : {};
  }

  const meters: Record<string, number | undefined> = {};
  DEMO_SOURCES.forEach((source, index) => {
    const phase = (tick + index * 2) % 10;
    meters[source.sourceId] = -24 + Math.sin(phase) * 15;
  });
  return meters;
}
