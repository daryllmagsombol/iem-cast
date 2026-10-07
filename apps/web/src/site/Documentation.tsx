export function Documentation() {
  const repo = 'https://github.com/daryllmagsombol/iem-cast';

  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-6 px-3 py-6 sm:px-4 lg:flex-row lg:gap-8">
      <nav aria-label="Documentation sections" className="lg:w-60 lg:shrink-0">
        <h2 className="text-section text-text">Sections</h2>
        <ul className="mt-2 flex list-none flex-col gap-1 p-0 text-body">
          <li>
            <a className="text-accent underline" href="#/docs">
              Overview
            </a>
          </li>
          <li>
            <a className="text-accent underline" href="#/demo">
              Mix demo
            </a>
          </li>
          <li>
            <a className="text-accent underline" href={`${repo}#readme`}>
              Repository
            </a>
          </li>
          <li>
            <a className="text-accent underline" href={`${repo}/releases`}>
              View releases
            </a>
          </li>
        </ul>
      </nav>

      <div className="flex min-w-0 flex-1 flex-col gap-6">
        <header className="flex flex-col gap-2">
          <h1 className="text-title text-text">IEM Cast documentation</h1>
          <p className="text-body text-text-secondary">
            IEM Cast captures Soundcraft USB channels into a host and generates an independent
            personal stereo mix for each musician, delivered to a wired phone browser.
          </p>
          <p className="rounded-panel border border-warning bg-surface p-3 text-label text-warning" role="note">
            POC work in progress &middot; Not stage qualified
          </p>
        </header>

        <section aria-labelledby="scope-heading" className="flex flex-col gap-2">
          <h2 id="scope-heading" className="text-section text-text">
            Scope
          </h2>
          <p className="text-body text-text-secondary">
            Capture is input-only. Streamed personal mixes are stereo. The POC supports two active
            receivers; the future production target is larger and is not yet proven. Phones use wired
            earphones and must stay visible with the screen on.
          </p>
          <p className="text-body text-text-secondary">
            Reported network and buffer numbers are not analog-input-to-ear latency. End-to-end
            electrical latency is shown as &ldquo;Not measured&rdquo; unless a physical measurement
            under stated conditions exists.
          </p>
        </section>

        <section aria-labelledby="setup-heading" className="flex flex-col gap-2">
          <h2 id="setup-heading" className="text-section text-text">
            Mac-first setup and qualification
          </h2>
          <ol className="flex list-decimal flex-col gap-1 pl-5 text-body text-text-secondary">
            <li>Confirm the Soundcraft device over USB and that 48 kHz is actually supported.</li>
            <li>Select the host network interface; VPN and public interfaces are excluded by default.</li>
            <li>Install and trust the local certificate on each test device as a manual step.</li>
            <li>Pair each phone with a fresh single-use code, then start listening by explicit action.</li>
          </ol>
          <p className="text-caption text-text-secondary">
            Full commands and gates live in the repository documents below; this page links out rather
            than duplicating them.
          </p>
        </section>

        <section aria-labelledby="links-heading" className="flex flex-col gap-2">
          <h2 id="links-heading" className="text-section text-text">
            Source documents
          </h2>
          <ul className="flex list-none flex-col gap-2 p-0">
            <li>
              <a className="text-accent underline" href={`${repo}/blob/main/README.md`}>
                README: product scope and source constraints
              </a>
            </li>
            <li>
              <a className="text-accent underline" href={`${repo}/blob/main/docs/ARCHITECTURE.md`}>
                Architecture: host, permissions, transport, and safety contracts
              </a>
            </li>
            <li>
              <a className="text-accent underline" href={`${repo}/blob/main/docs/DESIGN-SYSTEM.md`}>
                Design system: shared visual and interaction standard
              </a>
            </li>
            <li>
              <a
                className="text-accent underline"
                href={`${repo}/blob/main/docs/superpowers/plans/2026-10-07-macos-poc-implementation.md`}
              >
                macOS POC plan: proposed execution and approval gates
              </a>
            </li>
            <li>
              <a className="text-accent underline" href={`${repo}/releases`}>
                View releases
              </a>
            </li>
          </ul>
        </section>
      </div>
    </div>
  );
}
