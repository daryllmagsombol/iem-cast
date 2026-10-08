/**
 * Public-site configuration.
 *
 * The deployment target (custom domain vs. a project GitHub Pages path) is an operator choice, so
 * nothing here hard-codes a hostname or base path. Values come from build-time environment
 * variables with neutral defaults:
 *
 * - `VITE_SITE_BASE`   — the base path Vite emits asset URLs for (default `/`).
 * - `VITE_REPO_URL`    — the repository source link (default: this project's upstream).
 * - `VITE_SITE_URL`    — the canonical public address, if a deployment wants to show one.
 */

/** The upstream repository, used only as a fallback when the build does not supply one. */
const DEFAULT_REPO_URL = 'https://github.com/daryllmagsombol/iem-cast';

function env(name: string): string | undefined {
  const value = import.meta.env[name as keyof ImportMetaEnv];
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

/** The repository URL for source/release links. */
export const repoUrl: string = env('VITE_REPO_URL') ?? DEFAULT_REPO_URL;

/** The releases page for the configured repository. */
export const releasesUrl: string = `${repoUrl}/releases`;

/** The canonical public address, or `undefined` when this build does not define one. */
export const siteUrl: string | undefined = env('VITE_SITE_URL');
