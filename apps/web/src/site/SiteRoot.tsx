import { useCallback, useEffect, useState } from 'react';
import { repoUrl } from './config';
import { Documentation } from './Documentation';
import { MixDemo } from './MixDemo';

type Route = 'docs' | 'demo';
type Theme = 'dark' | 'light';

function routeFromHash(hash: string): Route {
  const normalized = hash.replace(/^#\/?/, '').split('?')[0];
  return normalized === 'demo' ? 'demo' : 'docs';
}

function readStoredTheme(): Theme {
  try {
    const stored = window.localStorage.getItem('iem-theme');
    return stored === 'light' ? 'light' : 'dark';
  } catch {
    return 'dark';
  }
}

/**
 * GitHub Pages site shell.
 *
 * Uses hash routes (#/docs, #/demo) so refreshes always resolve on the project
 * root and never 404 on a deep path. Base path is `/iem-cast/` (see
 * vite.site.config.ts). No runtime network, USB, LAN, WSS, WebRTC, or microphone.
 */
export function SiteRoot() {
  const [route, setRoute] = useState<Route>(() => routeFromHash(window.location.hash));
  const [theme, setTheme] = useState<Theme>(() => readStoredTheme());

  useEffect(() => {
    const onHashChange = () => setRoute(routeFromHash(window.location.hash));
    window.addEventListener('hashchange', onHashChange);
    return () => window.removeEventListener('hashchange', onHashChange);
  }, []);

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
    try {
      window.localStorage.setItem('iem-theme', theme);
    } catch {
      /* storage may be unavailable; theme still applies for this view */
    }
  }, [theme]);

  const navigate = useCallback((next: Route) => {
    window.location.hash = `#/${next}`;
    setRoute(next);
  }, []);

  const navClass = (active: boolean) =>
    `inline-flex min-h-target-compact items-center rounded-control border px-3 text-label transition-colors duration-100 ${
      active
        ? 'border-accent bg-accent text-on-accent'
        : 'border-boundary bg-surface text-text hover:bg-surface-raised'
    }`;

  return (
    <div className="min-h-dvh bg-canvas text-text">
      <a
        href="#site-main"
        className="absolute left-2 top-2 -translate-y-16 rounded-control border border-boundary bg-surface px-3 py-2 text-label text-text focus:translate-y-0"
      >
        Skip to content
      </a>

      <header className="border-b border-boundary bg-surface">
        <div className="mx-auto flex w-full max-w-5xl flex-wrap items-center justify-between gap-3 px-3 py-3 sm:px-4">
          <span className="text-label text-text">IEM Cast</span>
          <nav aria-label="Site" className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              aria-current={route === 'docs' ? 'page' : undefined}
              onClick={() => navigate('docs')}
              className={navClass(route === 'docs')}
            >
              Documentation
            </button>
            <button
              type="button"
              aria-current={route === 'demo' ? 'page' : undefined}
              onClick={() => navigate('demo')}
              className={navClass(route === 'demo')}
            >
              Mix demo
            </button>
            <button
              type="button"
              aria-pressed={theme === 'light'}
              onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
              className={navClass(false)}
            >
              {theme === 'dark' ? 'Light theme' : 'Dark theme'}
            </button>
            <a
              href={repoUrl}
              className="inline-flex min-h-target-compact items-center rounded-control border border-boundary bg-surface px-3 text-label text-accent underline hover:bg-surface-raised"
            >
              Source
            </a>
          </nav>
        </div>
      </header>

      <main id="site-main">
        {route === 'demo' ? <MixDemo /> : <Documentation />}
      </main>

      <footer className="border-t border-boundary bg-surface">
        <div className="mx-auto w-full max-w-5xl px-3 py-4 text-caption text-text-secondary sm:px-4">
          <p>POC work in progress &middot; Not stage qualified. No installer is available yet.</p>
          <p className="mt-1">
            This site is documentation and a silent simulated demo. It never contacts a host or plays audio.
          </p>
        </div>
      </footer>
    </div>
  );
}
