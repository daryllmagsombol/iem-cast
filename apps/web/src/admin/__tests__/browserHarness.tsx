// Browser-only test mounts. Never imported by a product entrypoint.
import { createRoot } from 'react-dom/client';
import { AdminRoot } from '../AdminRoot';
import { MusicianRoot } from '../../musician/MusicianRoot';
import type { HostBridge } from '../../desktop-bridge/contracts';
import type { ReceiverController, ReceiverSnapshot } from '../../receiver-ports';
import type { CounterString } from '../../protocol';

function container() { document.getElementById('root')?.remove(); const node = document.createElement('div'); node.id = 'root'; document.body.append(node); return node; }
export function mountOperator() {
  const calls: string[] = [];
  const unavailable = async () => { calls.push('FORBIDDEN'); throw new Error('Unsupported operation invoked'); };
  const bridge: HostBridge = { listDevices: async () => { calls.push('devices'); return [{ deviceId: 'fixture', name: 'Browser-test USB fixture', isDefault: false, inputChannels: 4, sampleFormats: ['F32'], sampleRateHz: 48000, bufferMinFrames: null, bufferMaxFrames: null }]; }, listInterfaces: async () => { calls.push('interfaces'); return [{ name: 'Fixture Ethernet', ipAddress: '192.0.2.10', prefix: 24 }]; }, startHost: unavailable, stopHost: unavailable, sourceCatalog: unavailable, setAvailableSources: unavailable, setSourceLabel: unavailable, issuePairingCredential: unavailable };
  createRoot(container()).render(<AdminRoot bridge={bridge} />);
  return calls;
}
export function mountReceiver() {
  const revision = '1' as CounterString;
  let snapshot: ReceiverSnapshot = { phase: 'ready-muted', catalog: { catalogRevision: revision, sources: [{ sourceId: 'fixture-source', physicalIndex: 0, label: 'A deliberately long browser-test vocal source name', role: 'inputChannel', available: true, authorized: true, stereoPair: null }] }, requested_mix: { catalogRevision: revision, mixRevision: revision, masterDb: -18, masterMuted: true, sources: [{ sourceId: 'fixture-source', gainDb: -12, muted: true }] }, accepted_mix: null, applied_mix: null, master_local_muted: true, error: null, diagnostics: { network_rtt_ms: 'Unavailable', buffer_delay_ms: 'Waiting for sample', end_to_end: 'Not measured', last_updated: null } };
  let notify: (s: ReceiverSnapshot) => void = () => {};
  const calls: string[] = [];
  const controller: ReceiverController = { subscribe: h => { notify = h; return () => {}; }, getSnapshot: () => snapshot, connect: async () => { calls.push('prepare'); }, arm: async () => { calls.push('arm'); snapshot = { ...snapshot, phase: 'armed' }; notify(snapshot); }, requestMix: async patch => { calls.push('mix'); return { acceptedRevision: '2' as CounterString, catalogRevision: revision, canonicalSettings: { ...patch, mixRevision: '2' as CounterString } }; }, personalMasterMute: muted => { calls.push('mute'); snapshot = { ...snapshot, master_local_muted: muted }; notify(snapshot); }, stop: () => { calls.push('stop'); snapshot = { ...snapshot, phase: 'ready-muted', master_local_muted: true }; notify(snapshot); }, disconnect: () => {} };
  createRoot(container()).render(<MusicianRoot controller={controller} />);
  return calls;
}
