# UI fixtures

- **Owner:** UI maintainers.
- **Purpose:** Capture state semantics for profile/runtime status and isolation proof presentation.
- **Commit:** Authored semantic JSON snapshots only; never commit screenshots, browser profiles, accessibility reports, or user data.
- **Generate/fetch:** Author snapshots from `docs/UI-SURFACES.md` and the runtime contract; visual and accessibility evidence remains an external test artifact.

The baseline snapshot distinguishes a stopped runtime from an unknown isolation verdict.
