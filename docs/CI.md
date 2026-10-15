# CI: tests, SonarQube, and AI code review

This repository has three workflows under `.github/workflows/`:

| Workflow | File | Trigger | Blocking? |
|---|---|---|---|
| Tests | `tests.yml` | PRs, push to `main`, manual | **Yes** — correctness gate |
| SonarQube | `sonarqube.yml` | push to `main`, `v*` tags, PRs to `main`, manual | **Yes** when it runs — `sonar.qualitygate.wait=true` fails on a red gate |
| AI Code Review | `opencode-review.yml` | PR opened/synchronize/reopened/ready_for_review | No — advisory (`continue-on-error`), and **disabled by default** (see below) |

The existing Pages deployment (`.github/workflows/pages.yml`) is unchanged.

## Correction to earlier docs

An earlier revision of this file described **both** SonarQube and AI review as
"informational". That was wrong for SonarQube: the workflow sets
`-Dsonar.qualitygate.wait=true`, so a failing quality gate **fails the job**. The
job only skips (and does not fail) when `SONAR_TOKEN`/`SONAR_HOST_URL` are absent.
This document now reflects the actual behavior.

## SonarQube baseline requirement (PR scans)

A pull-request scan compares against the target branch's existing analysis. The
project must therefore already have an analysis of the **base branch** (`main`)
on the SonarQube server before a PR scan can succeed. On the **Community Build**,
branch analysis is effectively main-only, and a configured community/PR plugin
still requires that baseline to exist. In the observed run the base `main` baseline
did not exist, so the scanner failed with
`ERROR No branch exists in Sonarqube with the name main` (exit code 3) during branch
configuration. The initial PR scans failed before establishing a baseline; `main`
has not yet been successfully analyzed. Establish that baseline before PR scans
can compare new code against it.

## Tests workflow (`tests.yml`)

Two independent correctness gates. Neither uses `continue-on-error`.

### `web` job — `ubuntu-latest`

- `actions/setup-node@v5`, Node 24 (matches the repo's toolchain use and `vitest` 5).
- `npm ci` — installs from the committed root `package-lock.json` (npm workspace
  layout: `apps/web`, `apps/desktop`).
- `npm test` — root script, runs `vitest run` in `apps/web`.
- `npm run build:web` — root script, runs the `apps/web` `build` script, which
  begins with `tsc --noEmit` (typecheck) and then Vite-builds the admin and
  musician bundles.
- `npx playwright install --with-deps chromium` — Playwright is pinned by the root
  lockfile (`@playwright/test` 1.63.0).
- Browser mix regression: `node src/musician/__tests__/mix.browser.mjs` run from
  `apps/web`. This drives the real `MusicianRoot` + `ReceiverController` in headless
  Chromium against a Vite dev server (`vite.musician.config.ts` is resolved relative
  to the working directory, so the step sets `working-directory: apps/web`).

### `rust` job — `macos-15`

**Runner choice.** The Rust workspace (`Cargo.toml`) has two members: `crates/host-core`
and `apps/desktop/src-tauri`. The desktop crate depends on Tauri and `cpal`, whose
native dependencies are macOS frameworks (CoreAudio, AppKit/WebKit, etc.). On Linux the
same build requires a working WebKitGTK/ALSA/CMake stack and is not a faithful
environment. `macos-15` is an arm64 image (macOS 14 is retiring on 2026-11-02, so it is
avoided).

- Node 24 + `npm ci` + `npm run build:web` run **before** the Rust tests. This is
  required, not cosmetic: `apps/desktop/src-tauri/src/asset_embed.rs` uses
  `#[folder = "../../web/dist/musician"]` to compile the built musician bundle into
  the shell, and a desktop test asserts the join page is present. `apps/web/dist` is
  gitignored, so it is always absent on a fresh runner.
- `dtolnay/rust-toolchain@7e38f4b4… # v1` installs Rust `1.90` with the `clippy`
  component, matching `rust-toolchain.toml` (`channel = "1.90"`).
- `Swatinem/rust-cache@6323deb1… # v2` caches the Cargo registry and `target/`,
  keyed on `Cargo.lock` and the rust-toolchain files, so it stays compatible with the
  locked toolchain.
- `cargo test --workspace --locked` then
  `cargo clippy --workspace --all-targets --locked -- -D warnings`.

**Native `opus` note.** `host-core` depends on `opus 0.4` → `opusic-sys`, which builds
the Opus C library via CMake. `macos-15` includes CMake and the Xcode toolchain, so no
extra system packages are needed. The build was verified locally on macOS 1.90 with the
same commands (see "Local verification" below).

## SonarQube requirements

### Repository variables and secrets

Configure under **Settings → Secrets and variables → Actions**.

Variables (non-secret):

| Name | Required | Purpose |
|---|---|---|
| `SONAR_PROJECT_KEY` | No | Project key. Defaults to the repository name. |
| `SONAR_PROJECT_NAME` | No | Project display name. Defaults to the repository name. |

Secrets:

| Name | Required | Purpose |
|---|---|---|
| `SONAR_HOST_URL` | Yes (to run the scan) | Base URL of the SonarQube server. |
| `SONAR_TOKEN` | Yes (to run the scan) | Analysis token for the project. |

`GITHUB_TOKEN` is automatic. Never commit secret values; only the names above are
documented.

### Pull-request behavior (fixed)

The pull-request parameters (`sonar.pullrequest.key/branch/base`) are left to the
scanner's supported auto-detection. SonarScanners running in GitHub Actions perform
this automatically starting from the **Developer edition**; on the Community edition
you must either supply explicit `sonar.pullrequest.*` values from a trusted step or
accept branch-only analysis. Passing `-Dsonar.branch.name` on a `pull_request` event
would conflict with auto-detected PR parameters, so it is **not** set. No branch name
(which can be attacker-influenced on a PR) is interpolated into the scan arguments.

Project key/name are the scanner's project identity (from a repository variable with a
repository-name fallback), and they are validated in a shell step **before** being
passed to the action's `args` — key against `^[A-Za-z0-9_.:-]+$`, name against
`^[A-Za-z0-9_.:()&+/ -]+$`.

### External setup (manual, cannot run in this repo)

1. Create the project on the SonarQube server, or set `SONAR_PROJECT_KEY` to the key
   you created.
2. Generate an analysis token → `SONAR_TOKEN`; set the server URL → `SONAR_HOST_URL`.
3. Confirm the desired quality gate (the workflow waits on it via
   `sonar.qualitygate.wait=true`).
4. Optionally adjust the new-code/branch baseline on the server.

### Fork and untrusted-PR handling

- SonarQube is gated on the presence of both secrets; on a fork PR or an unconfigured
  repo it skips with a `::warning::` annotation instead of failing.
- Do **not** convert either review workflow to `pull_request_target`; both use
  `pull_request`, so fork PRs never receive repository secrets or a privileged token.

## AI Code Review (`opencode-review.yml`)

Advisory only, and **currently disabled by default**. Secrets, vars, and actions are
not written.

### Disabled by default (opt-in)

The `review` job runs only when the repository variable
**`AI_CODE_REVIEW_ENABLED == 'true'`** (Settings → Secrets and variables → Actions →
Variables). When the variable is absent or set to anything else, the **whole job is
skipped** by its job-level `if:`, so there is no checkout, no use of
`OPENCODE_API_KEY`, and **no model/provider calls or spending**. The same `if` also
keeps the existing fork exclusion:

```yaml
if: >-
  github.event.pull_request.head.repo.fork == false &&
  vars.AI_CODE_REVIEW_ENABLED == 'true'
```

Why it is off: the previously configured model
`opencode/muse-spark-1.3-contributor-free` was rejected from GitHub Actions by the
provider with `403 FreeTierError: OpenCode's free tier can only be used from within
OpenCode` (`https://opencode.ai/zen/v1/responses`; observed on PR #1 rerun attempt 2).
That is a model/provider limitation, not a GitHub permission issue. The workflow does
**not** silently substitute a paid model.

A skipped run is **not** a review: the job shows green only because nothing ran (and,
when enabled, review failures are also non-blocking via `continue-on-error`). Do not
treat a green AI Code Review check as evidence the diff was reviewed.

### How to re-enable

Only after choosing a **CI-supported** model/provider and configuring its credential(s):

1. Pick a model/provider that is permitted to be called from GitHub Actions (the
   current free tier is not). This is a deliberate decision — do not select a paid
   model without owner approval.
2. Provide the required secret(s) (e.g. `OPENCODE_API_KEY`, or the provider-specific
   key) under repository/organization Actions secrets.
3. Update the action input `model:` in `.github/workflows/opencode-review.yml` if the
   provider/model id changes.
4. Set the repository variable `AI_CODE_REVIEW_ENABLED` to `true`.
5. Confirm a real review is posted to a PR before relying on it.

Re-disabling is the reverse: set the variable to `false` or delete it.

### Enforcement boundary (accurate)

The review action is third-party and supports no declarative "read-only" mode input,
but opencode itself exposes two supported environment variables that this workflow
uses to enforce a read-only reviewer beyond prompt text:

1. **`OPENCODE_PERMISSION`** — merged into opencode's global permission config. This
   workflow sets
   `{"edit":"deny","bash":"deny","webfetch":"deny","websearch":"deny","task":"deny","external_directory":"deny"}`.
   The `edit` key covers the `edit`/`write`/`apply_patch` tools; the shell tool's
   permission key is `bash`. Denying these prevents the reviewer from modifying files
   or running shell writes. `read`, `grep`, `glob`, and `list` remain allowed so it can
   inspect the change.
2. **`OPENCODE_DISABLE_PROJECT_CONFIG=true`** — prevents the agent from loading the
   PR's own `.opencode/` config, so an untrusted PR cannot inject permissions, agents,
   or plugins that weaken the review.

Additionally, `contents: read` + `persist-credentials: false` mean the token cannot
push, so no remote write is possible.

**What this does *not* claim:** `.opencode/` being gitignored does *not* by itself
prevent local file edits by the agent — it only avoids tracked-file churn. The tool
denial above is what prevents arbitrary local edits, and `contents: read` prevents
remote push. The prompt is defense-in-depth, not the enforcement mechanism.

### Job permissions

The job uses `contents: read` (checkout), `pull-requests: write`, and `issues: write`.
A pull request's comments/reactions are served by the "issues" endpoints. Grant
`pull-requests: write` for the action to post to a PR timeline: with only
`pull-requests: read` and `issues: write`, the action's
`POST /repos/{owner}/{repo}/issues/{n}/comments` and `.../reactions` calls returned
`403 Resource not accessible by integration` (observed on PR #1).
`issues: write` remains for issue-thread operations.

No `contents` write is granted, so the action still cannot push even if a tool were
misused. These GitHub API scopes are the action's own operations; they do not grant the
agent tool permissions (those are handled by `OPENCODE_PERMISSION` above).

### Repository variable and secret

| Name | Required | Purpose |
|---|---|---|
| `AI_CODE_REVIEW_ENABLED` | Yes (to run the review) | Repository **variable**. Review runs only when exactly `'true'`. Absent/false → the job is fully skipped with no API calls. |
| `OPENCODE_API_KEY` | Yes (to run the review) | Secret. API key for the review model (CI-supported provider only). |

The action is pinned to `anomalyco/opencode/github@11e47f91496005aab4d7c5a2d0a7da5d2651b4ac`
(the commit for tag `v1.17.8`).

**Pinning limitation (accurate):** the pinned action's `action.yml` still downloads the
*latest* opencode CLI release at runtime (`.../releases/latest` + the install script),
so pinning the action commit does not pin the CLI binary itself. The workflow-level
action is reproducible; the CLI is not. If fully reproducible runs are required, this
would need a different install mechanism, which is outside this change.

### Trust boundary

Any PR from a branch in this repository can modify files the reviewer reads, including
`.opencode/` and `sonar-project.properties`. This is why the workflow disables project
config loading (`OPENCODE_DISABLE_PROJECT_CONFIG=true`) rather than trusting it.

## Coverage

Coverage is wired into the SonarQube scan. Two independent producers emit
**LCOV**, both referenced by `sonar-project.properties`:

| Producer | Command | Report | Job |
|---|---|---|---|
| Web (TS/React) | `npm run test:coverage --workspace apps/web` (Vitest v8 provider) | `apps/web/coverage/lcov.info` | `sonarqube` (ubuntu-latest) |
| Rust workspace | `cargo llvm-cov --workspace --locked --lcov --output-path rust-coverage/lcov.info` (cargo-llvm-cov) | `rust-coverage/lcov.info` | `rust-coverage` (macos-15) |

`sonarqube.yml` therefore has two jobs:

- **`rust-coverage`** — `macos-15`, because the workspace's Tauri desktop crate has
  macOS-only native dependencies (same reason as the `rust` job in `tests.yml`). It
  builds the web bundles first (the desktop crate embeds `apps/web/dist/musician` and
  a test asserts the join page is present), installs Rust `1.90` + `llvm-tools-preview`,
  installs cargo-llvm-cov via `taiki-e/install-action@cargo-llvm-cov`, runs
  `cargo llvm-cov … --lcov`, and uploads `rust-coverage/lcov.info` as the
  `rust-coverage-lcov` artifact.
- **`sonarqube`** — `needs: rust-coverage`. After `npm ci` it runs the web coverage
  (`npm run test:coverage --workspace apps/web`), downloads the Rust artifact into
  `rust-coverage/`, then scans. Coverage generation is gated on the same credential
  check as the scan, so a skipped scan (fork PR / unconfigured repo) does not run the
  suites. `-Dsonar.qualitygate.wait=true` is unchanged.

### Report paths are repo-relative (required)

SonarQube resolves each LCOV `SF:` path against `sonar.projectBaseDir` (the repo
root). Vitest's lcov reporter defaults to paths like `SF:src/...`, which then resolve
to nothing and report **0%** coverage. `apps/web/vitest.config.ts` therefore sets the
lcov reporter's `projectRoot` to the monorepo root
(`fileURLToPath(new URL('../..', import.meta.url))`), so entries are repo-relative
(`SF:apps/web/src/...`). Removing that override silently reverts to 0%.

`cargo llvm-cov` instead emits **absolute** `SF:` paths (e.g.
`/Users/runner/work/iem-cast/iem-cast/crates/host-core/src/...`). Because the coverage
job runs on **macOS** while the scan job runs on **Linux**, those paths can never
resolve against `sonar.projectBaseDir`; SonarQube then imports nothing for Rust and
every Rust file is left with no coverage data at all (not 0% — absent). The workflow
therefore rewrites the records to repo-relative immediately after generating them, and
fails the job if any absolute `SF:` path survives:

```
perl -i -pe "s|^SF:\Q${GITHUB_WORKSPACE}\E/|SF:|" rust-coverage/lcov.info
```

### `sonar-project.properties`

```
sonar.javascript.lcov.reportPaths=apps/web/coverage/lcov.info
sonar.rust.lcov.reportPaths=rust-coverage/lcov.info
```

`sonar.javascript.lcov.reportPaths` covers TypeScript too; the
`sonar.typescript.lcov.reportPaths` property is deprecated and must not be added.

### Server-side Rust import support

Importing Rust LCOV is supported by SonarQube Community Build **25.5+** (it is not
part of older releases). Verify the server version before relying on the Rust
coverage condition on the quality gate; on an older server the web report is still
imported, but the `sonar.rust.lcov.reportPaths` import is ignored.

## Local verification

Performed in the authoring environment (macOS, Node 24.20.0, Cargo/rustc 1.90.0):

| Command | Result |
|---|---|
| `npm ci --dry-run --ignore-scripts` | Passed (lockfile consistent) |
| `npm test` | Passed (28 files, 307 tests) |
| `npm run test:coverage --workspace apps/web` | Passed; emitted `apps/web/coverage/lcov.info` with repo-relative `SF:apps/web/src/...` paths |
| `npm run build:web` | Passed (typecheck + admin/musician builds) |
| `node apps/web/src/musician/__tests__/mix.browser.mjs` (from `apps/web`) | Passed |
| `cargo test --workspace --locked` | Passed (8 suites) |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed, no issues |

What cannot be validated locally: an actual GitHub Actions run, an actual SonarQube
scan/quality-gate result, and an actual AI review comment. None of these should be
reported as passing until they run green in CI with credentials configured. The Rust
coverage command (`cargo llvm-cov`) was **not** run locally (the tool is not installed
in the authoring environment); the `rust-coverage` job and its artifact handoff are
verified only by inspection plus `actionlint`, not by an actual run.

## Publishing

These files are committed only when the user commits and pushes. Nothing here creates
commits, pushes, or writes secrets automatically.
