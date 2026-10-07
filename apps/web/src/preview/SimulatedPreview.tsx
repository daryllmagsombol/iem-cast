import { useState } from 'react';
import { Button } from '../ui/Button';
import { GainControl } from '../ui/GainControl';
import { DigitalMeter } from '../ui/DigitalMeter';
import { ThemeChoice } from '../ui/ThemeChoice';

// Fictional in-memory state only. No ports, IPC, transport, media or hardware adapter.
const initial = [
  { id: 'fictional-vocal', label: 'Lead vocal', db: -12, muted: true },
  { id: 'fictional-keys', label: 'Stereo keys · Linked L/R', db: -18, muted: true },
];

export function SimulatedPreview({ onExit }: { onExit(): void }) {
  const [selected, setSelected] = useState(false);
  const [channels, setChannels] = useState(initial);
  const [master, setMaster] = useState(-18);
  const [muted, setMuted] = useState(true);
  return (
    <div className="iem-shell">
      <header className="iem-topbar">
        <div>
          <p className="text-caption text-text-secondary">IEM Cast · Fictional examples</p>
          <h1 className="text-title">Simulated preview</h1>
        </div>
        <div className="iem-row">
          <ThemeChoice />
          <Button onClick={onExit}>Exit preview</Button>
        </div>
      </header>
      <p className="iem-preview-banner">SIMULATED · No host connection · No audio</p>
      <main className="iem-preview-content iem-stack">
        <section className="iem-panel iem-stack">
          <h2 className="text-section">Simulated setup</h2>
          <p>Fictional USB input · 4 example channels · Simulated 48 kHz</p>
          <Button onClick={() => setSelected(true)}>Select simulated device</Button>
          {selected && <p role="status">Simulated device selected — no capture</p>}
          <label className="iem-field">
            Simulated network interface
            <select>
              <option>Fictional Ethernet · 192.0.2.10</option>
            </select>
          </label>
          <label className="iem-field">
            Simulated certificate path
            <input defaultValue="/fictional/certificate.pem" />
          </label>
          <p className="iem-hint">These values exist only in this preview. No certificate is read, and no server or pairing code is created.</p>
        </section>
        <section className="iem-stack">
          <h2 className="text-section">Simulated personal mix</h2>
          {channels.map(channel => (
            <article className="iem-panel iem-stack" key={channel.id}>
              <h3 className="text-label">{channel.label}</h3>
              <DigitalMeter
                value={undefined}
                label={`Simulated ${channel.label} input`}
                simulated
              />
              <GainControl
                label={`Simulated ${channel.label}`}
                valueDb={channel.db}
                muted={channel.muted}
                simulated
                onChange={db =>
                  setChannels(list =>
                    list.map(c => c.id === channel.id ? { ...c, db } : c),
                  )
                }
              />
              <Button
                aria-pressed={channel.muted}
                onClick={() =>
                  setChannels(list =>
                    list.map(c =>
                      c.id === channel.id ? { ...c, muted: !c.muted } : c,
                    ),
                  )
                }
              >
                {channel.muted ? 'Unmute' : 'Mute'} simulated {channel.label}
              </Button>
            </article>
          ))}
        </section>
        <section className="iem-panel iem-stack">
          <h2 className="text-section">Simulated Master</h2>
          <GainControl
            label="Simulated Master attenuation"
            valueDb={master}
            onChange={setMaster}
            muted={muted}
            simulated
          />
          <Button aria-pressed={muted} onClick={() => setMuted(!muted)}>
            {muted ? 'Unmute simulated Master' : 'Mute simulated Master'}
          </Button>
          <Button
            onClick={() => {
              setChannels(initial);
              setMaster(-18);
              setMuted(true);
            }}
          >
            Reset simulated mix
          </Button>
          <p>End-to-end audio latency: Not measured</p>
          <p className="text-caption text-text-secondary">SIMULATED examples only. No listening session is armed and no sound is played.</p>
        </section>
      </main>
    </div>
  );
}
