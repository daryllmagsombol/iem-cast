import { useEffect, useMemo, useRef, useState } from 'react';
import type {
  DeviceInfo,
  SourceInfo,
  SourceLinkPair,
  SourcePanDefault,
  SourceSettings,
} from '../protocol';
import type { HostBridge } from '../desktop-bridge/contracts';
import { PanControl } from '../ui/PanControl';
import { Button } from '../ui/Button';

export interface SourcesViewProps {
  bridge?: HostBridge;
  device?: DeviceInfo | null;
  /**
   * The real host-running state, supplied by the parent. Defaults to `false` (stopped). This
   * component never infers whether the host is running: it only trusts the prop.
   */
  hostRunning?: boolean;
}

type SaveStatus = 'idle' | 'saving' | 'saved' | 'failed' | 'uncertain' | 'conflict';

/** A save the user must explicitly consent to, because it replaces an incompatible binding. */
type PendingConsent = { kind: 'defaults' } | { kind: 'links'; pairs: SourceLinkPair[] };

interface DefaultEntry {
  key: string;
  kind: 'mono' | 'pair';
  sourceIds: string[];
  labels: string[];
  /** Committed catalog default for this entry, normalized `[-1, 1]`. */
  committed: number;
  /** Unpaired `masterLr` keeps its parity split and exposes no default pan control. */
  disabled: boolean;
}

/** Build the grouped entries: valid reciprocal pairs become one entry, the rest stay mono. */
function buildEntries(sources: SourceInfo[]): DefaultEntry[] {
  const byId = new Map(sources.map(s => [s.sourceId, s]));
  const used = new Set<string>();
  const entries: DefaultEntry[] = [];
  for (const source of sources) {
    if (used.has(source.sourceId)) continue;
    const peer = source.stereoPair ? byId.get(source.stereoPair) : undefined;
    const reciprocal = peer !== undefined && peer.stereoPair === source.sourceId;
    const oppositeSides =
      source.stereoSide !== null &&
      peer !== undefined &&
      peer.stereoSide !== null &&
      source.stereoSide !== peer.stereoSide;
    if (reciprocal && oppositeSides) {
      used.add(source.sourceId);
      used.add(peer.sourceId);
      const left = source.stereoSide === 'left' ? source : peer;
      const right = source.stereoSide === 'left' ? peer : source;
      entries.push({
        key: `pair:${left.sourceId}|${right.sourceId}`,
        kind: 'pair',
        sourceIds: [left.sourceId, right.sourceId],
        labels: [left.label, right.label],
        // Both members carry the same committed pan; the left member is the canonical source.
        committed: left.defaultPan,
        disabled: false,
      });
    } else {
      used.add(source.sourceId);
      entries.push({
        key: `mono:${source.sourceId}`,
        kind: 'mono',
        sourceIds: [source.sourceId],
        labels: [source.label],
        committed: source.defaultPan,
        disabled: source.role === 'masterLr',
      });
    }
  }
  return entries;
}

/** Map every source id to the entry that owns it. */
function entryBySource(entries: DefaultEntry[]): Map<string, DefaultEntry> {
  const map = new Map<string, DefaultEntry>();
  for (const entry of entries) for (const id of entry.sourceIds) map.set(id, entry);
  return map;
}

/** The committed default for an entry from a settings view, normalized `[-1, 1]`. */
function committedFor(entry: DefaultEntry, settings: SourceSettings): number {
  const source = settings.catalog.sources.find(s => s.sourceId === entry.sourceIds[0]);
  return source ? source.defaultPan : entry.committed;
}

/** A complete default vector in catalog order: drafts where present, committed otherwise. */
function buildDefaults(
  entries: DefaultEntry[],
  drafts: Record<string, number>,
  sources: SourceInfo[],
): SourcePanDefault[] {
  const owners = entryBySource(entries);
  return sources.map(source => {
    const entry = owners.get(source.sourceId);
    const draft = entry ? drafts[entry.key] : undefined;
    return { sourceId: source.sourceId, pan: typeof draft === 'number' ? draft : source.defaultPan };
  });
}

/** Keys whose draft differs from the committed value and are editable. */
function dirtyKeys(
  entries: DefaultEntry[],
  drafts: Record<string, number>,
  settings: SourceSettings,
): string[] {
  return entries
    .filter(entry => !entry.disabled && drafts[entry.key] !== committedFor(entry, settings))
    .map(entry => entry.key);
}

/** Decimal counter compare; a malformed value is treated as not-older rather than thrown. */
function isOlderRevision(candidate: string, current: string): boolean {
  try {
    return BigInt(candidate) < BigInt(current);
  } catch {
    return false;
  }
}

/** The host's stable machine error code, when the rejection carried one. */
function errorCode(error: unknown): string | null {
  if (typeof error === 'object' && error !== null && 'code' in error) {
    const code = (error as { code?: unknown }).code;
    if (typeof code === 'string') return code;
  }
  return null;
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

/** A commit that installed the change advanced the revision; only a reload recovers. */
const RELOAD_CODES = new Set([
  'SOURCE_SETTINGS_DURABILITY_UNCERTAIN',
  'SOURCE_SETTINGS_REVISION_CONFLICT',
]);

function currentPairs(entries: DefaultEntry[]): SourceLinkPair[] {
  return entries
    .filter(entry => entry.kind === 'pair')
    .map(entry => ({ leftSourceId: entry.sourceIds[0], rightSourceId: entry.sourceIds[1] }));
}

// The catalog is read-only in this build: it mirrors the SELECTED device's real channels, derived
// by the host exactly as it does for phones. Nothing here is fabricated. The defaults section
// below is the only mutation surface, and it only writes saved defaults for NEW personal mixes.
export function SourcesView({ bridge, device, hostRunning = false }: SourcesViewProps) {
  const [sources, setSources] = useState<SourceInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const deviceId = device?.deviceId ?? null;

  const settingsCapable = Boolean(bridge) && typeof bridge?.readSourceSettings === 'function';

  const [settings, setSettings] = useState<SourceSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadingSettings, setLoadingSettings] = useState(false);
  const [drafts, setDrafts] = useState<Record<string, number>>({});
  const [status, setStatus] = useState<Record<string, SaveStatus>>({});
  const [statusMessage, setStatusMessage] = useState<Record<string, string>>({});
  const [consentPending, setConsentPending] = useState<PendingConsent | null>(null);
  const [scan, setScan] = useState(0);
  const [leftChoice, setLeftChoice] = useState('');
  const [rightChoice, setRightChoice] = useState('');
  const [linkStatus, setLinkStatus] = useState<SaveStatus>('idle');
  const [linkMessage, setLinkMessage] = useState<string | null>(null);
  const [linkReloadNeeded, setLinkReloadNeeded] = useState(false);

  const generationRef = useRef(0);
  const inFlightRef = useRef(false);
  const queuedRef = useRef(false);
  const replaceConfirmedRef = useRef(false);
  /** The exact intended link pair set for a pending/returned link save, replayed by Retry. */
  const linkIntentRef = useRef<SourceLinkPair[] | null>(null);

  const bridgeRef = useRef(bridge);
  const settingsRef = useRef(settings);
  const draftsRef = useRef(drafts);
  bridgeRef.current = bridge;
  settingsRef.current = settings;
  draftsRef.current = drafts;

  const entries = useMemo(() => buildEntries(settings?.catalog.sources ?? []), [settings]);
  const entriesRef = useRef(entries);
  entriesRef.current = entries;

  useEffect(() => {
    if (!bridge || deviceId === null) {
      setSources(null);
      setError(null);
      return undefined;
    }
    let current = true;
    setSources(null);
    setError(null);
    bridge
      .sourceCatalog(deviceId)
      .then(list => {
        if (current) setSources(list);
      })
      .catch(e => {
        if (current) {
          setSources(null);
          setError(e instanceof Error ? e.message : 'Could not read the source catalog.');
        }
      });
    return () => {
      current = false;
    };
  }, [bridge, deviceId]);

  useEffect(() => {
    if (!settingsCapable || !bridge || deviceId === null) {
      setSettings(null);
      setLoadError(null);
      setLoadingSettings(false);
      draftsRef.current = {};
      setDrafts({});
      setStatus({});
      setStatusMessage({});
      return undefined;
    }
    generationRef.current += 1;
    const generation = generationRef.current;
    inFlightRef.current = false;
    queuedRef.current = false;
    replaceConfirmedRef.current = false;
    linkIntentRef.current = null;
    settingsRef.current = null;
    draftsRef.current = {};
    setSettings(null);
    setDrafts({});
    setStatus({});
    setStatusMessage({});
    setLoadError(null);
    setConsentPending(null);
    setLinkStatus('idle');
    setLinkMessage(null);
    setLinkReloadNeeded(false);
    setLoadingSettings(true);

    const request = bridge.readSourceSettings(deviceId);
    Promise.resolve(request)
      .then(next => {
        if (generation !== generationRef.current) return;
        settingsRef.current = next;
        setSettings(next);
        const seeded: Record<string, number> = {};
        for (const entry of buildEntries(next.catalog.sources)) seeded[entry.key] = entry.committed;
        draftsRef.current = seeded;
        setDrafts(seeded);
        setLoadingSettings(false);
      })
      .catch(e => {
        if (generation !== generationRef.current) return;
        settingsRef.current = null;
        setSettings(null);
        setLoadError(e instanceof Error ? e.message : 'Saved defaults are unavailable.');
        setLoadingSettings(false);
      });

    return undefined;
  }, [bridge, deviceId, settingsCapable, scan]);

  async function flush() {
    if (inFlightRef.current) {
      queuedRef.current = true;
      return;
    }
    const activeBridge = bridgeRef.current;
    const current = settingsRef.current;
    if (!activeBridge || !current) return;
    const pending = dirtyKeys(entriesRef.current, draftsRef.current, current);
    if (pending.length === 0) return;
    if (current.bindingStatus === 'replacement' && !replaceConfirmedRef.current) {
      setConsentPending({ kind: 'defaults' });
      return;
    }
    await saveDefaultsNow(pending);
  }

  async function saveDefaultsNow(pending: string[]) {
    const activeBridge = bridgeRef.current;
    const current = settingsRef.current;
    if (!activeBridge || !current || pending.length === 0) return;

    const generation = generationRef.current;
    const targetDeviceId = current.deviceId;
    const baseRevision = current.settingsRevision;
    const payload = buildDefaults(entriesRef.current, draftsRef.current, current.catalog.sources);
    inFlightRef.current = true;
    setStatus(previous => {
      const next = { ...previous };
      for (const key of pending) next[key] = 'saving';
      return next;
    });
    setStatusMessage(previous => {
      const next = { ...previous };
      for (const key of pending) delete next[key];
      return next;
    });

    try {
      const result = await activeBridge.saveSourceDefaults(
        targetDeviceId,
        baseRevision,
        payload,
        replaceConfirmedRef.current,
      );
      // Discard a response whose device or request generation is no longer current.
      if (generation !== generationRef.current || settingsRef.current?.deviceId !== targetDeviceId) {
        return;
      }
      // A response carrying an older committed revision is stale; keep the newer draft and reload.
      if (isOlderRevision(result.settingsRevision, baseRevision)) {
        markDefaults(pending, 'conflict', 'The saved defaults changed underneath this request. Reload before retrying.');
        return;
      }
      settingsRef.current = result;
      setSettings(result);
      const durable = result.durabilityUncertain;
      setStatus(previous => {
        const next = { ...previous };
        for (const key of pending) {
          const entry = entriesRef.current.find(e => e.key === key);
          const resolved =
            entry !== undefined && draftsRef.current[key] === committedFor(entry, result);
          next[key] = resolved ? (durable ? 'uncertain' : 'saved') : 'idle';
        }
        return next;
      });
      if (durable) {
        setStatusMessage(previous => {
          const next = { ...previous };
          for (const key of pending) {
            next[key] = 'Installed, but restart durability was not confirmed. Reload before retrying.';
          }
          return next;
        });
      }
      replaceConfirmedRef.current = false;
      setConsentPending(null);
    } catch (error) {
      if (generation !== generationRef.current) return;
      const code = errorCode(error);
      const fallback = errorMessage(error, 'Could not save the defaults.');
      if (code === 'SOURCE_SETTINGS_DURABILITY_UNCERTAIN') {
        markDefaults(pending, 'uncertain', 'Installed, but restart durability was not confirmed. Reload before retrying.');
      } else if (RELOAD_CODES.has(code ?? '')) {
        markDefaults(pending, 'conflict', fallback);
      } else {
        // A plain failure keeps the draft and the host message so Retry is actionable.
        markDefaults(pending, 'failed', fallback);
      }
      // A failed confirm must not leave consent standing for a later, unconfirmed edit.
      replaceConfirmedRef.current = false;
    } finally {
      inFlightRef.current = false;
      if (queuedRef.current && generation === generationRef.current) {
        queuedRef.current = false;
        void flush();
      } else {
        queuedRef.current = false;
      }
    }
  }

  function markDefaults(keys: string[], state: SaveStatus, message: string) {
    setStatus(previous => {
      const next = { ...previous };
      for (const key of keys) next[key] = state;
      return next;
    });
    setStatusMessage(previous => {
      const next = { ...previous };
      for (const key of keys) next[key] = message;
      return next;
    });
  }

  function handleChange(key: string, value: number) {
    draftsRef.current = { ...draftsRef.current, [key]: value };
    setDrafts(draftsRef.current);
    setStatus(previous => {
      const current = previous[key];
      if (current === 'failed' || current === 'saved' || current === 'uncertain' || current === 'conflict') {
        return { ...previous, [key]: 'idle' };
      }
      return previous;
    });
    setStatusMessage(previous => {
      if (!(key in previous)) return previous;
      const next = { ...previous };
      delete next[key];
      return next;
    });
    // A draft reverted back to the committed value no longer needs a replacement confirmation.
    const current = settingsRef.current;
    if (
      current !== null &&
      dirtyKeys(entriesRef.current, draftsRef.current, current).length === 0
    ) {
      setConsentPending(previous => (previous?.kind === 'defaults' ? null : previous));
    }
    void flush();
  }

  /** Re-read the committed settings from the host, discarding drafts/status. */
  function reload() {
    setConsentPending(null);
    replaceConfirmedRef.current = false;
    linkIntentRef.current = null;
    setScan(n => n + 1);
  }

  function confirmConsent() {
    const pending = consentPending;
    replaceConfirmedRef.current = true;
    setConsentPending(null);
    if (pending?.kind === 'links') {
      void runLinkSave(pending.pairs, true);
      return;
    }
    void flush();
  }

  function cancelConsent() {
    const pending = consentPending;
    replaceConfirmedRef.current = false;
    if (pending?.kind === 'links') {
      linkIntentRef.current = null;
    } else {
      // Drop the unsaved default draft and return every editable entry to its committed value.
      const current = settingsRef.current;
      if (current) {
        const restored: Record<string, number> = {};
        for (const entry of entriesRef.current) restored[entry.key] = committedFor(entry, current);
        draftsRef.current = restored;
        setDrafts(restored);
      }
      setStatus({});
      setStatusMessage({});
    }
    setConsentPending(null);
  }

  async function runLinkSave(pairs: SourceLinkPair[], confirmed: boolean) {
    const activeBridge = bridgeRef.current;
    const current = settingsRef.current;
    if (!activeBridge || !current || hostRunning) return;
    if (current.bindingStatus === 'replacement' && !confirmed) {
      // Reuse the same explicit consent step for link saves; the intended mutation is preserved.
      linkIntentRef.current = pairs;
      setConsentPending({ kind: 'links', pairs });
      return;
    }
    const generation = generationRef.current;
    const targetDeviceId = current.deviceId;
    const baseRevision = current.settingsRevision;
    setLinkStatus('saving');
    setLinkMessage(null);
    setLinkReloadNeeded(false);
    try {
      const result = await activeBridge.saveSourceLinks(
        targetDeviceId,
        baseRevision,
        pairs,
        confirmed,
      );
      if (generation !== generationRef.current || settingsRef.current?.deviceId !== targetDeviceId) return;
      if (isOlderRevision(result.settingsRevision, baseRevision)) {
        linkIntentRef.current = pairs;
        setLinkStatus('conflict');
        setLinkReloadNeeded(true);
        setLinkMessage('The saved links changed underneath this request. Reload before retrying.');
        return;
      }
      settingsRef.current = result;
      setSettings(result);
      const seeded: Record<string, number> = {};
      for (const entry of buildEntries(result.catalog.sources)) seeded[entry.key] = entry.committed;
      draftsRef.current = seeded;
      setDrafts(seeded);
      setLeftChoice('');
      setRightChoice('');
      linkIntentRef.current = null;
      replaceConfirmedRef.current = false;
      if (result.durabilityUncertain) {
        setLinkStatus('uncertain');
        setLinkReloadNeeded(true);
        setLinkMessage('Links were installed, but restart durability was not confirmed. Reload saved defaults before retrying.');
      } else {
        setLinkStatus('saved');
        setLinkMessage(null);
      }
    } catch (error) {
      if (generation !== generationRef.current) return;
      // Keep the exact intended mutation so Retry replays it rather than the committed set.
      linkIntentRef.current = pairs;
      replaceConfirmedRef.current = false;
      const code = errorCode(error);
      if (code === 'SOURCE_SETTINGS_DURABILITY_UNCERTAIN') {
        setLinkStatus('uncertain');
        setLinkReloadNeeded(true);
        setLinkMessage('Links were installed, but restart durability was not confirmed. Reload saved defaults before retrying.');
      } else if (RELOAD_CODES.has(code ?? '')) {
        setLinkStatus('conflict');
        setLinkReloadNeeded(true);
        setLinkMessage(errorMessage(error, 'The saved links changed underneath this request. Reload before retrying.'));
      } else {
        setLinkStatus('failed');
        setLinkMessage(errorMessage(error, 'Could not save the stereo links.'));
      }
    }
  }

  function handleLinkRetry() {
    const intended = linkIntentRef.current;
    if (!intended) return;
    void runLinkSave(intended, replaceConfirmedRef.current);
  }

  function findLinkablePair(): { leftSourceId: string; rightSourceId: string } | null {
    return (
      currentPairs(entriesRef.current).find(
        pair =>
          (pair.leftSourceId === leftChoice && pair.rightSourceId === rightChoice) ||
          pair.leftSourceId === leftChoice ||
          pair.rightSourceId === rightChoice,
      ) ?? null
    );
  }

  function handleLink() {
    if (!leftChoice || !rightChoice || leftChoice === rightChoice) {
      setLinkMessage('Choose two different sources to link.');
      return;
    }
    const owners = entryBySource(entriesRef.current);
    const leftEntry = owners.get(leftChoice);
    const rightEntry = owners.get(rightChoice);
    if (leftEntry?.kind === 'pair' || rightEntry?.kind === 'pair') {
      setLinkMessage('One of the chosen sources is already linked. Unlink it first.');
      return;
    }
    void runLinkSave(
      [...currentPairs(entriesRef.current), { leftSourceId: leftChoice, rightSourceId: rightChoice }],
      replaceConfirmedRef.current,
    );
  }

  function handleUnlink() {
    const match = findLinkablePair();
    if (!match) {
      setLinkMessage('Choose a linked source to unlink.');
      return;
    }
    void runLinkSave(
      currentPairs(entriesRef.current).filter(
        pair => !(pair.leftSourceId === match.leftSourceId && pair.rightSourceId === match.rightSourceId),
      ),
      replaceConfirmedRef.current,
    );
  }

  function handleReverse() {
    const match = findLinkablePair();
    if (!match) {
      setLinkMessage('Choose a linked pair to reverse.');
      return;
    }
    void runLinkSave(
      currentPairs(entriesRef.current).map(pair =>
        pair.leftSourceId === match.leftSourceId && pair.rightSourceId === match.rightSourceId
          ? { leftSourceId: match.rightSourceId, rightSourceId: match.leftSourceId }
          : pair,
      ),
      replaceConfirmedRef.current,
    );
  }

  const linkEnabled = settings !== null && !hostRunning;

  return (
    <div className="iem-stack">
      <h2 className="text-section">Sources</h2>
      {!device && <p className="iem-hint">Select a capture device to see its source channels.</p>}
      {device && !bridge && (
        <p className="iem-hint">Source details are unavailable in a browser. Use the desktop app.</p>
      )}
      {device && bridge && error && (
        <p role="alert" className="text-danger">{error}</p>
      )}
      {device && bridge && !error && sources !== null && sources.length === 0 && (
        <p className="iem-hint">This device reports no source channels.</p>
      )}
      {device && bridge && !error && sources !== null && sources.length > 0 && (
        <>
          <p className="text-text-secondary">
            All {sources.length} channels reported by {device.name} are already available to
            musicians. No publishing step is required: a listener can mix these as soon as they
            join. Treating the channel names as fixed is a deliberate limit of this build.
          </p>
          <ul className="iem-stack" aria-label="Reported source channels">
            {sources.map(source => (
              <li key={source.sourceId} className="iem-row">
                <span className="text-label">{source.label}</span>
                <span className="text-caption text-text-secondary">
                  Channel {source.physicalIndex + 1} · {source.role}
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
      {/*
        Deliberately no disabled "Save label" / "Publish sources" buttons: they would imply a
        required step that does not exist. The host already exposes every channel, so the honest
        statement above replaces a control that could only mislead.
      */}

      {device && settingsCapable && (
        <section className="iem-stack" aria-labelledby="source-defaults-heading">
          <h3 id="source-defaults-heading" className="text-section">
            Defaults for new personal mixes
          </h3>
          <p className="text-text-secondary">
            These defaults are saved on this computer and apply only to new personal mixes. They do
            not change existing mixes, and they never start listening, arm a session, or unmute
            audio.
          </p>

          {loadingSettings && (
            <p role="status" className="text-text-secondary">Loading saved defaults…</p>
          )}

          {!loadingSettings && loadError && (
            <>
              <p role="alert" className="text-danger">Saved defaults are unavailable.</p>
              <p className="text-caption text-text-secondary">{loadError}</p>
              <div className="iem-row">
                <Button onClick={() => setScan(n => n + 1)}>Retry</Button>
              </div>
            </>
          )}

          {!loadingSettings && settings && (
            <>
              <ProfileStatus settings={settings} />

              {consentPending && (
                <div role="alert" className="iem-stack rounded-panel border border-boundary bg-surface-raised p-4">
                  <p className="text-label text-warning">Replace the saved profile?</p>
                  <p className="text-text-secondary">
                    {consentPending.kind === 'links'
                      ? 'The saved defaults belong to a different device or layout. Replacing them to save stereo links requires your explicit confirmation and a stopped host.'
                      : 'The saved defaults belong to a different device or layout. Replacing them requires your explicit confirmation and a stopped host.'}
                  </p>
                  <div className="iem-row">
                    <Button variant="primary" onClick={confirmConsent}>Confirm save</Button>
                    <Button onClick={cancelConsent}>Cancel</Button>
                  </div>
                </div>
              )}

              <ul className="iem-stack" aria-label="Default pan and balance">
                {entries.map(entry => {
                  const value = drafts[entry.key] ?? entry.committed;
                  const controlLabel = entry.kind === 'pair' ? 'Default balance' : 'Default pan';
                  const fullLabel =
                    entry.kind === 'pair'
                      ? `${entry.labels[0]} and ${entry.labels[1]} default balance`
                      : `${entry.labels[0]} default pan`;
                  return (
                    <li key={entry.key} className="iem-stack">
                      <div className="iem-row">
                        <span className="text-label">{entry.labels.join(' + ')}</span>
                        <span className="text-caption text-text-secondary">
                          {entry.kind === 'pair' ? 'Stereo pair' : 'Mono source'}
                        </span>
                      </div>
                      {entry.disabled ? (
                        <p className="text-caption text-text-secondary">
                          This master output keeps its fixed stereo split and has no default pan.
                        </p>
                      ) : (
                        <PanControl
                          label={controlLabel}
                          fullLabel={fullLabel}
                          value={value}
                          onChange={next => handleChange(entry.key, next)}
                          onReset={() => handleChange(entry.key, 0)}
                        />
                      )}
                      <SaveState
                        state={status[entry.key] ?? 'idle'}
                        message={statusMessage[entry.key]}
                        onRetry={() => void flush()}
                        onReload={reload}
                      />
                    </li>
                  );
                })}
              </ul>

              <div className="iem-stack">
                <h4 className="text-label">Stereo links</h4>
                <p className="text-caption text-text-secondary">
                  Link, unlink, or reverse a stereo pair. These changes are allowed only while the
                  host is stopped, and they reset the affected defaults to center.
                </p>
                {hostRunning && (
                  <p className="text-caption text-text-secondary">
                    The host is running. Stop it to change stereo links.
                  </p>
                )}
                <div className="iem-split">
                  <label className="iem-field">
                    Left source
                    <select
                      value={leftChoice}
                      disabled={!linkEnabled}
                      onChange={event => setLeftChoice(event.target.value)}
                    >
                      <option value="">Choose a source</option>
                      {settings.catalog.sources.map(source => (
                        <option key={source.sourceId} value={source.sourceId}>{source.label}</option>
                      ))}
                    </select>
                  </label>
                  <label className="iem-field">
                    Right source
                    <select
                      value={rightChoice}
                      disabled={!linkEnabled}
                      onChange={event => setRightChoice(event.target.value)}
                    >
                      <option value="">Choose a source</option>
                      {settings.catalog.sources.map(source => (
                        <option key={source.sourceId} value={source.sourceId}>{source.label}</option>
                      ))}
                    </select>
                  </label>
                </div>
                <div className="iem-row">
                  <Button disabled={!linkEnabled} onClick={handleLink}>Link</Button>
                  <Button disabled={!linkEnabled} onClick={handleUnlink}>Unlink</Button>
                  <Button disabled={!linkEnabled} onClick={handleReverse}>Reverse</Button>
                </div>
                {linkMessage && <p role="status" className="text-caption text-text-secondary">{linkMessage}</p>}
                <SaveState
                  state={linkStatus}
                  message={undefined}
                  onRetry={handleLinkRetry}
                  onReload={linkReloadNeeded ? reload : undefined}
                />
              </div>
            </>
          )}
        </section>
      )}
    </div>
  );
}

function ProfileStatus({ settings }: { settings: SourceSettings }) {
  if (settings.durabilityUncertain) {
    return (
      <p role="status" className="text-caption text-warning">
        Defaults were installed, but restart durability was not confirmed. Reload saved defaults
        before retrying.
      </p>
    );
  }
  switch (settings.bindingStatus) {
    case 'saved':
      return (
        <p role="status" className="text-caption text-success">
          Saved profile is active for this device.
        </p>
      );
    case 'new':
      return (
        <p role="status" className="text-caption text-text-secondary">
          Preview only — these defaults are not saved yet.
        </p>
      );
    case 'replacement':
      return (
        <p role="status" className="text-caption text-warning">
          This profile was saved for a different device or layout. Saving requires explicit
          confirmation.
        </p>
      );
    case 'legacy':
      return (
        <p role="status" className="text-caption text-warning">
          This profile was migrated from an earlier version and its channel layout is unresolved.
        </p>
      );
  }
}

function SaveState({
  state,
  message,
  onRetry,
  onReload,
}: {
  state: SaveStatus;
  message?: string;
  onRetry(): void;
  onReload?: () => void;
}) {
  if (state === 'saving') {
    return <span role="status" className="text-caption text-text-secondary">Saving…</span>;
  }
  if (state === 'saved') {
    return <span role="status" className="text-caption text-success">Saved</span>;
  }
  if (state === 'uncertain') {
    return (
      <span className="flex flex-wrap items-center gap-2 text-caption text-warning">
        <span role="status">{message ?? 'Installed, but restart durability was not confirmed.'}</span>
        <Button size="compact" onClick={onReload ?? onRetry}>Reload</Button>
      </span>
    );
  }
  if (state === 'conflict') {
    return (
      <span className="flex flex-wrap items-center gap-2 text-caption text-warning">
        <span role="status">
          {message ?? 'The saved settings changed underneath this request. Reload before retrying.'}
        </span>
        <Button size="compact" onClick={onReload ?? onRetry}>Reload</Button>
      </span>
    );
  }
  if (state === 'failed') {
    return (
      <span className="flex flex-wrap items-center gap-2 text-caption text-danger">
        <span role="status">failed</span>
        {message ? <span className="text-text-secondary">{message}</span> : null}
        <Button size="compact" onClick={onRetry}>Retry</Button>
      </span>
    );
  }
  return null;
}
