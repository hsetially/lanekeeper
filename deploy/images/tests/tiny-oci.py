#!/usr/bin/env python3
"""An OCI image of one layer holding one file, written by hand, so that a test needs no builder and no registry (09, CI first run).

    python3 -I deploy/images/tests/tiny-oci.py FILE.oci.tar

The archive is an OCI image layout (oci-layout, index.json, blobs/sha256/...) that syft and grype read as `oci-archive:`. It is the same
bytes on every run: the timestamps are 0.
"""
import hashlib, io, json, sys, tarfile

def blob(data):
    return hashlib.sha256(data).hexdigest(), data

def tar_of(members):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode='w') as t:
        for name, content in members:
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mtime = 0
            t.addfile(info, io.BytesIO(content))
    return buf.getvalue()

layer_digest, layer = blob(tar_of([('hello', b'hello\n')]))
config_digest, config = blob(json.dumps({'architecture': 'amd64', 'os': 'linux', 'config': {},
                                          'rootfs': {'type': 'layers', 'diff_ids': ['sha256:' + layer_digest]}}).encode())
manifest_digest, manifest = blob(json.dumps({
    'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.manifest.v1+json',
    'config': {'mediaType': 'application/vnd.oci.image.config.v1+json', 'digest': 'sha256:' + config_digest, 'size': len(config)},
    'layers': [{'mediaType': 'application/vnd.oci.image.layer.v1.tar', 'digest': 'sha256:' + layer_digest, 'size': len(layer)}],
}).encode())
index = json.dumps({'schemaVersion': 2, 'manifests': [{
    'mediaType': 'application/vnd.oci.image.manifest.v1+json', 'digest': 'sha256:' + manifest_digest, 'size': len(manifest),
    'platform': {'architecture': 'amd64', 'os': 'linux'}}]}).encode()
files = [('oci-layout', b'{"imageLayoutVersion":"1.0.0"}'), ('index.json', index)]
files += [('blobs/sha256/' + d, b) for d, b in (blob(layer), blob(config), blob(manifest))]
with tarfile.open(sys.argv[1], 'w') as t:
    for name, content in files:
        info = tarfile.TarInfo(name)
        info.size = len(content)
        info.mtime = 0
        t.addfile(info, io.BytesIO(content))
