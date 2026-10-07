import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { LatencyDiagnostics } from '../ui/LatencyDiagnostics';
import { Notice } from '../ui/Notice';
import { StatusBadge } from '../ui/StatusBadge';
import { Toggle } from '../ui/Toggle';
import { DigitalMeter } from '../ui/DigitalMeter';
import { GainControl } from '../ui/GainControl';
import {
  DEMO_SOURCES,
  DEMO_STATUS_LABELS,
  createInitialState,
  nextStatusCode,
  sampleDiagnostics,
  sampleMeters,
  type DemoState,
  type DemoStatusCode,
} from './demoData';
import type { DiagnosticsSnapshot } from '../receiver-ports';

const SIMULATED_NOTICE = 'SIMULATED \u00b7 Silent demo \u00b7 No host connection';

export interface MixDemoProps {
  /** Injectable clock for tests; defaults to Date.now. */
  now?: () => number;
}

export function MixDemo({ now = Date.now }: MixDemoProps) {
  const [state, setState] = useState<DemoState>(() => createInitialState());
  const [status, setStatus] = useState<DemoStatusCode>('normal');
  const [tick, setTick] = useState(0);
  const [diagnostics, setDiagnostics] = useState<DiagnosticsSnapshot>(() =>
    sampleDiagnostics('normal', now()),
  );
  const nowRef = useRef(now);
  nowRef.current = now;

  // Meters: at most 10 Hz while running.
  useEffect(() => {
    if (!state.running) return;
    const id = window.setInterval(() => setTick((value) => value + 1), 100);
    return () => window.clearInterval(id);
  }, [state.running]);

  // Diagnostics: exactly 1 Hz, independent of the meter rate.
  useEffect(() => {
    if (!state.running) return;
    const id = window.setInterval(() => {
      setDiagnostics(sampleDiagnostics(status, nowRef.current()));
    }, 1000);
    return () => window.clearInterval(id);
  }, [state.running, status]);

  const meters = useMemo(
    () => (state.running ? sampleMeters(status, tick) : {}),
    [state.running, status, tick],
  );

  const startDemo = useCallback(() => {
    setState((previous) => ({ ...previous, running: true }));
    setDiagnostics(sampleDiagnostics(status, nowRef.current()));
  }, [status]);

  const stopDemo = useCallback(() => {
    // Immediate local stop; returns to the muted demo gate and stops fixtures.
    setState(() => createInitialState());
    setTick(0);
  }, []);

  const reset = useCallback(() => {
    // Reset returns all mute gates to muted without starting automatically.
    setState((previous) => ({
      ...previous,
      channels: previous.channels.map((channel) => ({ ...channel, muted: true })),
      masterMuted: true,
    }));
  }, []);

  const setChannelGain = useCallback((sourceId: string, db: number) => {
    setState((previous) => ({
      ...previous,
      channels: previous.channels.map((channel) =>
        channel.sourceId === sourceId
          ? { ...channel, requestedDb: db, appliedDb: db }
          : channel,
      ),
    }));
  }, []);

  const setChannelMute = useCallback(
    (sourceId: string, muted: boolean) => {
      // Unmute requires an explicit action while the demo is running.
      if (!muted && !state.running) return;
      setState((previous) => ({
        ...previous,
        channels: previous.channels.map((channel) =>
          channel.sourceId === sourceId ? { ...channel, muted } : channel,
        ),
      }));
    },
    [state.running],
  );

  const linksById = useMemo(() => {
    const map = new Map<string, string>();
    for (const source of DEMO_SOURCES) {
      if (source.stereoPairId) {
        const pair = DEMO_SOURCES.find((candidate) => candidate.sourceId === source.stereoPairId);
        map.set(source.sourceId, `Linked stereo pair with ${pair?.label ?? 'paired channel'}`);
      }
    }
    return map;
  }, []);

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 px-3 py-6 sm:px-4">
      <p className="rounded-panel border border-warning bg-surface p-3 text-label text-warning" role="note">
        {SIMULATED_NOTICE}
      </p>

      <header className="flex flex-col gap-2">
        <h1 className="text-title text-text">Mix demo</h1>
        <p className="text-body text-text-secondary">
          Explore personal mix controls with fictional sample data. Nothing here connects to a host,
          plays audio, or represents a real performance.
        </p>
        <StatusBadge tone={state.running ? 'warning' : 'neutral'}>
          {state.running ? 'Demo running \u00b7 No audio' : 'Demo stopped'}
        </StatusBadge>
      </header>

      <div className="flex flex-wrap items-center gap-3">
        <Button variant="primary" onClick={startDemo} disabled={state.running}>
          Start demo
        </Button>
        <Button variant="destructive" onClick={stopDemo} disabled={!state.running}>
          Stop demo
        </Button>
        <Button variant="secondary" onClick={reset}>
          Reset mutes
        </Button>
      </div>

      <div className="flex flex-col gap-2 rounded-panel border border-boundary p-4">
        <label htmlFor="demo-sample-state" className="text-label text-text">
          Sample state
        </label>
        <p className="text-caption text-text-secondary">
          Choose a fixture to preview normal, waiting, unavailable, and stale readouts.
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <select
            id="demo-sample-state"
            value={status}
            onChange={(event) => {
              const next = event.target.value as DemoStatusCode;
              setStatus(next);
              setDiagnostics(sampleDiagnostics(next, nowRef.current()));
            }}
            className="min-h-target-compact rounded-control border border-boundary bg-surface-raised px-2 text-body text-text"
          >
            {(Object.keys(DEMO_STATUS_LABELS) as DemoStatusCode[]).map((key) => (
              <option key={key} value={key}>
                {DEMO_STATUS_LABELS[key]}
              </option>
            ))}
          </select>
          <Button
            size="compact"
            variant="secondary"
            onClick={() => {
              const next = nextStatusCode(status);
              setStatus(next);
              setDiagnostics(sampleDiagnostics(next, nowRef.current()));
            }}
          >
            Next sample state
          </Button>
        </div>
      </div>

      {!state.running ? (
        <Notice tone="empty" title="Demo stopped">
          <p>Select Start demo to run the silent sample. Playback never begins.</p>
        </Notice>
      ) : (
        <Notice tone="warning" title="Silent demo running">
          <p>Meters and diagnostics are simulated fixtures. No audio is played and no host is contacted.</p>
        </Notice>
      )}

      <section aria-labelledby="demo-channels-heading" className="flex flex-col gap-4">
        <h2 id="demo-channels-heading" className="text-section text-text">
          Simulated channels
        </h2>
        <ul className="flex list-none flex-col gap-4 p-0">
          {state.channels.map((channel) => (
            <li
              key={channel.sourceId}
              className="flex flex-col gap-3 rounded-panel border border-boundary bg-surface p-4"
            >
              <header className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                <h3 className="min-w-0 truncate text-label text-text" title={channel.label}>
                  {channel.label}
                </h3>
                <span className="text-caption text-text-secondary">Simulated channel</span>
              </header>
              {linksById.get(channel.sourceId) ? (
                <p className="text-caption text-text-secondary">{linksById.get(channel.sourceId)}</p>
              ) : null}

              <DigitalMeter
                value={meters[channel.sourceId]}
                label={`${channel.label} input`}
                simulated
              />

              <GainControl
                label={channel.label}
                valueDb={channel.requestedDb}
                muted={channel.muted}
                onChange={(db) => setChannelGain(channel.sourceId, db)}
                simulated
              />

              <Toggle
                label={`Mute ${channel.label}`}
                checked={channel.muted}
                onChange={(muted) => setChannelMute(channel.sourceId, muted)}
                onLabel="Muted"
                offLabel="Not muted"
                disabled={!state.running && channel.muted}
                description={
                  !state.running && channel.muted
                    ? 'Start the demo before unmuting. Changing gain does not unmute.'
                    : undefined
                }
              />
            </li>
          ))}
        </ul>
      </section>

      <section aria-labelledby="demo-master-heading" className="flex flex-col gap-3 rounded-panel border border-boundary bg-surface p-4">
        <h2 id="demo-master-heading" className="text-section text-text">
          Simulated personal Master
        </h2>
        <div className="flex flex-col gap-2">
          <div className="flex items-baseline justify-between gap-2">
            <label htmlFor="demo-master" className="text-label text-text">
              Master attenuation
            </label>
            <span className="font-mono text-readout tabular-nums text-text">
              {state.masterMuted ? 'Muted (\u2212\u221E dB)' : `${state.masterDb} dB`}
              <span className="ml-1 font-sans text-caption text-warning">SIMULATED</span>
            </span>
          </div>
          <input
            id="demo-master"
            type="range"
            className="h-2 w-full cursor-pointer accent-accent"
            min={-60}
            max={0}
            step={1}
            value={state.masterDb}
            onChange={(event) =>
              setState((previous) => ({ ...previous, masterDb: Number(event.target.value) }))
            }
            aria-valuetext={`Personal master, minus ${Math.abs(state.masterDb)} decibels`}
          />
        </div>
        <Toggle
          label="Personal master mute"
          checked={state.masterMuted}
          onChange={(muted) => {
            if (!muted && !state.running) return;
            setState((previous) => ({ ...previous, masterMuted: muted }));
          }}
          onLabel="Muted"
          offLabel="Not muted"
          disabled={!state.running && state.masterMuted}
          description={
            !state.running && state.masterMuted
              ? 'Start the demo before unmuting. Changing gain does not unmute.'
              : undefined
          }
        />
      </section>

      <LatencyDiagnostics snapshot={diagnostics} simulated />
    </div>
  );
}
