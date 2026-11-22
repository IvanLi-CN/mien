<!-- SEED: established with the user before implementation; re-run /impeccable document once there's code to capture the actual tokens and components. -->

---
name: Snailfish Operate
description: A working proofing desk for isolated browser profiles.
---

# Design System: Snailfish Operate

## Overview

**Creative North Star: "The Working Proofing Desk"**

Snailfish's Operate surface should feel like a press proofing desk, not a marketing page. The world is offset proof sheets and job tickets: paper, ink, process colors, registration marks, and a strip that proves a check actually happened. The tool is the artifact; the print language enters only as evidence, density, and one signature move.

The interface is deliberately dense. A profile list is a working register, not a gallery: many records, clear states, a persistent detail surface, and standard controls the operator already knows. Paper gives the ground, ink gives structure, and process colors are spent on verification states rather than decoration.

Motion is state feedback, not choreography: 150-200 ms feedback, no page-load sequence, no scroll reveal. The proof strip changes only when a condition changes. The selected row's registration crosshair hands off to the job ticket; that is the memorable moment.

**Key Characteristics:**
- Paper-and-ink ground with process-color evidence.
- Dense table plus persistent detail surface.
- Standard controls, 1px hairlines, square data marks.
- Four-block proof strip and registration crosshair as the signature.
- Color used only when it carries state or verification meaning.

## Colors

The palette is restrained by structure but committed in its ink shell: paper ground, ink navigation, process cyan for primary action and selection, and three state colors used only where they mean something.

### Primary

- **Press Ink** (#17191C): top-level shell, primary buttons, table rules, and the job-ticket header. It is the structural ink, not a background for every panel.
- **Proof Paper** (#F3F1E9): application ground and the main table surface. It is a cool-neutral paper, not a warm cream.
- **Process Cyan** (#007C91): primary action, selected-row crosshair, focus, and the active tab underline. It is used on a small fraction of the screen.

### Secondary

- **Proof Yellow** (#C99700): attention and degraded state in the proof strip and status text.
- **Verified Green** (#2F6B4F): verified fingerprint, healthy proxy, and running state.
- **Error Red** (#B3261E): failed check and crashed runtime.

### Neutral

- **Ticket Surface** (#FBFAF5): job-ticket body, inputs, and raised panels.
- **Hairline** (#D8D4C9): table rules, panel borders, and dividers.
- **Muted Ink** (#6B6B66): secondary labels, timestamps, and supporting text.

**The Evidence Color Rule.** A process color in the proof strip always represents a check state. It never decorates a row, icon, or heading.

**The One Ink Rule.** There is one structural ink. Do not introduce a second dark shell, a navy variant, or a gradient panel.

## Typography

**Display Font:** system UI sans (`-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif`)

**Body Font:** the same system UI sans

**Label/Mono Font:** `ui-monospace, SFMono-Regular, Menlo, monospace`

**Character:** one workhorse sans carries the interface; hierarchy comes from size and weight, not a display face. Mono is reserved for job numbers, seeds, measurements, and short evidence labels, where alignment and provenance matter.

### Hierarchy

- **Headline** (650, 27px, 1.1, -0.02em): page title; one per screen.
- **Title** (650, 20-23px, 1.15, -0.02em): selected profile or ticket title.
- **Body** (400, 13px, 1.45): table content, descriptions, and controls.
- **Label** (600, 10px, 1.3, 0.08-0.1em uppercase, mono): column labels, job numbers, proof-strip keys.
- **Data** (500-600, 11-12px, tabular numerals): measurements, seeds, latency, counts.

**The Mono Is Evidence Rule.** Mono marks an identifier or a measurement. It is never used for prose or as a costume for "technical".

**The One Family Rule.** There is no display/body pairing on Operate. One sans plus one mono is the full type system.

## Layout

The spatial grammar is a dense working register with a persistent detail surface. Use an 8px base rhythm; a 56px table row; 24px page gutters; 1px rules; a full-width table; and a 320-360px detail surface on wide screens. Top-level navigation may be a top bar or a rail, but it must read as navigation, not as a printed sheet.

Responsive behavior is structural: at 1024, the detail surface becomes a drawer or moves below the table; at 768, the table becomes a stacked record list rather than a squeezed grid; at 390, one column with full-width actions. Do not convert the register into a wall of cards.

**The Table Before Cards Rule.** Profiles are rows because comparison is the job. Cards are allowed only for a genuinely single-record surface, never as the default list container.

**The Equal Edge Rule.** Table, filters, headings, and detail surface share the same content edges. No element floats out to one side.

## Elevation & Depth

The system is flat by default. Depth comes from the ink shell, the tinted selected row, the ticket surface, and hairline separation, not from shadows. Floating overlays may use one soft shadow when they genuinely leave the page; static panels stay flat.

Do not use glass, blur, gradient glows, or zero-offset halos. Do not use a colored left border to indicate selection; use a full-row ground tint plus the registration crosshair.

## Shapes

Controls use a tight 2-3px radius. Table and ticket surfaces use 0-3px radius; data marks, proof blocks, and table structure are square. Filter chips may be pill-shaped because they are standard selection controls. Hairlines are 1px and carry the grid; heavy borders and large rounded cards are out.

**The Square Evidence Rule.** Proof blocks, stamps, and data marks are square or circular only when the mark itself is a stamp. They never become decorative rounded tiles.

## Do's and Don'ts

### Do:

- **Do** make the profile table the first read and the selected job ticket the second.
- **Do** use the four-block proof strip for fingerprint, proxy, runtime, and isolation state.
- **Do** use the registration crosshair for the selected row and the ticket header.
- **Do** use mono for job numbers, seeds, latency, and evidence labels.
- **Do** keep standard controls: buttons, tabs, inputs, filters, sortable table headers.
- **Do** disclose host-bound screen and font limits in the interface.

### Don't:

- **Don't** add paper textures, torn edges, ink splatter, or print registration furniture as decoration.
- **Don't** use process colors on inactive rows, headings, or backgrounds.
- **Don't** use display serifs, gradient text, glass, or neon accents.
- **Don't** use monospace for prose or labels that are not identifiers or measurements.
- **Don't** replace the table with a card wall or a poster grid.
- **Don't** use colored left borders above 1px to indicate state.
- **Don't** import landing-page composition, hero treatments, or scroll choreography into Operate.
