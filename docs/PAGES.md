# GitHub Pages: documentation and simulated mix demo

**DRAFT — design for review before site implementation.** The user selected both documentation and an interactive simulated UI demo. This document does not authorize bypassing POC plan review, create a website, or claim deployment. The site is served at the configured custom domain; the base path is a deployment setting (`VITE_SITE_BASE`), not a source constant.

## Purpose and content

The public site explains IEM Cast and demonstrates personal mix controls without connecting to hardware. GitHub Pages serves static files; it cannot run the Rust capture/DSP host or replace the trusted local HTTPS receiver. Publishing the repository after POC work and publishing Pages are separate integration actions.

Use a simple header with **Documentation**, **Mix demo**, theme choice, and **Source**. The introduction describes Soundcraft USB capture, host-generated personal stereo mixes, and wired phone receivers. Keep an explicit status notice: “POC work in progress · Not stage qualified,” updating only against verified project evidence. Do not imply two receivers prove the future 8/13-client targets or advertise achieved latency.

Documentation contains a concise scope overview, Mac-first setup/qualification guidance, and links to authoritative source documents:

- [README](../README.md): product scope and source constraints.
- [Architecture](ARCHITECTURE.md): host, permissions, transport, and safety contracts.
- [Design system](DESIGN-SYSTEM.md): shared visual and interaction standard.
- [macOS POC plan](superpowers/plans/2026-10-07-macos-poc-implementation.md): proposed execution and approval gates.
- [Repository](https://github.com/daryllmagsombol/iem-cast) and [Releases](https://github.com/daryllmagsombol/iem-cast/releases).

On the eventual site, source-document links resolve to their repository locations, not nonexistent Pages Markdown routes. Use “View releases,” not an installer/download claim while artifacts are unavailable. Summaries remain short and link out; no new Markdown-rendering dependency or full documentation-book framework is needed.

## Layout and visual continuity

Follow the existing calm instrument-console design exactly: canonical light/dark semantic tokens, offline system fonts, tabular readouts, spacing, boundaries, targets, focus, and reduced-motion rules. Tailwind CSS is generated and bundled at build time. No CDN fonts, decorative effects, new palette, or component-library requirement.

Documentation uses a readable content column with section navigation beside it on desktop. On phones, navigation becomes an ordinary labeled expandable list above content, not a horizontal gesture strip. Provide a skip link and meaningful headings. The demo uses vertically stacked channel cards and a personal Master dock with reserved safe-area/scroll clearance, following the design system at 320px and above.

## MixDemo behavior and sample data

Keep this notice visible throughout: **“SIMULATED · Silent demo · No host connection.”** The primary action is **Start demo**, never Start listening. Running state says **“Demo running · No audio”**, never Listening, Connected, Armed, or stage qualified. Stop demo acts immediately, returns to the muted demo gate, and stops fixture updates.

Use a small fixed fictional catalog, including mono and stereo-linked examples and one long source name. Reuse gain, channel mute, master attenuation, and master mute contracts with native accessible ranges, numeric entry, increase/decrease buttons, explicit units, and non-drag alternatives. Fixtures use the POC’s declared bounded attenuation range, not a new audio range. Start muted; changing gain does not start the demo or unmute. Unmute requires an explicit action while the demo is running. Reset returns all mute gates to muted without starting automatically.

A site-local data adapter updates only in-memory state; reloading resets it. Pending/accepted/applied examples, if shown, explicitly say **“Simulated”** and cannot imply DSP or audible application. No host messages, credentials, pairing QR, or admin controls are exposed.

Every meter and numeric statistic carries a **SIMULATED** text badge and accessible description. Fixture digital peaks use dBFS; Network RTT and Browser buffer delay use ms and remain separate. End-to-end audio latency always reads **“Not measured.”** Never sum or halve sample diagnostics, imply fresh STUN measurements, safe hearing, or qualification. Meter fixtures refresh at most 10Hz and diagnostic fixtures once per second; neither receives live-region announcements on each update. Offer a labeled sample-state selector for normal, waiting, unavailable, and stale examples. Unavailable is not zero; stale fixtures include an explicitly simulated last-update label. These examples never auto-unmute or resume.

## Data, build, and ownership boundaries

Propose a separate Vite site output with base **`/iem-cast/`**, independent of admin and musician outputs. Use hash navigation such as `#/docs` and `#/demo`, keeping refreshes on the project root to avoid deep-path Pages 404s. Bundle all required assets locally; Node is build tooling, not a production server.

Reuse frozen presentational UI primitives and approved types only after POC interfaces exist. The adapter must not import the real ReceiverController, transport implementation, desktop bridge, or `@tauri-apps/api`. No USB, LAN discovery, WSS, ICE, WebRTC media, microphone permission, actual playback, or background requests to a local host. External repository/release navigation is deliberate link activation, not runtime data fetching. Publish no certificate keys, pairing secrets, private hardware records, or qualification claims unsupported by evidence.

Future site ownership is exclusively `apps/web/src/site/**`. During parallel POC work, this lane must not edit shared `src/ui/**` or styles. Integration owns root/manifests, site Vite configuration, HTML entrypoint, and eventual Actions workflow after contract freeze. Any missing shared primitive is requested from its owner, not independently changed.

## Acceptance and publication gate

Before implementation: user review of this draft and the POC plan. Before publication: integration review of intended public files, code approval, and assigned checks. Verify desktop/mobile 320px layouts, both themes, 200% zoom, keyboard/screen-reader labels, non-drag gain controls, visible focus, reduced motion, unobscured Master controls, simulated badges, and missing/stale states. Assert no LAN, signaling, microphone, or media requests during demo operation. Test project-base asset URLs, hash-route reloads, unknown-route fallback, source/release links, and deployment/404 behavior.

Integration must consult current official Actions/Pages deployment documentation before configuring artifact publication, minimum workflow permissions, and Pages settings. Repository admin access is not evidence of deployment. After publishing `main` and a successful Pages deployment, verify the public address before adding the website/homepage and README links; those changes belong to integration, not this lane.

Sources: current repository documents linked above and [GitHub Pages overview](https://docs.github.com/en/pages/getting-started-with-github-pages/what-is-github-pages). Checks for this draft are documentation/design review only; no site, workflow, browser, or deployment tests have run.
