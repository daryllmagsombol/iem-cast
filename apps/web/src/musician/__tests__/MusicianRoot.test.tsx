import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { MusicianRoot } from '../MusicianRoot';
import type { ReceiverController, ReceiverSnapshot } from '../../receiver-ports';

function mockController(snapshot: ReceiverSnapshot): ReceiverController {
  return {
    subscribe: () => () => {},
    getSnapshot: () => snapshot,
    connect: async () => {},
    requestMix: async () => ({ acceptedRevision: '1', catalogRevision: '1', canonicalSettings: {} }) as never,
    arm: async () => {},
    personalMasterMute: () => {},
    stop: () => {},
    disconnect: () => {},
  };
}

test('unpaired_root_shows_join_gate_and_never_arms_implicitly', () => {
  render(
    <MusicianRoot
      controller={mockController({
        phase: 'unpaired',
        catalog: { catalogRevision: '0' as never, sources: [] },
        requested_mix: null,
        accepted_mix: null,
        applied_mix: null,
        master_local_muted: true,
        diagnostics: {
          network_rtt_ms: 'Unavailable',
          buffer_delay_ms: 'Unavailable',
          end_to_end: 'Not measured',
          last_updated: null,
        },
        error: null,
      })}
    />,
  );
  expect(screen.getByText(/waiting for pairing/i)).toBeInTheDocument();
  expect(screen.queryByText(/^listening$/i)).not.toBeInTheDocument();
});
