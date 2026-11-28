# Host-side AppArmor profile

`mien-chromium` confines the browser container. Docker's default profile denies
`mount`/`umount2`/`pivot_root`/`chroot` without `CAP_SYS_ADMIN`, which breaks
Chromium's namespace sandbox, so the container is started with
`--security-opt apparmor=mien-chromium` after the host operator installs this
file at `/etc/apparmor.d/mien-chromium`.

`SECURITY.md` allows a second, documented option: run with
`--security-opt apparmor=unconfined` and record the accepted risk. That fallback
never applies silently; the spike runner requires `MIEN_ALLOW_UNCONFINED=1`.

## Install

```sh
install -m 0644 mien-chromium /etc/apparmor.d/mien-chromium
apparmor_parser -r /etc/apparmor.d/mien-chromium
```

## Release obligations

- This file is a host-side release artifact: its SHA-256 belongs in
  `SHA256SUMS`, the SBOM and the artifact attestation, next to the seccomp
  profile.
- The container image carries no copy of this profile; only the host path is
  authoritative.
- SHA-256: `3b5971053689731de88a2df9b0881c5172cbd258509145b2a3cc4f8054c88e98`
- Status: the profile is authored and structure-checked only. It has not been
  loaded on any available host. Both Agent VM hosts are Incus containers: even
  the `Ubuntu 24.04 Rust privileged` image runs with full `CAP_SYS_ADMIN` and
  `unconfined`, yet `mount -t securityfs securityfs /sys/kernel/security` returns
  `EPERM`, so `apparmor_parser` cannot load a policy there. Policy loading must
  happen on the host (or a host where `securityfs` is mountable). Do not claim
  the hardened path is verified until a host load succeeds and the spike passes
  with `apparmor=mien-chromium`.
