#!/usr/bin/env python3
"""Explains why two OCI archives of the same image differ (09/T1, deploy/images/REPRODUCIBILITY.md).

    python3 -I deploy/images/oci-diff.py A.oci.tar B.oci.tar

Prints the manifest digests, the image config fields that differ, and for each layer whose digest differs, the files whose
content, mode, owner or time differ. Exit 0 when the manifests are identical, 1 when they differ. Standard library only;
the archives are only read, and nothing in them is extracted to disk.
"""

import gzip
import hashlib
import io
import json
import sys
import tarfile

MAX_FILES = 50


def blob(archive, digest):
    algo, hexdigest = digest.split(":", 1)
    return archive.extractfile(f"blobs/{algo}/{hexdigest}").read()


def manifest_of(archive):
    index = json.loads(archive.extractfile("index.json").read())
    descriptor = index["manifests"][0]
    raw = blob(archive, descriptor["digest"])
    manifest = json.loads(raw)
    if "manifests" in manifest:  # an index inside the layout: take its first image
        descriptor = manifest["manifests"][0]
        raw = blob(archive, descriptor["digest"])
        manifest = json.loads(raw)
    return descriptor["digest"], manifest


def entries(layer_bytes):
    data = layer_bytes
    if data[:2] == b"\x1f\x8b":
        data = gzip.decompress(data)
    out = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as tar:
        for member in tar:
            content = ""
            if member.isfile():
                content = hashlib.sha256(tar.extractfile(member).read()).hexdigest()
            out[member.name] = (member.type, oct(member.mode), member.uid, member.gid, member.mtime, member.size, content,
                                member.linkname)
    return out


def config_diff(a, b, prefix=""):
    keys = sorted(set(a) | set(b)) if isinstance(a, dict) and isinstance(b, dict) else []
    if not keys:
        if a != b:
            print(f"  config {prefix or '.'}: {json.dumps(a)[:200]} != {json.dumps(b)[:200]}")
        return
    for key in keys:
        config_diff(a.get(key), b.get(key), f"{prefix}.{key}")


def main():
    if len(sys.argv) != 3:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    with tarfile.open(sys.argv[1]) as ta, tarfile.open(sys.argv[2]) as tb:
        da, ma = manifest_of(ta)
        db, mb = manifest_of(tb)
        print(f"manifest A {da}\nmanifest B {db}")
        if da == db:
            print("identical")
            return 0
        ca, cb = ma["config"]["digest"], mb["config"]["digest"]
        if ca != cb:
            print(f"config differs: {ca} != {cb}")
            config_diff(json.loads(blob(ta, ca)), json.loads(blob(tb, cb)))
        la, lb = ma.get("layers", []), mb.get("layers", [])
        if len(la) != len(lb):
            print(f"layer count differs: {len(la)} != {len(lb)}")
        for n, (x, y) in enumerate(zip(la, lb)):
            if x["digest"] == y["digest"]:
                continue
            print(f"layer {n} differs: {x['digest']} != {y['digest']}")
            ea, eb = entries(blob(ta, x["digest"])), entries(blob(tb, y["digest"]))
            shown = 0
            for name in sorted(set(ea) | set(eb)):
                if ea.get(name) != eb.get(name):
                    print(f"  {name}\n    A {ea.get(name)}\n    B {eb.get(name)}")
                    shown += 1
                    if shown == MAX_FILES:
                        print("  ...")
                        break
            if shown == 0:
                print("  same files, metadata and content: the difference is in the tar or gzip encoding")
        return 1


if __name__ == "__main__":
    sys.exit(main())
