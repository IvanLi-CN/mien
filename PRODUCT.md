# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Tauri 2 + React 19 + TypeScript + Vite + Rust core, confirmed by the user. The management UI is a web UI shipped inside the macOS Tauri app and served by the Docker web host; the Rust core is shared by both hosts.

## Users

- Multi-account operators who maintain isolated store, social, or advertising environments and need stable profiles, proxy isolation, and simple daily operation.
- QA and compatibility engineers who reproduce regional, language, and platform personas and need repeatable environments and automation entry points.
- Privacy and anti-tracking researchers who inspect how sites behave under controlled fingerprints and need transparent limits.
- Automation developers who drive multiple independent browser environments through a stable API and CDP.
- Small team operators who run browser environments on a server and need Docker deployment, web access, persistence, and observability.

## Product Purpose

Snailfish manages multiple isolated browser profiles, each with its own browser data directory, reproducible fingerprint specification, forward proxy, and supervised runtime. It runs as a macOS Apple Silicon desktop app and as a Docker `linux/amd64` service. Success means a user can create a profile, launch a coherent environment, verify its proxy and fingerprint behavior, and stop it without leaking data or state.

## Positioning

Snailfish is a local-first, dual-host environment management platform, not just a launcher. It combines a reproducible fingerprint specification, a per-profile protocol engine for VLESS, VMess, Shadowsocks, Trojan, and related protocols, fail-closed proxying, and honest disclosure of kernel and host limitations. The mechanism a neighboring launcher could not truthfully copy is the shared Rust core driving both the macOS desktop app and the Docker web service, with the same profile, fingerprint, proxy, and runtime semantics.

## Operating Context

Users work from a macOS desktop or a browser connected to a Docker deployment. Profiles are organized by groups, tags, notes, and runtime state. Each profile binds a fingerprint specification, a proxy configuration, and a start URL. Running profiles expose status, logs, proxy checks, and an optional loopback CDP endpoint for automation. The server edition uses a shared Xvfb display for headful profiles and supports headless profiles as an explicit reduced-assurance mode.

## Capabilities and Constraints

- Profile CRUD, duplication, archiving, groups, tags, search, import, export, and encrypted export.
- Fingerprint fields: seed, platform, platform version, brand, brand version, CPU cores, languages, timezone, window size, spoof toggles, and WebRTC policy.
- Forward proxy support: HTTP, HTTPS, SOCKS5, VLESS, VMess, Shadowsocks, Trojan; Hysteria2, TUIC, and WireGuard are Tier B and depend on the protocol engine capability manifest.
- Per-profile sing-box protocol engine and loopback mixed inbound; credentials never enter Chromium.
- Fail-closed networking, remote DNS for target hosts, WebRTC non-proxied UDP blocked by default, DoH bypass disabled, IPv6 handled explicitly.
- macOS Apple Silicon and macOS 13+ only; Docker `linux/amd64` only. macOS Intel and native Linux ARM64 are out of scope for the first release.
- Docker on Apple Silicon runs the amd64 image through emulation; Hysteria2 and TUIC require UDP egress.
- fingerprint-chromium `148.0.7778.215` is pinned. Known kernel limitations, including canvas and font behavior, must be disclosed rather than hidden.
- Screen size, device pixel ratio, color depth, and font enumeration are host-bound and cannot be promised as per-profile spoofed values.
- sing-box is GPL-3.0-or-later and distributed as a separate process; legal review and Xray-core fallback are release constraints.
- No cloud sync, team RBAC, subscription refresh, node marketplace, or built-in proxy sales in the first release.
- The product does not provide anonymity or a guarantee of passing third-party detection.

## Brand Commitments

- Name: Snailfish.
- Voice: precise, calm, technical, and honest. No unverifiable claims such as “undetectable”, “100% isolated”, or “absolutely safe”.
- Interface languages: Simplified Chinese and English. Profile language, timezone, and proxy region are independent of the UI language.
- Visual system: Deep Current, a dark spatial workspace with space identities, deterministic seed auras, and four-block proof strips. The canonical rules and tokens live in `DESIGN.md`; the approved first-screen comp is `docs/design/profiles-dark.png`.
- No logo or licensed photography has been established. Future brand work must extend Deep Current rather than introduce a second visual world.

## Evidence on Hand

- Product requirements, architecture, detailed design, technology selection, security model, test plan, ADRs, and topic specifications live in `docs/`.
- No user research, customer interviews, testimonials, screenshots, product demos, pricing, or production analytics exist yet.
- An approved visual-direction comp exists at `docs/design/profiles-dark.png`; no production UI assets, licensed photography, or illustrations exist yet. Future design work must not fabricate customers, benchmarks, or product screenshots.

## Product Principles

1. Isolation and reproducibility outrank feature breadth; a profile must be explainable, repeatable, and removable.
2. Limits are part of the product truth; host-bound fingerprint fields, headless degradation, and kernel defects are disclosed in the interface.
3. Security defaults are fail-closed; proxy or engine failure must never become a silent direct connection.
4. Local-first is the default; server deployment is an explicit operating mode, not a requirement for desktop use.
5. The interface is a spatial workspace: spaces, live sessions, and profile identities are visible at a glance; scanning, proof, and recovery outrank decorative novelty.

## Accessibility & Inclusion

The product requires keyboard-reachable primary actions, visible labels, non-color-only state, readable contrast, and bilingual copy. No product-specific accessibility standard or assistive-technology research has been established beyond the PRD’s baseline requirements.
