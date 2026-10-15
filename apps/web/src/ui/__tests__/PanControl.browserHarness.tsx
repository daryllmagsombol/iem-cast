// Standalone browser-only fixture: no product wiring, signaling, media, or persistence.
import { createRoot } from 'react-dom/client';
import { PanControl } from '../PanControl';

export function mountPanControls() {
  const existing = document.getElementById('root');
  if (!existing) throw new Error('Missing fixture root');
  const fixture = document.createElement('div');
  existing.replaceWith(fixture);
  const root = createRoot(fixture);
  const events: Array<number | 'reset'> = [];
  let value = 0.5;
  let disabled = false;
  const labels = ['Pan', 'Balance', 'Default pan', 'Default balance'];
  function update(nextValue = value, nextDisabled = disabled) {
    value = nextValue;
    disabled = nextDisabled;
    root.render(
      <main className="iem-receiver pb-5">
        <h1 className="text-section">Pan control browser fixture</h1>
        <p className="text-caption text-text-secondary">Fixed test values · No audio or host connection</p>
        {labels.map(label => (
          <section key={label} aria-label={`${label} fixture`} className="iem-panel">
            <h2 className="mb-3 text-label">A deliberately long browser-test source or stereo pair name</h2>
            <PanControl
              label={label === 'Default balance' ? `${label} for a deliberately long browser-test stereo pair name` : label}
              fullLabel={`A deliberately long browser-test source or stereo pair name ${label.toLowerCase()}`}
              value={value}
              disabled={disabled}
              onChange={next => { events.push(next); update(next); }}
              onReset={() => { events.push('reset'); update(0); }}
            />
          </section>
        ))}
      </main>,
    );
  }
  update();
  return { update, events, dispose: () => root.unmount() };
}
