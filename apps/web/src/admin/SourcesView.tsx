import { useEffect, useState } from 'react';
import type { SourceInfo } from '../protocol';
import type { HostBridge } from '../desktop-bridge/contracts';
import { Button } from '../ui/Button';
import { Notice } from '../ui/Notice';

export interface SourcesViewProps {
  bridge: HostBridge;
}

export function SourcesView({ bridge }: SourcesViewProps) {
  const [sources, setSources] = useState<SourceInfo[] | null>(null);
  const [labelDrafts, setLabelDrafts] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  useEffect(() => {
    let active = true;
    bridge
      .sourceCatalog()
      .then((list) => {
        if (active) setSources(list);
      })
      .catch(() => {
        if (active) setError('Could not load the source catalog.');
      });
    return () => {
      active = false;
    };
  }, [bridge]);

  async function toggleAvailable(source: SourceInfo) {
    if (!sources) return;
    setPending(true);
    const nextIds = sources
      .filter((candidate) => (candidate.sourceId === source.sourceId ? !source.available : candidate.available))
      .map((candidate) => candidate.sourceId);
    try {
      const updated = await bridge.setAvailableSources(nextIds);
      setSources(updated.sources);
      setError(null);
    } catch {
      setError('The host rejected the availability change.');
    } finally {
      setPending(false);
    }
  }

  async function saveLabel(source: SourceInfo) {
    const draft = labelDrafts[source.sourceId];
    if (draft === undefined || draft === source.label) return;
    setPending(true);
    try {
      const updated = await bridge.setSourceLabel(source.sourceId, draft);
      setSources(updated.sources);
      setError(null);
    } catch {
      setError('The host rejected the label change.');
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <header className="flex flex-col gap-1">
        <h1 className="text-title text-text">Sources</h1>
        <p className="text-body text-text-secondary">
          Publish the channels musicians may hear and give them stable labels. Source identity survives renames.
        </p>
      </header>

      {error ? (
        <Notice tone="error" title="Source change rejected" live="alert">
          <p>{error}</p>
        </Notice>
      ) : null}

      {sources === null && !error ? (
        <Notice tone="empty" title="Loading source catalog">
          <p>Reading assigned channels from the host.</p>
        </Notice>
      ) : null}

      {sources && sources.length === 0 ? (
        <Notice tone="empty" title="No sources available">
          <p>No input channels are assigned. Map device channels before publishing sources.</p>
        </Notice>
      ) : null}

      {sources && sources.length > 0 ? (
        <ul className="flex list-none flex-col gap-3 p-0">
          {sources.map((source) => (
            <li
              key={source.sourceId}
              className="flex flex-col gap-3 rounded-panel border border-boundary bg-surface p-4"
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="font-mono text-caption text-text-secondary">
                  Input {source.physicalIndex + 1}
                  {source.role === 'masterLr' ? ' · Master L/R' : ''}
                </span>
                <Button
                  variant={source.available ? 'secondary' : 'primary'}
                  size="compact"
                  pending={pending}
                  onClick={() => void toggleAvailable(source)}
                >
                  {source.available ? 'Unavailable to musicians' : 'Make available'}
                </Button>
              </div>
              <div className="flex flex-wrap items-end gap-2">
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <label htmlFor={`label-${source.sourceId}`} className="text-label text-text">
                    Label
                  </label>
                  <input
                    id={`label-${source.sourceId}`}
                    type="text"
                    value={labelDrafts[source.sourceId] ?? source.label}
                    onChange={(event) =>
                      setLabelDrafts((drafts) => ({ ...drafts, [source.sourceId]: event.target.value }))
                    }
                    className="min-h-target-compact rounded-control border border-boundary bg-surface-raised px-2 text-body text-text"
                  />
                </div>
                <Button
                  variant="secondary"
                  size="compact"
                  pending={pending}
                  onClick={() => void saveLabel(source)}
                >
                  Save label
                </Button>
              </div>
              <p className="text-caption text-text-secondary">
                {source.available ? 'Visible to musicians' : 'Hidden from musicians'}
                {source.stereoPair ? ' · Linked stereo pair' : ' · Mono, centered'}
              </p>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
