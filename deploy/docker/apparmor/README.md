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
- Status: the profile is authored and reviewed by structure only. It has not
  been loaded on a host that permits AppArmor policy loading; both available
  test hosts are unprivileged containers where `securityfs` cannot be mounted.
  Do not claim the hardened path is verified until a host load succeeds and the
  spike passes with `apparmor=mien-chromium`.
