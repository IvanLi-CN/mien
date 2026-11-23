---
version: 1
slug: "ui-src-pages-profiles"
primary_target: "ui/src/pages/profiles"
related_targets: []
---

# Profiles surface brief

## Scope

- Surface: the Profiles management screen, first screen of the Mien Operate surface.
- Mode: Operate. The user is here to scan spaces and environments, verify state, open a live session, and start or stop a profile.
- Host: shared web UI in the Tauri macOS app and in the Docker web host. Desktop transport is Tauri IPC; server transport is HTTP/WS.

## Audience and job

- Primary user: a multi-account operator or QA engineer working through a long list of isolated environments.
- Job: see what is live, compare profiles by space, inspect fingerprint/proxy/runtime/isolation proof, and act without hunting.
- Primary action: open or stop a live session; creating a profile is the global secondary action.
- Proof/content: synthetic sample profiles, proxy nodes, seeds, and states. No real customers, benchmarks, or testimonials.
- Constraints: standard web controls, dark spatial field, no table, no permanent inspector, honest host-bound and headless limits, Tauri/window widths 1440 and 1024 first.

## Direction contract

THESIS: A spatial, fail-closed workspace where each profile is a recognizable identity with visible proof, not a row in an admin table. It refuses the category default of light dashboards, uniform card walls, and status dots without evidence.

OWN-WORLD: Deep Current. Abyss `#080A0F`, rail `#0D1016`, surface `#11151C`, raised `#171C25`, ink `#F4F7FC`, muted `#8E97A8`, line `rgba(255,255,255,.09)`. Identity gradients: cobalt→cyan, coral→amber, lime→cyan, amber→coral. State colors: running `#45D483`, verified `#5BE1F5`, warning `#FFB454`, failed `#FF6B6B`, stopped `#8E97A8`. Floating surfaces, 16–20px radii, 1px borders, deterministic seed auras, four proof blocks. No glass, no neon, no paper, no pure black or white.

STORY: The operator lands in a Space, immediately sees which environments are live and where traffic goes, scans a field of profiles with stable identity colors, and can read the four proof dimensions before opening or stopping anything. A failed check is visible, not hidden behind hover or a detail page.

FIRST VIEWPORT: 1440×900. A 222px Space rail with group identities and a single global live status; a 54px command bar with search, Import, New profile, and avatar; a 46px `Growth` title with space counts; a 286px live area: one large Aurora Supply slab (route, latency, uptime, fingerprint, sparkline, Open/Stop) plus stacked Harbor Ads and Fieldnote cards; a 12-column profile field with six profiles in 5/4/3 and 4/5/3 spans, each with orb, state, platform, proxy, fingerprint signature, proof strip, and action; a compact footer with global totals and fail-closed status.

FORM: Deep Current, approved by the owner. The Space rail, Live slab, and profile field use asymmetric widths; the profile field is a scan surface, not a uniform four-column grid. State is text plus dot/proof, never color alone. The detail surface is a floating drawer or route, never a permanent right inspector.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, the updated DESIGN.md, and the approved comp at `docs/design/profiles-dark.png`.

## Open decisions

- None at direction level. The profile detail surface (floating drawer versus route) and final icon set are build-level decisions this contract constrains rather than opens.
