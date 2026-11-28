#!/usr/bin/env python3
"""Turns one build's buildx metadata plus its exported OCI archive into a build record.

`repro-compare.mjs` needs a machine-readable description of what a build produced,
taken from the artifact itself rather than from log scraping. This script reads the
`index.json` of the OCI archive, the manifest it points at, and the image config, so
the recorded manifest digest, layer list and diff IDs all come from bytes on disk.

The archive is written by the OCI exporter with `rewrite-timestamp=true` and no image
name, so `index.json` holds exactly one manifest descriptor with no name annotation.
That is asserted, not assumed: an archive with a different shape fails here instead of
letting the comparison run on the wrong digest.
"""

import argparse
import hashlib
import json
import sys
import tarfile
from datetime import datetime, timezone


def fail(message):
    print(f"repro-record: FAIL\n  - {message}", file=sys.stderr)
    raise SystemExit(1)


def read_member(archive, name):
    try:
        member = archive.getmember(name)
    except KeyError:
        fail(f"{archive.name}: no {name} member; not an OCI image archive")
    extracted = archive.extractfile(member)
    if extracted is None:
        fail(f"{archive.name}: {name} is not a regular file")
    return extracted.read()


def blob_name(digest):
    algorithm, _, encoded = digest.partition(":")
    if algorithm != "sha256" or not encoded:
        fail(f"unsupported digest {digest!r}; only sha256 is accepted")
    return f"blobs/sha256/{encoded}"


def parse_timestamp(value):
    """Parse an OCI RFC 3339 timestamp; return None when it is not one."""
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def read_blob(archive, digest, label):
    """Read a blob and prove the bytes hash to the digest that named it.

    Without this, `index.json` is taken at its word and two archives could agree on a
    digest string while holding different content.
    """
    payload = read_member(archive, blob_name(digest))
    actual = "sha256:" + hashlib.sha256(payload).hexdigest()
    if actual != digest:
        fail(f"{archive.name}: {label} digest {digest} does not hash to its content ({actual})")
    return payload


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata", required=True, help="buildx --metadata-file output")
    parser.add_argument("--archive", required=True, help="OCI archive written by --output type=oci")
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--source-date-epoch", required=True, type=int)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--frontend", required=True)
    parser.add_argument("--buildkit-image", required=True)
    parser.add_argument("--buildx-version", required=True)
    parser.add_argument("--compression", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    with open(args.metadata, encoding="utf-8") as handle:
        metadata = json.load(handle)
    image_digest = metadata.get("containerimage.digest")
    if not image_digest:
        fail(f"{args.metadata}: no containerimage.digest; the build recorded no image")

    try:
        archive = tarfile.open(args.archive)
    except (OSError, tarfile.TarError) as error:
        fail(f"{args.archive}: cannot open as a tar archive ({error})")

    with archive:
        index = json.loads(read_member(archive, "index.json"))
        layout = json.loads(read_member(archive, "oci-layout"))
        manifests = index.get("manifests") or []
        if len(manifests) != 1:
            fail(
                f"{args.archive}: index.json lists {len(manifests)} manifests; "
                "a single-platform build must list exactly one"
            )
        descriptor = manifests[0]
        if descriptor.get("annotations", {}).get("org.opencontainers.image.ref.name"):
            fail(
                f"{args.archive}: the manifest is named; omit image names from the build "
                "so the comparison binds to the bare manifest digest"
            )
        oci_manifest_digest = descriptor["digest"]
        manifest_bytes = read_blob(archive, oci_manifest_digest, "manifest")
        manifest = json.loads(manifest_bytes)
        if len(manifest_bytes) != descriptor.get("size"):
            fail(
                f"{archive.name}: index.json says the manifest is {descriptor.get('size')} bytes, "
                f"the blob is {len(manifest_bytes)}"
            )
        config_descriptor = manifest["config"]
        config = json.loads(read_blob(archive, config_descriptor["digest"], "config"))
        for layer in manifest.get("layers", []):
            read_blob(archive, layer["digest"], f"layer {layer['digest']}")

    record = {
        "source_sha": args.source_sha,
        "source_date_epoch": args.source_date_epoch,
        "platform": args.platform,
        "frontend": args.frontend,
        "buildkit_image": args.buildkit_image,
        "buildx_version": args.buildx_version,
        "compression": args.compression,
        "image_digest": image_digest,
        "oci_manifest_digest": oci_manifest_digest,
        "oci_layout_version": layout.get("imageLayoutVersion"),
        "manifest_media_type": manifest.get("mediaType"),
        "config_digest": config_descriptor["digest"],
        # The platform the archive was actually built for, read from the config, so a
        # wrong --platform cannot pass as a matching build.
        "image_platform": f"{config.get('os')}/{config.get('architecture')}",
        "created": config.get("created"),
        "layers": [
            {
                "mediaType": layer.get("mediaType"),
                "digest": layer["digest"],
                "size": layer.get("size"),
            }
            for layer in manifest.get("layers", [])
        ],
        # OCI image config uses snake_case here (`rootfs.diff_ids`); the Docker schema
        # v2 config it descends from does the same.
        "diff_ids": config.get("rootfs", {}).get("diff_ids", []),
        "history": config.get("history", []),
    }

    if record["image_platform"] != args.platform:
        fail(f"archive platform is {record['image_platform']}, expected {args.platform}")
    if record["oci_layout_version"] != "1.0.0":
        fail(f"unexpected OCI layout version {record['oci_layout_version']!r}")
    if not record["layers"] or not record["diff_ids"]:
        fail("archive has no layers; the probe image must install packages")
    if len(record["layers"]) != len(record["diff_ids"]):
        fail("layer count and diff ID count disagree; the archive is inconsistent")

    # Prove SOURCE_DATE_EPOCH actually reached the frontend. BuildKit stamps each
    # history entry created by *this* Dockerfile with the resolved epoch verbatim
    # (`commitToHistory(..., d.epoch)`) and only clamps timestamps inherited from the
    # base image, so the last entry must equal the epoch exactly. Without the build arg
    # that entry carries the build wall-clock time instead, and the two digests would
    # differ for a reason nobody could see in the artifact.
    if not record["history"]:
        fail("archive config has no history; cannot confirm SOURCE_DATE_EPOCH took effect")
    last_created = record["history"][-1].get("created")
    epoch_dt = datetime.fromtimestamp(args.source_date_epoch, tz=timezone.utc)
    if last_created is None:
        fail("the final history entry has no created timestamp; SOURCE_DATE_EPOCH did not apply")
    parsed = parse_timestamp(last_created)
    if parsed is None:
        fail(f"unparseable final history timestamp {last_created!r}")
    if parsed != epoch_dt:
        fail(
            f"final history entry created is {last_created}, not SOURCE_DATE_EPOCH "
            f"{epoch_dt.isoformat().replace('+00:00', 'Z')}"
        )

    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(record, handle, indent=2)
        handle.write("\n")

    print(
        f"repro-record: {record['image_platform']} manifest {record['oci_manifest_digest']} "
        f"config {record['config_digest']} layers {len(record['layers'])}"
    )
    print(f"repro-record: buildx reported {image_digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
