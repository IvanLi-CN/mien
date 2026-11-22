---
version: 1
slug: "ui-src-pages-profiles"
primary_target: "ui/src/pages/profiles"
related_targets: []
---

# Profiles surface brief

## Scope

- Surface: the Profiles management screen, first screen of the Snailfish Operate surface.
- Mode: Operate. The user is here to scan environments, verify state, select a profile, and start or stop it.
- Host: shared web UI in the Tauri macOS app and in the Docker web host. Desktop transport is Tauri IPC; server transport is HTTP/WS.

## Audience and job

- Primary user: a multi-account operator or QA engineer working through a long list of isolated environments.
- Job: compare profiles quickly, see which fingerprint/proxy/runtime/isolation conditions hold, open one profile's evidence, and act without hunting.
- Primary action: stop a running runtime; create a new profile is the secondary global action.
- Proof/content: synthetic sample profiles, proxy nodes, seeds, and states. No real customers, benchmarks, or testimonials.
- Constraints: standard web controls, dense but readable table, no local HTTP management port, honest host-bound fingerprint limits, Tauri/window widths 1440 and 1024 first.

## Direction contract

THESIS: A browser-profile manager that reads as a working proofing desk: every profile is a proof job with a verifiable strip, not a neutral dashboard row. It refuses the category-default arrangement of grey rows, one blue accent, and status dots with no evidence.

OWN-WORLD: Offset press proof sheet and job ticket. Paper #F3F1E9, ink #17191C, process cyan #007C91, proof yellow #C99700, error red #B3261E, verified green #2F6B4F. Hairline rules, mono job numbers, registration crosshair, four-block proof strip. Standard controls with 0-3px radius; no paper texture, no gradient, no decorative print furniture.

STORY: The operator understands at a glance which environments are verified, degraded, or failed, and why. They select a profile, read its job ticket and proof strip, and trust that a failed check is visible rather than hidden. The strip is evidence, not ornament.

FIRST VIEWPORT: 1440x900. Ink top bar 60px with Snailfish wordmark, Profiles/Proxies/Kernels/Activity tabs, search, and New profile. Paper main area: 27px "Profiles" heading, 11px count, filter chips, full-width 10-row table with 56px rows. Selected row gets a cyan registration crosshair and pale cyan ground. Right 350px job ticket: black header with profile name and job number, 4-block proof strip, compact facts, host-bound note, recent events, and cyan "Stop runtime" primary action at the bottom. The table is the first read; the ticket is the selected proof.

FORM: Press Proof, position 1 of 7 in the grounded list; seed:a45df6eacd66ed4f. Identity is the proof strip and registration crosshair; standard navigation and controls stay standard. Flat paper ground, no second visual world.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance.

## Open decisions

- None at direction level. Responsive table priorities at 1024 and the final icon set are build-level decisions this contract constrains rather than opens.
