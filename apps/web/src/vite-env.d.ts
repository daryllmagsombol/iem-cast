/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Base path the public site is built for (e.g. `/` or `/iem-cast/`). */
  readonly VITE_SITE_BASE?: string;
  /** Repository URL used for source/release links on the public site. */
  readonly VITE_REPO_URL?: string;
  /** Canonical public address of the deployed site, when one is defined. */
  readonly VITE_SITE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
